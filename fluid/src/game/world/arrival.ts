/**
 * The arrival box (2.6e C14): a walk out of the console room lands the
 * player on another domain's bridge, stepping out of a police box that
 * stands beside the bridge's entrance for that one visit.
 *
 * The box stands on the hall's south wall, backed against it and facing
 * into the hall (turn 0), in the first of `arrivalBoxCandidates` where it
 * fits. The candidates are the hall's south-row cells in order of their
 * distance from the entrance cell, east before west at equal distance
 * (`x+1`, `x+2`, `x-2`, `x+3`, `x-3`, ...), leaving out the entrance and
 * the placard cells, every cell whose south edge holds a fixture and any
 * cell whose south edge is not a wall. `placeArrivalBox` takes a
 * candidate when the box's floor (`heroFootprint`) fits the hall
 * (`fitsFloor`), enters no lane of `dressingSites(room)` and overlaps
 * none of its taken boxes (the fixtures' footprints, the furniture and
 * the scaffold frames), and when the player's circle at the box's use
 * point (`heroUsePoint`, 1.75 m out) is on floor and clear of the same
 * taken boxes. It reads nothing of the room's heroes, props or curios:
 * those move round the box, not the box round them.
 *
 * `withArrivalBox` then drops the room's own heroes whose floor overlaps
 * the box's grown by `HERO_CLEAR` (two heroes never stand inside each
 * other's moat), keeps the rest, and re-dresses the room round the box
 * and the kept heroes through the shared forced-hero seam (`withHeroes`
 * in `generate.ts`, C15), so props and curios keep off the box as they
 * keep off any hero the generator drew. The rule that keeps a backed hero
 * off the edges beside a way is not applied: the box is 1.3 m wide in a
 * 2 m cell, so a hatch beside it keeps its lane and its arrival spot. The
 * player's spawn is the box's use point, facing the way the box faces.
 * When no candidate fits there is no box: the room comes back as it was
 * handed in, the spawn is null and the player arrives at the entrance, as
 * on any other visit.
 *
 * This is the session side (C15): nothing on the generator side imports
 * it, so a bridge entered any other way (a door, a reload, the level
 * select) is generated plain, and the box lives only in the room object
 * the session holds for that visit. It imports `generate.ts`,
 * `heroes.ts`, `footprints.ts`, `sites.ts` and `move.ts` (for the
 * player's radius).
 */

import { seedFor } from "../core/seed";
import { HERO_FRONT, heroFootprint } from "./footprints";
import { withHeroes } from "./generate";
import { HERO_CLEAR, heroUsePoint } from "./heroes";
import { PLAYER_RADIUS } from "./move";
import {
  dressingSites,
  edgeKey,
  fitsFloor,
  grow,
  overlaps,
  wallAnchor,
} from "./sites";
import type { Box, Hero, PlaceInput, RoomSpec, WallSlot } from "./types";

/**
 * The hall's south-row edges the arrival box may stand on, best first
 * (C14): every south wall edge of the hall's last row, less the entrance's
 * and the placard's (the placard sits one cell west of the entrance) and
 * every edge a fixture holds, sorted by distance from the entrance's
 * column, the east one first where two lie equally far. An edge that is
 * not a wall of the grid (none on a generated hall's south row, which
 * only the entrance opens) is left out too.
 */
export function arrivalBoxCandidates(room: RoomSpec): WallSlot[] {
  const e = room.entrance;
  const held = new Set(room.fixtures.map((f) => edgeKey(f.slot)));
  const walls = new Set(dressingSites(room).runs.flat().map(edgeKey));
  const y = room.hall.y1 - 1;
  const out: WallSlot[] = [];
  for (let x = room.hall.x0; x < room.hall.x1; x++) {
    if (x === e.x || x === e.x - 1) continue;
    const edge: WallSlot = { x, y, side: "s" };
    const key = edgeKey(edge);
    if (held.has(key) || !walls.has(key)) continue;
    out.push(edge);
  }
  const far = (c: WallSlot) => Math.abs(c.x - e.x);
  return out.sort((a, b) => far(a) - far(b) || b.x - a.x);
}

