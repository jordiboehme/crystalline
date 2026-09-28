/**
 * The director: the sound sink the session sends its cues to, playing
 * each through the mixer (M4 C20 to C23, C28).
 *
 * The session names moments (`audio/cues.ts`); the director picks their
 * patches (`audio/effects.ts`, `audio/ambience.ts`) and plays them on a bus
 * of the mixer (`audio/mixer.ts`), every effect on `effects` and the drone
 * on `ambience`:
 *
 * - **One-shots.** A step, a door, a portal's or a hatch's travel, a
 *   fault, a terminal. A placed cue (a door, a fault) plays through a
 *   stereo panner at its pan into the bus, its voices scaled by its gain.
 *   At most `VOICE_CAP` patches play at once, the drone and the ride's hum
 *   counted; one more drops the oldest step or fault first (a 30 ms fade),
 *   and only when none is left the oldest other one-shot. The drone and the
 *   hum are never dropped. While the context is not running the one-shots
 *   are dropped: its clock stands still, and everything scheduled on it
 *   would sound at once when it runs again.
 * - **The drone.** One looping drone per room: a `room` cue with the
 *   ambience playing keeps it (whatever the seed), another cross-fades,
 *   the old one fading out over `CROSSFADE_S` while the new one fades in
 *   over its attack. The wanted ambience is kept even while there is no
 *   context (a reload, where the mixer makes its context on the first
 *   click), and the drone starts once an unlock makes one.
 * - **The lift ride.** `depart` plays the doors shutting and holds the
 *   rising hum (a looping patch). `arrive` stops the hum (a
 *   `RIDE_FADE_S` fade) and owes the chime and the doors opening to the
 *   room the ride lands in: the session sends the landing's `room` cue in
 *   the same call as the `arrive`, so the chime plays if a `room` cue comes
 *   before the next microtask, and a ride that ends without landing (a
 *   failed stop, a load error, a new place) stops its hum in silence.
 * - **Mute** is the master's alone (`toggleMute`, the mixer's): the cues
 *   play on into a silent master, so sound coming back mid-room brings the
 *   drone back at once.
 * - **Gestures.** A click or a key on the window while the context is not
 *   running unlocks it (Safari may refuse the resume after a hidden tab,
 *   F29), inside that gesture; the first one also makes the context on a
 *   reload.
 *
 * `suspend` and `resume` are the host's (the pause and a hidden tab);
 * `dispose` stops everything, takes the listeners down and closes the
 * mixer. The `box`, `jump` and `answer` cues are the signature sounds'
 * (M4 C24) and play nothing yet.
 */

import { CROSSFADE_S, dronePatch } from "./ambience";
import type { AudioNodeLike } from "./context";
import type { Ambience, Cue, SoundSink } from "./cues";
import {
  doorPatch,
  faultPatch,
  hatchPatch,
  portalPatch,
  ridePatch,
  stepPatch,
  terminalPatch,
} from "./effects";
import type { Bus, Mixer } from "./mixer";
import type { Patch } from "./patch";
import { playPatch, type PlayingPatch } from "./synth";

/** The most patches that play at once, the drone and the ride's hum counted. */
export const VOICE_CAP = 24;

/** How fast the ride's hum falls silent when the ride arrives. */
export const RIDE_FADE_S = 0.04;

/** The game's sound sink, with the lifecycle its host drives. */
export interface Director extends SoundSink {
  /** Suspends the context: the pause, a hidden tab. */
  suspend(): void;
  /** Resumes the context after a suspend. */
  resume(): void;
  /** From a gesture: unlocks the context, starting the room's drone if owed. */
  unlock(): void;
  /** Stops every sound, drops the listeners and closes the mixer. */
  dispose(): void;
}

/** A one-shot that is playing: what it is, and its panner if placed. */
interface Shot {
  playing: PlayingPatch;
  /** Steps and faults are the first dropped at the cap. */
  cheap: boolean;
  panner: AudioNodeLike | null;
}

/** How a one-shot is played. */
interface ShotOptions {
  pan?: number;
  gain?: number;
  cheap?: boolean;
}

