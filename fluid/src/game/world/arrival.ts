/**
 * The arrival box (2.6e C14): a walk out of the console room lands the
 * player on another domain's bridge, stepping out of a police box that
 * stands beside the bridge's entrance for that one visit.
 *
 * The box stands free near the entrance, never against a wall (2.6e,
 * "free-standing, always"), facing north into the hall (turn 0: a
 * generated hall's entrance is always in its south wall, so north is
 * away from it and into the hall) unless it falls back to standing
 * against a side wall (Jordi, M4; see below), at the first spot three
 * passes in `placeArrivalBox` try in turn:
 *
 * 1. Every half-cell point of the hall within `ARRIVAL_REACH` cells of
 *    the entrance's doorway (`arrivalBoxCandidates`, the middle of the
 *    entrance cell's south edge), nearest first, the east one first
 *    where two lie equally far, kept to the ones that clear the bridge
 *    lift's column (the entrance's own edge) by `ARRIVAL_LIFT_CLEARANCE`
 *    cells, so the lift and the box both stay in plain view.
 * 2. Failing that (most real bridges are `layout.ts`'s 5-cell minimum
 *    width, too narrow for any spot to clear the column by that much):
 *    pressed against whichever side wall sits nearer the lift's column,
 *    at least `ARRIVAL_SIDE_DEPTH` rows in from the lift's own wall and
 *    never onto the lift's own column (`ARRIVAL_LIFT_COLUMN`), turned to
 *    face the hall's centre line instead of north
 *    (`placeAgainstSideWall`) - still plainly clear of the lift, just not
 *    two full cells off its column.
 * 3. Last resort, so a hall this cramped still lands the box exactly as
 *    it did before this rule existed: every `arrivalBoxCandidates` point
 *    again with no rule against the lift's column at all, nearest first.
 *    Nothing here stops this pass reaching a spot `ARRIVAL_LIFT_COLUMN`
 *    cells or farther off the column - rows 0 and 1 beside the lift's own
 *    wall fall outside both passes 1 and 2's own row rules, and a spot
 *    past `ARRIVAL_REACH` falls outside every pass. In practice the
 *    lift's own lane and the walkway rule `fits` already runs for every
 *    pass rule those rows out too, so a landing sweep over real bridges
 *    finds this pass on the lift's column only when nothing else in
 *    reach is free - measured, not guaranteed by this function's shape
 *    alone.
 *
 * Every pass takes a candidate when the box stands free (`standsFree` in
 * `heroes.ts`: a walkway of `HERO_WALKWAY` all round it, clear of every
 * wall and of what hangs on the walls), its floor (`heroFootprint`)
 * enters no lane of `dressingSites(room)`, its moat (`HERO_CLEAR`, wider
 * than the walkway) overlaps none of its taken boxes (the fixtures'
 * footprints, the furniture and the scaffold frames) and no pipe run
 * hanging where the tall box would reach. The player's circle at the
 * box's use point (`heroUsePoint`, `HERO_USE_OUT` in front of it) then
 * lies inside the walkway and the moat, so it is on floor and clear of
 * the same boxes. It reads nothing of the room's heroes, props or curios:
 * those move round the box, not the box round them.
 *
 * `withArrivalBox` then drops the room's own heroes whose floor overlaps
 * the box's moat (grown by `HERO_CLEAR`), keeps the rest, and re-dresses
 * the room round the box and the kept heroes through the shared
 * forced-hero seam (`withHeroes` in `generate.ts`, C15), so props,
 * curios and decals keep off the box's moat as they keep off any hero the
 * generator drew: the walkway round it holds nothing. The player's spawn is the
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
 * How far the arrival box's centre must stand from the bridge lift's
 * column (the entrance edge's midpoint, `room.entrance.x + 0.5`) on the x
 * axis, in cells, when a spot that far out is free (Jordi, M4): with the
 * lift always on the entrance edge, standing this far off keeps the lift
 * and the box both plainly in view from the console room's exit and
 * leaves the walk between them open. A hall too narrow to spare a spot
 * that far out falls back to `placeAgainstSideWall`, then further still
 * (see `placeArrivalBox`): the clearance is a preference over
 * `arrivalBoxCandidates`' own nearest-first order, never a reason to
 * strand the player at the entrance.
 */
export const ARRIVAL_LIFT_CLEARANCE = 2;

/**
 * How many rows in from the lift's wall (the entrance edge) the box's
 * side-wall fallback stands, at minimum (Jordi, M4), read the way the
 * round-1 ruling counted rows: row 0 is the one touching the wall, so a
 * centre `ARRIVAL_SIDE_DEPTH` rows in is at least that many whole rows
 * plus its own half back from the wall, `south - ARRIVAL_SIDE_DEPTH -
 * 0.5`. A bridge only `layout.ts`'s 5-cell minimum wide has no spot
 * `ARRIVAL_LIFT_CLEARANCE` off the lift's column at all (the widest gap a
 * free-standing box can reach there, against a side wall, is 1.5 cells),
 * so `placeAgainstSideWall` presses the box against whichever side wall
 * sits nearer the lift's column instead, this many rows past the
 * doorway, so it plainly is not just a step from the lift.
 */
export const ARRIVAL_SIDE_DEPTH = 2;

/**
 * The closest a column `placeAgainstSideWall` tries may come to the
 * lift's column, in cells, without the box's floor reaching the lift's
 * own cell (Jordi, M4): the box is 1.3 m wide, 0.65 m either side of its
 * centre, well inside the half cell (1 m) `ARRIVAL_LIFT_COLUMN` leaves it
 * from the column's near edge. So pressing a blocked near-wall column
 * inward, spot by spot, when the wall itself has nothing free, never
 * drifts onto the lift's own column.
 */
