/**
 * Malfunctions: how a broken way out shows that it is broken.
 *
 * A way is broken when it is sealed with a failure label (`BROKEN_LABELS`:
 * an unresolved or a denied target) or when the player's travel through it
 * failed this visit (the session's `failed` map, fixture index to label).
 * A way sealed `NO ROUTE` is not a failure and stays calm.
 *
 * Each broken way has a `Fault`: a clock in 35 Hz ticks that starts a run
 * when the player comes near (`approaches`, after a seeded delay) or
 * presses E at it, plays the run's frames one per tick, then waits 2 to 5
 * s before the next run may start. A run is a list of `FaultFrame`s
 * planned in full when it starts (`planRun`), from a seed keyed by the
 * fixture's own seed and slot and the run's number, so two broken ways
 * never stutter in step and a way malfunctions the same way on every
 * visit.
 *
 * Everything here is pure and runtime only: the generator never reads it,
 * and nothing here enters `RoomSpec`. The session steps it each tick and
 * hands `faultFrames` to the renderer, which turns a frame into the
 * movers' per-draw uniforms (`render/parts.ts`). A frame is only ever
 * drawn: `DoorState`, which decides travel, never sees it, and collision
 * never reads either.
 */
import { TICK_HZ } from "../core/loop";
import { createRng, seedFor } from "../core/seed";
import { NOT_FOUND, ACCESS_DENIED } from "./generate";
import { approaches, type DoorState } from "./interact";
import type { Player } from "./move";
import type { RoomSpec, WallSlot } from "./types";

/** The kinds of way that can break. */
export type FaultKind = "door" | "hatch" | "portal";

/**
 * One tick of a run, read per kind:
 * - `open`: a door's leaves or a hatch's lid, as a fraction of the mover's
 *   travel (0 shut);
 * - `spark`: the door's sparks' gain, 0 hidden;
 * - `lamp`: the door's hazard lamp's gain, 0 for its idle glow;
 * - `scale`: the portal disc's size about its centre, 1 whole, 0 a point;
 * - `gain`: the portal disc's brightness, 1 normal;
 * - `shift`: seconds added to the swirl's clock.
 */
export interface FaultFrame {
  open: number;
  spark: number;
  lamp: number;
  scale: number;
  gain: number;
  shift: number;
}

/** A frame that changes nothing. */
export const REST_FRAME: FaultFrame = {
  open: 0,
  spark: 0,
  lamp: 0,
  scale: 1,
  gain: 1,
  shift: 0,
};

/**
 * One broken way's clock: its kind and seed, how many runs have started,
 * the current run's frames and the index into them (`frames` null while
 * idle), the ticks left before the next run may start on approach, and
 * whether a run is `armed` to start by itself (a way that just failed on
 * travel; a door waits until it has shut).
 */
export interface Fault {
  kind: FaultKind;
  seed: number;
  runs: number;
  frames: readonly FaultFrame[] | null;
  at: number;
  wait: number;
  armed: boolean;
}

/** The seal labels that mean a failed target: not found, and denied. */
export const BROKEN_LABELS: readonly string[] = [NOT_FOUND, ACCESS_DENIED];

/** Seconds per tick. */
const TICK_S = 1 / TICK_HZ;
/** The first run's seeded delay on approach, at most this many ticks. */
export const START_MAX = 10;
/** The wait between runs, in ticks: 2 to 5 s. */
export const WAIT_MIN = 2 * TICK_HZ;
export const WAIT_MAX = 5 * TICK_HZ;
/** How far a door jerks open: 30 to 50 percent of its travel. */
export const DOOR_PEAK_MIN = 0.3;
export const DOOR_PEAK_MAX = 0.5;
/** How far a shudder moves the leaves either side of the peak. */
export const SHUDDER = 0.04;
/** The hazard lamp's gain while lit, and how many ticks each blink lasts. */
export const LAMP_ON = 2.2;
export const LAMP_TICKS = 4;
/** Ticks of a door's jerk open and of its slam shut. */
export const DOOR_JERK = 3;
export const DOOR_SLAM = 2;

/** The kind of way a fixture is, or null for anything that cannot break. */
function kindOf(room: RoomSpec, index: number): FaultKind | null {
  const f = room.fixtures[index];
  return f?.kind === "door" || f?.kind === "hatch" || f?.kind === "portal"
    ? f.kind
    : null;
}

/** Whether fixture `index` is a broken way; see the module doc. */
export function isBrokenWay(
  room: RoomSpec,
  index: number,
  failed: ReadonlyMap<number, string>,
): boolean {
  const f = room.fixtures[index];
  if (f === undefined || kindOf(room, index) === null) return false;
  if (failed.has(index)) return true;
  return (
    (f.kind === "door" || f.kind === "portal") &&
    f.sealedLabel !== null &&
    BROKEN_LABELS.includes(f.sealedLabel)
  );
}

/** The indices of every broken way, in fixture order. */
export function brokenWays(
  room: RoomSpec,
  failed: ReadonlyMap<number, string>,
): number[] {
  return room.fixtures.flatMap((_, i) =>
    isBrokenWay(room, i, failed) ? [i] : [],
  );
}

/** A way's fault seed: its own seed and its slot (M11). */
export function faultSeed(slot: WallSlot, seed: number): number {
  return seedFor(seed, "fault", slot.x, slot.y, slot.side);
}

/** A fresh fault: idle, with the seeded first-approach delay. */
export function newFault(kind: FaultKind, seed: number): Fault {
  const wait = createRng(seedFor(seed, "start")).int(0, START_MAX);
  return { kind, seed, runs: 0, frames: null, at: 0, wait, armed: false };
}