/** The box a circle of the player's radius at `(x, z)` fits inside, in metres. */
function circleBounds(x: number, z: number): Box {
  return {
    x0: x - PLAYER_RADIUS,
    x1: x + PLAYER_RADIUS,
    z0: z - PLAYER_RADIUS,
    z1: z + PLAYER_RADIUS,
  };
}

/** True when a circle of the player's radius at `(x, z)` overlaps `b`: `move.ts`'s collision test. */
function circleHits(x: number, z: number, b: Box): boolean {
  const nx = Math.max(b.x0, Math.min(x, b.x1));
  const nz = Math.max(b.z0, Math.min(z, b.z1));
  const dx = x - nx;
  const dz = z - nz;
  return dx * dx + dz * dz < PLAYER_RADIUS * PLAYER_RADIUS;
}

/**
 * The arrival box for `room`, or null when none fits (C14): a police box,
 * variant 0, seed `seedFor(room.seed, "arrival-box")`, anchored at the
 * wall point (`wallAnchor`) of the first of `arrivalBoxCandidates(room)`
 * whose box fits the hall's floor, enters no lane, overlaps no taken box
 * (fixture footprints, furniture, scaffold) and leaves the player's
 * circle at its use point on floor and clear of the same taken boxes.
 * The room's heroes, props and curios are not read.
 */
export function placeArrivalBox(room: RoomSpec): Hero | null {
  const sites = dressingSites(room);
  const seed = seedFor(room.seed, "arrival-box");
  for (const c of arrivalBoxCandidates(room)) {
    const { x, y, turn } = wallAnchor(c);
    const box: Hero = { kind: "police-box", variant: 0, x, y, turn, seed };
    const floor = heroFootprint(box);
    if (!fitsFloor(room, floor)) continue;
    if (sites.lanes.some((l) => overlaps(l, floor))) continue;
    if (sites.taken.some((t) => overlaps(t, floor))) continue;
    const use = heroUsePoint(box);
    if (use === null) continue;
    if (!fitsFloor(room, circleBounds(use.x, use.z))) continue;
    if (sites.taken.some((t) => circleHits(use.x, use.z, t))) continue;
    return box;
  }
  return null;
}

/**
 * `room` with the arrival box standing in it, for one visit (C14): `room`
 * is the room re-dressed round the box through `withHeroes` (C15), less
 * the room's own heroes whose floor overlaps the box's grown by
 * `HERO_CLEAR`; `box` the box's index in that room's `heroes` (sorted by
 * `HERO_ORDER`, so not always the last); `spawn` where the player steps
 * out, in metres: the box's use point, with the yaw that faces the way the
 * box faces (into the hall). When no candidate fits, `room` is the very
 * object handed in and `box` and `spawn` are null. Never throws.
 */
export function withArrivalBox(
  place: PlaceInput,
  room: RoomSpec,
): {
  room: RoomSpec;
  box: number | null;
  spawn: { x: number; z: number; yaw: number } | null;
} {
  const placed = placeArrivalBox(room);
  const use = placed === null ? null : heroUsePoint(placed);
  if (placed === null || use === null) return { room, box: null, spawn: null };
  const moat = grow(heroFootprint(placed), HERO_CLEAR);
  const kept = room.heroes.filter((h) => !overlaps(moat, heroFootprint(h)));
  const next = withHeroes(place, room, [...kept, placed]);
  const box = next.heroes.findIndex(
    (h) =>
      h.kind === placed.kind &&
      h.x === placed.x &&
      h.y === placed.y &&
      h.seed === placed.seed,
  );
  const [fx, fz] = HERO_FRONT[placed.turn] ?? [0, -1];
  // `forwardOf(yaw)` is `[-sin, -cos]`; the `+ 0` turns a -0 into 0.
  const yaw = Math.atan2(-fx, -fz) + 0;
  return { room: next, box, spawn: { x: use.x, z: use.z, yaw } };
}
