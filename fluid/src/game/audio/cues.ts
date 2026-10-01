/**
 * The cues: what happens in the station, named for the ear (M4 C22, C23).
 *
 * The session sends a `Cue` into a `SoundSink` for every moment that makes
 * a sound, as it writes text into a `HudSink`, and knows nothing of how a
 * cue sounds. The sink (the director, `audio/director.ts`) turns each cue
 * into patches, so a sound pass can swap any patch without touching the
 * session. A cue names the event, never a sound: a door says its kind and
 * whether it opened, a fault its way and which run it is (F15), a room
 * its ambience and seed.
 *
 * This module is the pure side: no WebAudio and no React. It places a cue
 * in the stereo field (`placeCue`), counts footsteps from the stride phase
 * (`stepsBetween`), turns the door targets and the faults of two ticks into
 * cues (`doorCues`, `faultCues`) and names a room's ambience (`ambienceOf`).
 */

import { rightOf } from "../gl/math";
import { boxFront, boxKey } from "../world/box";
import { doorKey, wallPoint } from "../world/interact";
import type { Fault } from "../world/malfunction";
import type { Player } from "../world/move";
import type { RoomSpec } from "../world/types";

/**
 * What a door-like thing sounds as: a door by its style, a room's exit
 * and a police box's doors. A hatch and a lift send no door cue: a
 * hatch's lid has no door state (Space at it travels at once, so its pop
 * is the `travel` cue `hatch`), and a lift's doors only ever head shut
 * (C27), so their opening and closing are the `ride` cues' `depart` and
 * `arrive`.
 */
export type DoorSound = "sliding" | "bulkhead" | "blast" | "exit" | "box";

/**
 * The drone a room loops (C22): an engram room's by its condition, the
 * airlock's, a hangar's, the console room's, and the dark room's (C19).
 */
export type Ambience =
  | "clean"
  | "construction"
  | "dim"
  | "derelict"
  | "airlock"
  | "hangar"
  | "console"
  | "dark";

/**
 * One moment that makes a sound.
 *
 * - `step`: one footstep; `foot` alternates, `run` while the run key is
 *   held, `n` counts the session's steps (the seed of its pitch).
 * - `door`: a door-like thing turned to open or to shut, placed.
 * - `travel`: a way taken, by what it went through.
 * - `fault`: a broken way started a run, which one it is (`run`), placed.
 * - `terminal`: a screen came up (the reader, the level select, the lift's
 *   stops) or a room's text changed under the player.
 * - `ride`: a lift ride left (`depart`) or ended (`arrive`).
 * - `box`: the police box took off (the cut into the console room) or
 *   landed (the step out onto a bridge).
 * - `jump`: the level select's jump.
 * - `answer`: a terminal answered (Task 9), placed.
 * - `room`: a room was entered, with its ambience and seed.
 */
export type Cue =
  | { kind: "step"; foot: 0 | 1; run: boolean; n: number }
  | {
      kind: "door";
      sound: DoorSound;
      open: boolean;
      pan: number;
      gain: number;
    }
  | { kind: "travel"; via: "door" | "portal" | "hatch" | "exit" | "lift" }
  | {
      kind: "fault";
      way: "door" | "hatch" | "portal";
      run: number;
      pan: number;
      gain: number;
    }
  | { kind: "terminal" }
  | { kind: "ride"; phase: "depart" | "arrive" }
  | { kind: "box"; phase: "takeoff" | "landing" }
  | { kind: "jump" }
  | { kind: "answer"; pan: number; gain: number }
  | { kind: "room"; ambience: Ambience; seed: number };

/** Where the session sends its sounds, as it sends text to a `HudSink`. */
export interface SoundSink {
  cue(cue: Cue): void;
  /** Toggles mute and answers the new state. */
  toggleMute(): boolean;
  /** Toggles the ambience bus alone and answers the new state (true = off). */
  toggleAmbience(): boolean;
}

/**
 * The distance, in metres, at which a placed cue has fallen to half its
 * gain: `placeCue`'s gain is `1 / (1 + d / CUE_HALF_M)`, 1 at the player
 * and never below 0.
 */