/** The frames of run `run` of a way of `kind` with fault seed `seed`. */
export function planRun(
  kind: FaultKind,
  seed: number,
  run: number,
): FaultFrame[] {
  const rng = createRng(seedFor(seed, "run", run));
  const out: FaultFrame[] = [];
  const push = (p: Partial<FaultFrame>) => {
    out.push({ ...REST_FRAME, ...p });
  };
  switch (kind) {
    case "door": {
      const peak = rng.range(DOOR_PEAK_MIN, DOOR_PEAK_MAX);
      const shudder = rng.int(10, 16);
      const hold = rng.int(7, 21);
      let tick = 0;
      const lamp = () =>
        Math.floor(tick++ / LAMP_TICKS) % 2 === 0 ? LAMP_ON : 0;
      for (let t = 1; t <= DOOR_JERK; t++) {
        push({ open: (peak * t) / DOOR_JERK, lamp: lamp() });
      }
      for (let t = 0; t < shudder; t++) {
        push({
          open: Math.max(0, peak + rng.range(-SHUDDER, SHUDDER)),
          spark: rng.chance(0.55) ? rng.range(1.5, 3) : 0,
          lamp: lamp(),
        });
      }
      for (let t = 0; t < hold; t++) push({ open: peak, lamp: lamp() });
      for (let t = DOOR_SLAM - 1; t >= 0; t--) {
        push({
          open: (peak * t) / DOOR_SLAM,
          spark: t === 0 ? rng.range(2, 3) : 0,
          lamp: lamp(),
        });
      }
      return out;
    }
    case "hatch": {
      push({ open: 0.5 });
      push({ open: 1 });
      const rattle = rng.int(8, 14);
      for (let t = 0; t < rattle; t++) push({ open: rng.range(0.55, 1) });
      push({ open: 0.5 });
      push({ open: 0 });
      return out;
    }
    case "portal": {
      const stutter = rng.int(12, 20);
      let shift = 0;
      for (let t = 0; t < stutter; t++) {
        shift += rng.chance(0.5) ? -TICK_S : rng.range(0.05, 0.3);
        push({ shift, gain: rng.range(0.25, 1.6) });
      }
      for (let t = 1; t <= 6; t++) {
        push({ shift, gain: 1.8, scale: 1 - (t / 6) ** 2 });
      }
      const dark = rng.int(10, 20);
      for (let t = 0; t < dark; t++) push({ scale: 0 });
      for (let t = 1; t <= 8; t++) {
        push({ scale: t / 8, gain: 0.4 + (0.6 * t) / 8 });
      }
      return out;
    }
  }
}

/**
 * The faults one tick later: one per broken way (`brokenWays`), a new one
 * for a way that just broke, none for a way that is no longer broken.
 *
 * A running fault moves to its next frame and goes idle after its last,
 * with a seeded wait of `WAIT_MIN` to `WAIT_MAX` ticks. An idle fault
 * counts its wait down and starts a run when it is armed (a door only once
 * its `DoorState` has shut), when E was pressed at it this tick
 * (`pressed`), or when the player `approaches` it and the wait is over.
 * Returns a new map; `faults` is left as it was.
 */
export function stepFaults(
  room: RoomSpec,
  player: Player,
  faults: ReadonlyMap<number, Fault>,
  failed: ReadonlyMap<number, string>,
  pressed: number | null,
  doors: ReadonlyMap<number, DoorState>,
): Map<number, Fault> {
  const out = new Map<number, Fault>();
  for (const i of brokenWays(room, failed)) {
    const fx = room.fixtures[i];
    const kind = kindOf(room, i);
    if (fx === undefined || fx.kind === "placard" || kind === null) continue;
    const was = faults.get(i) ?? newFault(kind, faultSeed(fx.slot, fx.seed));
    const f: Fault = { ...was };
    if (f.frames !== null) {
      f.at++;
      if (f.at >= f.frames.length) {
        f.frames = null;
        f.at = 0;
        f.wait = createRng(seedFor(f.seed, "wait", f.runs)).int(
          WAIT_MIN,
          WAIT_MAX,
        );
      }
    } else {
      if (f.wait > 0) f.wait--;
      const shut = kind !== "door" || (doors.get(i)?.open ?? 0) === 0;
      const start = f.armed
        ? shut
        : pressed === i || (f.wait === 0 && approaches(fx.slot, player));
      if (start) {
        f.frames = planRun(kind, f.seed, f.runs);
        f.runs++;
        f.at = 0;
        f.armed = false;
      }
    }
    out.set(i, f);
  }
  return out;
}

/**
 * The faults with fixture `index` armed: it just failed on travel, so it
 * runs once by itself (a door once it has shut). The session calls it
 * right after adding `index` to `failed`.
 */
export function armFault(
  room: RoomSpec,
  index: number,
  faults: ReadonlyMap<number, Fault>,
): Map<number, Fault> {
  const out = new Map(faults);
  const fx = room.fixtures[index];
  const kind = kindOf(room, index);
  if (fx === undefined || fx.kind === "placard" || kind === null) return out;
  const f = out.get(index) ?? newFault(kind, faultSeed(fx.slot, fx.seed));
  out.set(index, { ...f, armed: true, wait: 0 });
  return out;
}

/** The current frame of every running fault, by fixture index. */
export function faultFrames(
  faults: ReadonlyMap<number, Fault>,
): Map<number, FaultFrame> {
  const out = new Map<number, FaultFrame>();
  for (const [i, f] of faults) {
    const frame = f.frames?.[f.at];
    if (frame !== undefined) out.set(i, frame);
  }
  return out;
}
