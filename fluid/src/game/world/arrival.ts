/**
 * The arrival box (2.6e C14): a walk out of the console room lands the
 * player on another domain's bridge, stepping out of a police box that
 * stands beside the bridge's entrance for that one visit.
 *
 * The box stands free near the entrance, never against a wall (2.6e,
 * "free-standing, always"), facing north into the hall (turn 0: a
 * generated hall's entrance is always in its south wall, so north is
 * away from it and into the hall), at the first of
 * `arrivalBoxCandidates` where it fits. The candidates are the hall's
 * half-cell points within `ARRIVAL_REACH` cells of the entrance's doorway
 * (the middle of the entrance cell's south edge), nearest first, the east
 * one first where two lie equally far. `placeArrivalBox` takes a candidate when
 * the box stands free (`standsFree` in `heroes.ts`: a walkway of `HERO_WALKWAY`
 * all round it, clear of every wall and of what hangs on the walls), its floor
 * (`heroFootprint`) enters no lane of `dressingSites(room)`, its moat
 * (`HERO_CLEAR`, wider than the walkway) overlaps none of its taken boxes
 * (the fixtures' footprints, the furniture and the scaffold frames) and
 * no pipe run hanging where the tall box would reach. The player's
 * circle at the box's use point (`heroUsePoint`, `HERO_USE_OUT` in front
 * of it) then lies inside the walkway and the moat, so it is on floor and
 * clear of the same boxes. It reads nothing of the room's heroes, props or
 * curios: those move round the box, not the box round them.
 *
 * `withArrivalBox` then drops the room's own heroes whose floor overlaps
 * the box's moat (grown by `HERO_CLEAR`), keeps the rest, and re-dresses
 * the room round the box and the kept heroes through the shared
 * forced-hero seam (`withHeroes` in `generate.ts`, C15), so props and
 * curios keep off the box's moat as they keep off any hero the generator
 * drew: the walkway round it holds nothing. The player's spawn is the
 * box's use point, facing the way the box faces. When no candidate fits
 * there is no box: the room comes back as it was handed in, the spawn is
 * null and the player arrives at the entrance, as on any other visit.
 *
 * This is the session side (C15): nothing on the generator side imports
 * it, so a bridge entered any other way (a door, a reload, the level
 * select) is generated plain, and the box lives only in the room object
 * the session holds for that visit. It imports `generate.ts`,
 * `heroes.ts`, `footprints.ts` and `sites.ts`.
 */

import { seedFor } from "../core/seed";
import { HERO_FRONT, heroFootprint, pipeRunBox } from "./footprints";
import { withHeroes } from "./generate";
import { HERO_CLEAR, heroUsePoint, standsFree } from "./heroes";
import { yawFacing } from "./interact";
import { dressingSites, grow, overlaps } from "./sites";
import type { Hero, PlaceInput, RoomSpec } from "./types";

/**
 * How far from the entrance's doorway an arrival box may stand, in cells
 * (the box's centre, measured from the middle of the entrance cell's south
 * edge): near enough that the player plainly steps out beside the way in.
 */
export const ARRIVAL_REACH = 4;

/** One point the arrival box may stand at: its centre, in cell units. */
export interface ArrivalSpot {
  x: number;
  y: number;
}

/**
 * The points the arrival box may stand at, best first (C14): every
 * half-cell point of the hall within `ARRIVAL_REACH` cells of the
 * entrance's doorway (the middle of the entrance cell's south edge,
 * `(e.x + 0.5, e.y + 1)`), nearest first, the east one first where two
 * lie equally far (two points equally far on one column would lie either
 * side of the doorway, and the hall has only the north side). Whether the
 * box fits there is `placeArrivalBox`'s business.
 */
export function arrivalBoxCandidates(room: RoomSpec): ArrivalSpot[] {
  const e = room.entrance;
  const ex = e.x + 0.5;
  const ey = e.y + 1;
  const h = room.hall;
  const out: (ArrivalSpot & { far: number })[] = [];
  for (let y2 = 2 * h.y0; y2 <= 2 * h.y1; y2++)
    for (let x2 = 2 * h.x0; x2 <= 2 * h.x1; x2++) {
      const x = x2 / 2;
      const y = y2 / 2;
      const far = Math.hypot(x - ex, y - ey);
      if (far <= ARRIVAL_REACH) out.push({ x, y, far });
    }
  out.sort((a, b) => a.far - b.far || b.x - a.x);
  return out.map(({ x, y }) => ({ x, y }));
}

/**
 * The arrival box for `room`, or null when none fits (C14): a police box,
 * variant 0, seed `seedFor(room.seed, "arrival-box")`, turn 0 (facing
 * north, into the hall), centred on the first of
 * `arrivalBoxCandidates(room)` where it stands free (`standsFree`), its
 * floor enters no lane, its moat (`HERO_CLEAR`) overlaps no taken box
 * (fixture footprints, furniture, scaffold) and no pipe run. The
 * player's circle at its use point (`HERO_USE_OUT` in front, the player's
 * radius round it) lies inside the walkway and the moat, so it is on floor
 * and clear of those boxes with no check of its own. The room's heroes,
 * props and curios are not read.
 */
export function placeArrivalBox(room: RoomSpec): Hero | null {
  const sites = dressingSites(room);
  const seed = seedFor(room.seed, "arrival-box");
  const pipes = room.decor
    .map((d) => pipeRunBox(d, room.hall))
    .filter((b) => b !== null);
  for (const c of arrivalBoxCandidates(room)) {
    const box: Hero = {
      kind: "police-box",
      variant: 0,
      x: c.x,
      y: c.y,
      turn: 0,
      seed,
    };
    const floor = heroFootprint(box);
    if (!standsFree(room, floor)) continue;
    if (sites.lanes.some((l) => overlaps(l, floor))) continue;
    const moat = grow(floor, HERO_CLEAR);
    if (sites.taken.some((t) => overlaps(t, moat))) continue;
    if (pipes.some((p) => overlaps(p, moat))) continue;
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
  const front = HERO_FRONT[placed.turn] ?? [0, -1];
  const yaw = yawFacing(front);
  return { room: next, box, spawn: { x: use.x, z: use.z, yaw } };
}
