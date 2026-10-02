/**
 * The director: the sound sink the session sends its cues to, playing
 * each through the mixer (M4 C20 to C23, C28).
 *
 * The session names moments (`audio/cues.ts`); the director picks their
 * patches (`audio/effects.ts`, `audio/ambience.ts`) and plays them on a bus
 * of the mixer (`audio/mixer.ts`), every effect on `effects`, the drone
 * on `ambience` and the signature sounds (`audio/signature.ts`) on
 * `signature`:
 *
 * - **One-shots.** A step, a door, a portal's or a hatch's travel, a
 *   fault, a terminal; and on the signature bus the police box's wheeze
 *   (`box`, its take-off or its landing) and the five tones (`jump`, and
 *   `answer`, placed like a door; M4 C24, C25). A placed cue (a door, a
 *   fault, an answer) plays through a stereo panner at its pan into its
 *   bus, its voices scaled by its gain. The tones never overlap: an
 *   answer while they sound is dropped, a jump cuts them short. At most
 *   `VOICE_CAP` patches play at once, the drone and the ride's hum
 *   counted; one more drops the oldest step or fault first (a 30 ms fade),
 *   and only when none is left the oldest other one-shot. The drone and the
 *   hum are never dropped, and starting either makes room the same way. A
 *   placed shot's panner is disconnected as soon as the shot has ended.
 *   Noise plays from one seeded buffer per bus (the synth keeps a buffer
 *   per seed name, and a name per patch would keep fifteen two-second
 *   buffers for the context's life). While the context is not running the
 *   one-shots are dropped: its clock stands still, and everything
 *   scheduled on it would sound at once when it runs again.
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
 * - **The modem.** `dial` plays the launch's dial-in (`audio/modem.ts`,
 *   M4 C26) on the `modem` bus and answers its stop, which fades it out
 *   over `DIAL_FADE_S`; `carrier` plays the hang-up on a drop and a short
 *   handshake on a return (C27), also on the `modem` bus. Both are
 *   one-shots under the cap, and a new dial cuts one still sounding. A
 *   dial asked for while the context is still settling a resume waits
 *   for it to run (StrictMode's second mount, F29) rather than being
 *   dropped like another one-shot.
 * - **Mute** is the master's alone (`toggleMute`, the mixer's): the cues
 *   play on into a silent master, so sound coming back mid-room brings the
 *   drone back at once. The ambience switch (`toggleAmbience`) is the
 *   ambience bus's alone: the drones keep running into a silent bus, the
 *   effects, signature cues and modem stay audible.
 * - **Gestures.** A click or a key on the window while the context is not
 *   running unlocks it (Safari may refuse the resume after a hidden tab,
 *   F29), inside that gesture; the first one also makes the context on a
 *   reload. Not between the host's `suspend` and `resume`: a click on the
 *   pause screen leaves the sound off.
 *
 * `suspend` and `resume` are the host's (the pause and a hidden tab);
 * `dispose` stops everything, takes the listeners down and closes the
 * mixer.
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
import { handshakePatch, hangupPatch, reconnectPatch } from "./modem";
import type { Patch } from "./patch";
import { landingPatch, takeoffPatch, tonesPatch } from "./signature";
import { playPatch, type PlayingPatch } from "./synth";

/** The most patches that play at once, the drone and the ride's hum counted. */
export const VOICE_CAP = 24;

/** How fast the ride's hum falls silent when the ride arrives. */
export const RIDE_FADE_S = 0.04;

/** How fast the dial-in falls silent when its screen is skipped or done. */
export const DIAL_FADE_S = 0.04;

/** The game's sound sink, with the lifecycle its host drives. */
export interface Director extends SoundSink {
  /** Dials `number` on the modem bus (M4 C26); answers its stop. */
  dial(number: string): () => void;
  /** The change stream went down or came back (M4 C27). */
  carrier(up: boolean): void;
  /** Suspends the context: the pause, a hidden tab. */
  suspend(): void;
  /** Resumes the context after a suspend. */
  resume(): void;
  /** From a gesture: unlocks the context, starting the room's drone if owed. */
  unlock(): void;
  /** Stops every sound, drops the listeners and closes the mixer. */
  dispose(): void;
}

/** A one-shot that is playing. */
interface Shot {
  playing: PlayingPatch;
  /** Steps and faults are the first dropped at the cap. */
  cheap: boolean;
}

/** How a one-shot is played. */
interface ShotOptions {
  pan?: number;
  gain?: number;
  cheap?: boolean;
  /** The bus it plays on, `effects` unless given. */
  bus?: Bus;
  /** Called once the shot has ended (or been stopped and faded). */
  onEnd?: () => void;
}