/** The director over `mixer`. */
export function createDirector(mixer: Mixer): Director {
  let disposed = false;
  /** The one-shots still playing, oldest first. */
  let shots: Shot[] = [];
  /** One-shots dropped at the cap, fading, whose panners wait to go. */
  let fading: Shot[] = [];
  /** The room's ambience and seed, kept when there is no context. */
  let wanted: { ambience: Ambience; seed: number } | null = null;
  let drone: { ambience: Ambience; playing: PlayingPatch } | null = null;
  let hum: PlayingPatch | null = null;
  /** Whether a ride arrived and its chime waits for the room it lands in. */
  let chimeOwed = false;

  const held = () => (drone === null ? 0 : 1) + (hum === null ? 0 : 1);

  /** Forgets the one-shots that have ended, disconnecting their panners. */
  const prune = (now: number) => {
    const ended = (s: Shot) => s.playing.end <= now;
    for (const s of [...shots, ...fading]) {
      if (ended(s)) s.panner?.disconnect();
    }
    shots = shots.filter((s) => !ended(s));
    fading = fading.filter((s) => !ended(s));
  };

  /** Drops the oldest step or fault, else the oldest one-shot. */
  const evict = () => {
    const i = shots.findIndex((s) => s.cheap);
    const [gone] = shots.splice(i < 0 ? 0 : i, 1);
    if (gone === undefined) return;
    gone.playing.stop();
    fading.push(gone);
  };

  /**
   * Plays `patch` on `bus` from now: through a panner when placed, its
   * voices scaled by `gain`. Null without a context or bus.
   */
  const start = (
    patch: Patch,
    bus: Bus,
    { pan, gain = 1 }: ShotOptions = {},
  ): { playing: PlayingPatch; panner: AudioNodeLike | null } | null => {
    const ctx = mixer.ctx;
    const out = mixer.bus(bus);
    if (ctx === null || out === null) return null;
    let dest = out;
    let panner: AudioNodeLike | null = null;
    if (pan !== undefined) {
      const node = ctx.createStereoPanner();
      node.pan.value = pan;
      node.connect(out);
      panner = node;
      dest = node;
    }
    const played =
      gain === 1
        ? patch
        : {
            ...patch,
            voices: patch.voices.map((v) => ({ ...v, gain: v.gain * gain })),
          };
    const playing = playPatch(ctx, dest, played, ctx.currentTime, patch.name);
    return { playing, panner };
  };

  /** A one-shot on the effects bus, under the cap; dropped while not running. */
  const shot = (patch: Patch, options: ShotOptions = {}) => {
    const ctx = mixer.ctx;
    if (disposed || ctx === null || !mixer.running) return;
    if (options.gain !== undefined && !(options.gain > 0)) return;
    prune(ctx.currentTime);
    while (shots.length > 0 && shots.length + held() >= VOICE_CAP) evict();
    const played = start(patch, "effects", options);
    if (played === null) return;
    shots.push({ ...played, cheap: options.cheap === true });
  };

  /** Starts the wanted drone when there is none and a context to play it. */
  const startDrone = () => {
    if (disposed || wanted === null || drone !== null) return;
    const played = start(dronePatch(wanted.ambience, wanted.seed), "ambience");
    if (played !== null) {
      drone = { ambience: wanted.ambience, playing: played.playing };
    }
  };

  /** A room entered: its drone, kept for the same ambience, else faded to. */
  const room = (ambience: Ambience, seed: number) => {
    if (chimeOwed) {
      chimeOwed = false;
      shot(ridePatch("arrive"));
      shot(doorPatch("sliding", true));
    }
    const same = wanted?.ambience === ambience;
    wanted = { ambience, seed };
    if (same && drone !== null) return;
    drone?.playing.stop(CROSSFADE_S);
    drone = null;
    startDrone();
  };

  const ride = (phase: "depart" | "arrive") => {
    hum?.stop(RIDE_FADE_S);
    hum = null;
    if (phase === "arrive") {
      chimeOwed = true;
      queueMicrotask(() => {
        chimeOwed = false;
      });
      return;
    }
    chimeOwed = false;
    shot(doorPatch("sliding", false));
    if (disposed || !mixer.running) return;
    hum = start(ridePatch("depart"), "effects")?.playing ?? null;
  };

  const cue = (c: Cue) => {
    if (disposed) return;
    switch (c.kind) {
      case "step":
        shot(stepPatch(c.n, c.run, c.foot), { cheap: true });
        return;
      case "door":
        shot(doorPatch(c.sound, c.open), { pan: c.pan, gain: c.gain });
        return;
      case "travel":
        if (c.via === "portal") shot(portalPatch());
        else if (c.via === "hatch") shot(hatchPatch());
        return;
      case "fault":
        shot(faultPatch(c.way, c.run), {
          pan: c.pan,
          gain: c.gain,
          cheap: true,
        });
        return;
      case "terminal":
        shot(terminalPatch());
        return;
      case "ride":
        ride(c.phase);
        return;
      case "room":
        room(c.ambience, c.seed);
        return;
      case "box":
      case "jump":
      case "answer":
        return;
    }
  };

  const unlock = () => {
    if (disposed) return;
    mixer.unlock();
    startDrone();
  };

  const onGesture = () => {
    if (!mixer.running) unlock();
  };
  window.addEventListener("click", onGesture, true);
  window.addEventListener("keydown", onGesture, true);

  return {
    cue,
    toggleMute: () => mixer.toggleMute(),
    suspend: () => {
      if (!disposed) mixer.suspend();
    },
    resume: () => {
      if (!disposed) mixer.resume();
    },
    unlock,
    dispose() {
      if (disposed) return;
      disposed = true;
      window.removeEventListener("click", onGesture, true);
      window.removeEventListener("keydown", onGesture, true);
      for (const s of shots) s.playing.stop();
      drone?.playing.stop();
      hum?.stop();
      for (const s of [...shots, ...fading]) s.panner?.disconnect();
      shots = [];
      fading = [];
      drone = null;
      hum = null;
      wanted = null;
      mixer.close();
    },
  };
}