export const CUE_HALF_M = 6;

/**
 * The ambience of `room` (C22): the dark room's when `dark` (C19), else a
 * station space's by its `space` (a deck by its condition), else the
 * console room's for a hand-built room with fittings and no `space`, else
 * the room's condition. `space` is read before `interior`, since the
 * airlock carries both.
 */
export function ambienceOf(room: RoomSpec, dark: boolean): Ambience {
  if (dark) return "dark";
  switch (room.space) {
    case "airlock":
      return "airlock";
    case "hangar":
      return "hangar";
    case "deck":
      return room.condition;
    case undefined:
      break;
  }
  if (room.interior !== undefined) return "console";
  return room.condition;
}

/**
 * Stereo pan (-1 left .. 1 right of the facing) and a gain by distance
 * (M4 C23). The pan is the sine of the bearing from the facing, so a point
 * straight ahead or behind sits in the middle; a point at the player sits
 * there at full gain.
 */
export function placeCue(
  player: Player,
  x: number,
  z: number,
): { pan: number; gain: number } {
  const dx = x - player.x;
  const dz = z - player.z;
  const d = Math.hypot(dx, dz);
  if (d === 0) return { pan: 0, gain: 1 };
  const [rx, rz] = rightOf(player.yaw);
  const pan = Math.max(-1, Math.min(1, (dx * rx + dz * rz) / d));
  return { pan, gain: 1 / (1 + d / CUE_HALF_M) };
}

/**
 * The footsteps between two bob phases: one per half cycle, a step each
 * time the phase passes a multiple of pi.
 */
export function stepsBetween(prevBob: number, bob: number): number {
  return Math.max(0, Math.floor(bob / Math.PI) - Math.floor(prevBob / Math.PI));
}

/** A door-like fixture's sound, or null for a fixture with no doors. */
function fixtureSound(f: RoomSpec["fixtures"][number]): DoorSound | null {
  switch (f.kind) {
    case "door":
      return f.style;
    case "exit":
      return f.kind;
    default:
      return null;
  }
}

/**
 * Open and close cues for every door whose target changed: the fixtures'
 * doors under `doorKey`, the police boxes' under `boxKey`, each read as
 * shut when a map has no entry for it. Each is placed at its wall point
 * (a box at its front).
 */
export function doorCues(
  room: RoomSpec,
  player: Player,
  before: ReadonlyMap<string, number>,
  after: ReadonlyMap<string, number>,
): Cue[] {
  const out: Cue[] = [];
  const check = (key: string, sound: DoorSound, x: number, z: number) => {
    const was = before.get(key) ?? 0;
    const now = after.get(key) ?? 0;
    if (was === now) return;
    out.push({ kind: "door", sound, open: now > 0, ...placeCue(player, x, z) });
  };
  room.fixtures.forEach((f, i) => {
    const sound = fixtureSound(f);
    if (sound === null) return;
    const w = wallPoint(f.slot);
    check(doorKey(i), sound, w.x, w.z);
  });
  room.heroes.forEach((h, i) => {
    if (h.kind !== "police-box") return;
    const front = boxFront(h);
    check(boxKey(i), "box", front.x, front.z);
  });
  return out;
}

/**
 * A fault cue for every way whose fault started a new run (its `runs`
 * went up; a way with no fault before counts from 0), carrying the run's
 * count (F15), placed at the way's wall point.
 */
export function faultCues(
  room: RoomSpec,
  player: Player,
  before: ReadonlyMap<number, Fault>,
  after: ReadonlyMap<number, Fault>,
): Cue[] {
  const out: Cue[] = [];
  for (const [i, f] of after) {
    if (f.runs <= (before.get(i)?.runs ?? 0)) continue;
    const fixture = room.fixtures[i];
    if (fixture === undefined) continue;
    const w = wallPoint(fixture.slot);
    out.push({
      kind: "fault",
      way: f.kind,
      run: f.runs,
      ...placeCue(player, w.x, w.z),
    });
  }
  return out;
}