/** The director over `mixer`. */
export function createDirector(mixer: Mixer): Director {
  let disposed = false;
  /** The one-shots still playing, oldest first. */
  let shots: Shot[] = [];
  /** The panners of placed shots not yet ended. */
  const panners = new Set<AudioNodeLike>();
  /** The room's ambience and seed, kept when there is no context. */
  let wanted: { ambience: Ambience; seed: number } | null = null;
  let drone: { ambience: Ambience; playing: PlayingPatch } | null = null;
  let hum: PlayingPatch | null = null;
  /** The five tones while they sound (a jump's or an answer's). */
  let tones: PlayingPatch | null = null;
  /** The dial-in while it sounds. */
  let dialing: PlayingPatch | null = null;
  /** Whether a ride arrived and its chime waits for the room it lands in. */
  let chimeOwed = false;
  /** Whether the host suspended the sound (the pause, a hidden tab). */
  let quiet = false;

  const held = () => (drone === null ? 0 : 1) + (hum === null ? 0 : 1);

  /** Drops the oldest step or fault, else the oldest one-shot. */
  const evict = () => {
    const i = shots.findIndex((s) => s.cheap);
    const [gone] = shots.splice(i < 0 ? 0 : i, 1);
    gone?.playing.stop();
  };

  /**
   * Makes room under `VOICE_CAP` for one more patch: forgets the one-shots
   * whose time has passed, then drops one-shots until the next one fits.
   */
  const makeRoom = () => {
    const now = mixer.ctx?.currentTime ?? 0;
    shots = shots.filter((s) => s.playing.end > now);
    while (shots.length > 0 && shots.length + held() + 1 > VOICE_CAP) evict();
  };

  /**
   * Plays `patch` on `bus` from now: through a panner when placed (taken
   * down when the patch ends), its voices scaled by `gain`, its noise from
   * the bus's buffer. Null without a context or bus.
   */
  const start = (
    patch: Patch,
    bus: Bus,
    { pan, gain = 1, onEnd: ended }: ShotOptions = {},
  ): PlayingPatch | null => {
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
    if (panner !== null) panners.add(panner);
    const onEnd =
      panner === null && ended === undefined
        ? undefined
        : () => {
            // `dispose` took every panner down and let go of every shot.
            if (disposed) return;
            if (panner !== null) {
              panner.disconnect();
              panners.delete(panner);
            }
            ended?.();
          };
    return playPatch(ctx, dest, played, ctx.currentTime, bus, onEnd);
  };

  /**
   * A one-shot on its bus (the effects unless given), under the cap;
   * dropped while not running.
   */
  const shot = (
    patch: Patch,
    options: ShotOptions = {},
  ): PlayingPatch | null => {
    const ctx = mixer.ctx;
    if (disposed || ctx === null || !mixer.running) return null;
    if (options.gain !== undefined && !(options.gain > 0)) return null;
    makeRoom();
    const playing = start(patch, options.bus ?? "effects", options);
    if (playing === null) return null;
    shots.push({ playing, cheap: options.cheap === true });
    return playing;
  };

  /**
   * The five tones, for a jump or an answer (M4 C24, C25). An answer while
   * tones still sound is dropped (a console left and come straight back
   * to, a second console in the room), so the motif never overlaps
   * itself; a jump cuts sounding tones short (`evict`'s fade) and plays its
   * own.
   */
  const playTones = (options: ShotOptions, jump: boolean) => {
    if (tones !== null) {
      if (!jump) return;
      tones.stop();
      tones = null;
    }
    const playing: PlayingPatch | null = shot(tonesPatch(), {
      ...options,
      bus: "signature",
      onEnd: () => {
        if (tones === playing) tones = null;
      },
    });
    tones = playing;
  };

  /** Starts the wanted drone when there is none and a context to play it. */
  const startDrone = () => {
    if (disposed || wanted === null || drone !== null) return;
    if (mixer.ctx === null) return;
    makeRoom();
    const playing = start(dronePatch(wanted.ambience, wanted.seed), "ambience");
    if (playing !== null) drone = { ambience: wanted.ambience, playing };
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
    makeRoom();
    hum = start(ridePatch("depart"), "effects");
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
        shot(c.phase === "takeoff" ? takeoffPatch() : landingPatch(), {
          bus: "signature",
        });
        return;
      case "jump":
        playTones({}, true);
        return;
      case "answer":
        playTones({ pan: c.pan, gain: c.gain }, false);
        return;
    }
  };

  const unlock = () => {
    if (disposed) return;
    mixer.unlock();
    startDrone();
  };

  // Not while the host keeps it quiet: a click on the pause screen must
  // not bring the sound back under it.
  const onGesture = () => {
    if (!quiet && !mixer.running) unlock();
  };
  window.addEventListener("click", onGesture, true);
  window.addEventListener("keydown", onGesture, true);

  /** How many dials were asked for: a later one replaces a waiting one. */
  let calls = 0;
  const dial = (number: string): (() => void) => {
    dialing?.stop(DIAL_FADE_S);
    dialing = null;
    const call = ++calls;
    let hungUp = false;
    let playing: PlayingPatch | null = null;
    const play = () => {
      if (hungUp || disposed || call !== calls) return;
      const shotPlaying: PlayingPatch | null = shot(handshakePatch(number), {
        bus: "modem",
        onEnd: () => {
          if (dialing === shotPlaying) dialing = null;
        },
      });
      playing = shotPlaying;
      dialing = shotPlaying;
    };
    // A context still settling a resume (StrictMode's second mount, whose
    // context the first mount's cleanup suspended, F29): the dial waits
    // for it to run instead of being dropped as a one-shot.
    const ctx = mixer.ctx;
    if (!mixer.running && !quiet && ctx !== null && ctx.state === "suspended")
      void ctx.resume().then(play, () => undefined);
    else play();
    return () => {
      hungUp = true;
      playing?.stop(DIAL_FADE_S);
    };
  };

  return {
    cue,
    dial,
    carrier: (up) => {
      if (disposed) return;
      shot(up ? reconnectPatch() : hangupPatch(), { bus: "modem" });
    },
    toggleMute: () => mixer.toggleMute(),
    toggleAmbience: () => mixer.toggleAmbience(),
    suspend: () => {
      if (disposed) return;
      quiet = true;
      mixer.suspend();
    },
    resume: () => {
      if (disposed) return;
      quiet = false;
      mixer.resume();
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
      for (const panner of panners) panner.disconnect();
      panners.clear();
      shots = [];
      drone = null;
      hum = null;
      tones = null;
      dialing = null;
      wanted = null;
      mixer.close();
    },
  };
}