export const ARRIVAL_LIFT_COLUMN = 1;

/**
 * Half-cell x values between `x0` and `x1`, kept to the side of `limit`
 * that `wall` is on (so a search that never reaches `limit` never crosses
 * it), nearest `wall` first: the column order `placeAgainstSideWall`
 * presses in from the wall with.
 */
function columnsFromWall(
  wall: number,
  limit: number,
  x0: number,
  x1: number,
): number[] {
  const lo = Math.max(Math.min(wall, limit), x0);
  const hi = Math.min(Math.max(wall, limit), x1);
  const out: number[] = [];
  for (let k = Math.round(lo * 2); k <= Math.round(hi * 2); k++)
    out.push(k / 2);
  out.sort((a, b) => Math.abs(a - wall) - Math.abs(b - wall));
  return out;
}

/**
 * Half-cell y values at least `ARRIVAL_SIDE_DEPTH` rows in from the
 * lift's wall (`south`, see `ARRIVAL_SIDE_DEPTH`'s doc for the row
 * count), down to the hall's north wall (`y0`), nearest that minimum
 * first.
 */
function depthsFromEntrance(south: number, y0: number): number[] {
  const maxY = south - ARRIVAL_SIDE_DEPTH - 0.5;
  const out: number[] = [];
  for (let k = Math.round(y0 * 2); k <= Math.round(maxY * 2); k++)
    out.push(k / 2);
  out.sort((a, b) => b - a);
  return out;
}

/**
 * The arrival box pressed against a side wall (Jordi, M4), or null when
 * neither wall has a spot that fits: the wall nearer the lift's column
 * first, then the other, at every column from that wall in
 * (`columnsFromWall`, nearest the wall first) kept short of
 * `ARRIVAL_LIFT_COLUMN` cells off the lift's column - so pressing a
 * blocked near-wall spot inward never drifts onto the lift's own column -
 * crossed with every row at least `ARRIVAL_SIDE_DEPTH` in from the lift's
 * own wall (`depthsFromEntrance`, nearest that minimum first), turned to
 * face the hall's centre line instead of north: east from the west wall
 * (turn 1), west from the east wall (turn 3).
 */
function placeAgainstSideWall(
  room: RoomSpec,
  fits: (c: ArrivalSpot, turn: number) => Hero | null,
): Hero | null {
  const centreX = room.entrance.x + 0.5;
  const south = room.entrance.y + 1;
  const sides = [
    { turn: 1, wall: room.hall.x0, limit: centreX - ARRIVAL_LIFT_COLUMN },
    { turn: 3, wall: room.hall.x1, limit: centreX + ARRIVAL_LIFT_COLUMN },
  ].sort((a, b) => Math.abs(a.wall - centreX) - Math.abs(b.wall - centreX));
  const ys = depthsFromEntrance(south, room.hall.y0);
  for (const side of sides) {
    const xs = columnsFromWall(
      side.wall,
      side.limit,
      room.hall.x0,
      room.hall.x1,
    );
    for (const x of xs)
      for (const y of ys) {
        const box = fits({ x, y }, side.turn);
        if (box !== null) return box;
      }
  }
  return null;
}

/**
 * The arrival box for `room`, or null when none fits (C14): a police box,
 * variant 0, seed `seedFor(room.seed, "arrival-box")`, at the first spot
 * the module doc's three passes try in turn - `arrivalBoxCandidates(room)`
 * clear of the lift's column by `ARRIVAL_LIFT_CLEARANCE`, then
 * `placeAgainstSideWall`, then `arrivalBoxCandidates(room)` a last time
 * with no rule against the lift's column at all. Every pass takes a
 * candidate when the box stands free (`standsFree`), its floor enters no
 * lane, its moat (`HERO_CLEAR`) overlaps no taken box (fixture
 * footprints, furniture, scaffold) and no pipe run. The player's circle
 * at its use point (`HERO_USE_OUT` in front, the player's radius round
 * it) lies inside the walkway and the moat, so it is on floor and clear
 * of those boxes with no check of its own. The room's heroes, props and
 * curios are not read.
 */
export function placeArrivalBox(room: RoomSpec): Hero | null {
  const sites = dressingSites(room);
  const seed = seedFor(room.seed, "arrival-box");
  const pipes = room.decor
    .map((d) => pipeRunBox(d, room.hall))
    .filter((b) => b !== null);
  const fits = (c: ArrivalSpot, turn: number): Hero | null => {
    const box: Hero = {
      kind: "police-box",
      variant: 0,
      x: c.x,
      y: c.y,
      turn,
      seed,
    };
    const floor = heroFootprint(box);
    if (!standsFree(room, floor)) return null;
    if (sites.lanes.some((l) => overlaps(l, floor))) return null;
    const moat = grow(floor, HERO_CLEAR);
    if (sites.taken.some((t) => overlaps(t, moat))) return null;
    if (pipes.some((p) => overlaps(p, moat))) return null;
    return box;
  };
  const candidates = arrivalBoxCandidates(room);
  const liftX = room.entrance.x + 0.5;

  for (const c of candidates) {
    if (Math.abs(c.x - liftX) < ARRIVAL_LIFT_CLEARANCE) continue;
    const box = fits(c, 0);
    if (box !== null) return box;
  }

  const bySideWall = placeAgainstSideWall(room, fits);
  if (bySideWall !== null) return bySideWall;

  for (const c of candidates) {
    const box = fits(c, 0);
    if (box !== null) return box;
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
