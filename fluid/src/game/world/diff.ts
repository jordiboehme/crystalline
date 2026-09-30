/**
 * The runtime seams a live change runs on a room already built (M4 C16,
 * C17, C19). Session side: nothing on the generator side imports this
 * module, and nothing here reaches a golden, since every function works on
 * a `RoomSpec` after generation.
 *
 * `diffRooms` tells the room on screen from the room the same place builds
 * now: `"same"` when the two serialise equal, `"text"` when they serialise
 * equal once every text field is blanked (a terminal's heading and lines, a
 * door's, portal's, hatch's and exit's label, a poster's, placard's and
 * screen's lines, a lift's stop labels and note, the room's title), and
 * `"shape"` otherwise. Seeds are never blanked, so a renamed heading, whose
 * terminal seed follows it, is a shape change.
 *
 * `settleSpot` is where the player stands after a reshape: where they
 * stood when that point is on a floor cell and inside no blocker, else the
 * centre of the nearest floor cell whose centre is inside no blocker.
 *
 * `darkened` is the room whose engram went (a 404 or a 403 on a re-check):
 * the same room with every light zone at a quarter of its level and
 * failing, and a hatch to the domain's bridge on the first wall edge in
 * `wallSlots` order that `freeEdgeTest` (`bridge.ts`) passes and whose
 * arrival point (`ARRIVAL_DISTANCE` in front of the wall) lies in no
 * blocker; the props, curios and decals are then re-dressed round it as
 * the bridge's own fittings are (`redress`). The edges are every wall edge
 * (`wallRuns`, which walks the walls in the very order `wallSlots` does),
 * not only the slots: a generated room's dressing hangs a wall prop on
 * every slot no fixture took, so the free edges lie between them, as the
 * bridge's screen finds them. A room with no such edge only goes dark: its
 * exit door and lifts still lead out.
 *
 * `dipFactor` is the light curve of a text change's flicker and a shape
 * change's power dip, a factor on every light level.
 *
 * This module may import `bridge.ts`, `interact.ts` and `move.ts`; never
 * `reachChecks.ts`, which holds test helpers only.
 */

import { seedFor } from "../core/seed";
import { MANIFEST_PERMALINK } from "../paths";
import { freeEdgeTest, redress } from "./bridge";
import { ARRIVAL_DISTANCE, wallPoint } from "./interact";
import { isFloor, wallRuns } from "./layout";
import { LIFT_WORDS } from "./lifts";
import { blockersFor } from "./move";
import { NO_NEAR } from "./sites";
import type { Box, Fixture, RoomSpec, WallSlot } from "./types";
import { CELL } from "./units";

/**
 * How two builds of one place differ (M4 C16): not at all, in their text
 * only, or in anything else.
 */
export type RoomDiff = "same" | "text" | "shape";

/** How long a text change's flicker lasts, in milliseconds (M4 C17). */
export const FLICKER_MS = 400;

/** How long a shape change's power dip lasts, in milliseconds (M4 C17). */
export const DIP_MS = 1000;

/**
 * When in the dip the new room is entered, in milliseconds (M4 C17): in
 * the middle of the dark stretch, so the swap is never seen lit.
 */
export const DIP_SWAP_MS = 500;

/** When the dip reaches its low point, in milliseconds. */
const DIP_DARK_MS = 400;

/** When the dip starts to rise again, in milliseconds. */
const DIP_RISE_MS = 600;

/** The light factor at the dip's low point: 10 %. */
const DIP_LOW = 0.1;

/** How long one step of the flicker lasts, in milliseconds. */
const FLICKER_STEP_MS = 50;

/** The flicker's light factor per step, eight steps over `FLICKER_MS`. */
const FLICKER_PATTERN: readonly number[] = [0.55, 1, 0.7, 1, 0.6, 0.9, 0.75, 1];

/** A deep copy of the fixture with every text field blanked (M4 C16). */
function blankFixture(f: Fixture): Fixture {
  switch (f.kind) {
    case "terminal":
      return { ...f, heading: "", lines: [] };
    case "door":
    case "portal":
    case "hatch":
    case "exit":
      return { ...f, label: "" };
    case "poster":
    case "placard":
    case "screen":
      return { ...f, lines: [] };
    case "lift":
      return {
        ...f,
        stops: f.stops.map((s) => ({ ...s, label: "" })),
        note: null,
      };
    case "machine":
      return f;
  }
}

/** The room with every text field blanked (M4 C16); seeds are kept. */
function blanked(room: RoomSpec): RoomSpec {
  return { ...room, title: "", fixtures: room.fixtures.map(blankFixture) };
}

/**
 * How `b` differs from `a` (M4 C16): `"same"` when they serialise equal,
 * `"text"` when they do once every text field is blanked, else `"shape"`.
 * See the module doc for the fields.
 */
export function diffRooms(a: RoomSpec, b: RoomSpec): RoomDiff {
  if (JSON.stringify(a) === JSON.stringify(b)) return "same";
  if (JSON.stringify(blanked(a)) === JSON.stringify(blanked(b))) return "text";
  return "shape";
}

/** True when the point lies inside the box, its edges included. */
function inBox(x: number, z: number, b: Box): boolean {
  return x >= b.x0 && x <= b.x1 && z >= b.z0 && z <= b.z1;
}

/** True when the point lies inside none of the boxes. */
function clearOf(x: number, z: number, blockers: readonly Box[]): boolean {
  return !blockers.some((b) => inBox(x, z, b));
}

/**
 * Where the player stands after a reshape (M4 C17): `(x, z)` itself when
 * the cell under it is floor and the point is inside no box of
 * `blockersFor(room)`; else the centre of the floor cell, its centre
 * inside no blocker, nearest `(x, z)`, ties broken by row, then column.
 * A room with no such cell at all gives its spawn cell's centre.
 */
export function settleSpot(
  room: RoomSpec,
  x: number,
  z: number,
): { x: number; z: number } {
  const blockers = blockersFor(room);
  if (
    isFloor(room.grid, Math.floor(x / CELL), Math.floor(z / CELL)) &&
    clearOf(x, z, blockers)
  )
    return { x, z };
  let best: { x: number; z: number } | null = null;
  let bestDist = Infinity;
  room.grid.forEach((row, cy) => {
    for (let cx = 0; cx < row.length; cx++) {
      if (!isFloor(room.grid, cx, cy)) continue;
      const c = { x: (cx + 0.5) * CELL, z: (cy + 0.5) * CELL };
      if (!clearOf(c.x, c.z, blockers)) continue;
      const dist = (c.x - x) ** 2 + (c.z - z) ** 2;
      // Rows walk top down and cells west to east, so a strict `<` keeps
      // the first of a tie: the lower row, then the lower column.
      if (dist < bestDist) {
        bestDist = dist;
        best = c;
      }
    }
  });
  return (
    best ?? {
      x: (room.spawn.x + 0.5) * CELL,
      z: (room.spawn.y + 0.5) * CELL,
    }
  );
}

/** True when the arrival point in front of the slot is inside no blocker. */
function arrivalClear(slot: WallSlot, blockers: readonly Box[]): boolean {
  const w = wallPoint(slot);
  return clearOf(
    w.x + w.inward[0] * ARRIVAL_DISTANCE,
    w.z + w.inward[1] * ARRIVAL_DISTANCE,
    blockers,
  );
}

/**
 * The room gone dark in place, with a hatch to the bridge (M4 C19): a new
 * room, `room` itself untouched. Every light zone at `round(level / 4)`,
 * `failing`; the hatch labelled `LIFT_WORDS.bridge`, addressed to the
 * MANIFEST of `domain` (the domain's bridge), seeded
 * `seedFor(room.seed, "dark-hatch")`, on the first free wall edge, then the
 * room re-dressed round it. See the module doc.
 */
export function darkened(room: RoomSpec, domain: string): RoomSpec {
  const dark: RoomSpec = structuredClone(room);
  dark.lights = dark.lights.map((zone) => ({
    ...zone,
    level: Math.round(zone.level / 4),
    special: "failing",
  }));
  const free = freeEdgeTest(room);
  const blockers = blockersFor(room);
  const slot = wallRuns(room.grid)
    .flat()
    .find((s) => free(s) && arrivalClear(s, blockers));
  if (slot === undefined) return dark;
  const hatch: Fixture = {
    kind: "hatch",
    slot,
    label: LIFT_WORDS.bridge,
    address: { domain, permalink: MANIFEST_PERMALINK },
    seed: seedFor(room.seed, "dark-hatch"),
  };
  return redress({ ...dark, fixtures: [...dark.fixtures, hatch] }, NO_NEAR);
}

/**
 * The light factor at `ms` into a flicker or a dip (M4 C17), 1 outside
 * either. The dip falls in a straight line from 1 at 0 ms to 0.1 at 400
 * ms, holds 0.1 until 600 ms (the swap at `DIP_SWAP_MS` falls there) and
 * rises in a straight line back to 1 at `DIP_MS`. The flicker steps
 * through a fixed pattern every 50 ms for `FLICKER_MS`.
 */
export function dipFactor(kind: "flicker" | "dip", ms: number): number {
  if (kind === "flicker") {
    if (ms < 0 || ms >= FLICKER_MS) return 1;
    return FLICKER_PATTERN[Math.floor(ms / FLICKER_STEP_MS)] ?? 1;
  }
  if (ms <= 0 || ms >= DIP_MS) return 1;
  if (ms < DIP_DARK_MS) return 1 - ((1 - DIP_LOW) * ms) / DIP_DARK_MS;
  if (ms <= DIP_RISE_MS) return DIP_LOW;
  return (
    DIP_LOW + ((1 - DIP_LOW) * (ms - DIP_RISE_MS)) / (DIP_MS - DIP_RISE_MS)
  );
}
