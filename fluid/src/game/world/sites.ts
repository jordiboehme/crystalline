/**
 * Where set dressing may go: the free wall edges, the clear lanes, the
 * footprints already taken, and the floor spots of a finished room. What
 * goes where is decided by the dressing pass (`dress.ts`); this module only
 * works out the places, from a room that has its fixtures, decor and
 * scaffolding but no props yet (`RoomBase`).
 *
 * The room states its own layout (`RoomSpec.entrance`, `bays` and
 * `corridor`, filled by the generator from `planLayout`), so nothing here is
 * read back from the spawn or rebuilt from the doorway columns: the entrance
 * edge is the `s` edge of `room.entrance`, and floor props may stand in the
 * hall and in `room.bays`, never in the corridor or a doorway column. The
 * doorway cells themselves are the floor cells of `doorwayColumns`.
 *
 * The rules, in the order `dressingSites` applies them:
 *
 * - **Wall edges.** The edges of `wallRuns`: every side of a floor cell
 *   whose neighbour is void or outside the grid, in straight runs.
 * - **Free edges.** Wall edges minus `fixtureEdges` (every fixture's slot
 *   edge, the placard's too; a slot no fixture took stays free), minus the
 *   entrance edge and the edges one cell either side of it along the same
 *   wall (`{x: e.x +- 1, y: e.y, side: "s"}`), minus every edge whose cell's
 *   x is in `doorwayColumns`. The removed edges are `noRun`, the edges no
 *   wall or ceiling run may cover either; free is exactly wall edges minus
 *   `noRun`, so the two never drift apart (ruling 6).
 * - **Lanes** (ruling 10), which floor props never enter:
 *   - for every door, hatch, portal, terminal and machine, sealed or not:
 *     `footprint(slot, { along: LANE_WIDTH, out: LANE_DEPTH })`;
 *   - the entrance: x `(entrance.x + 0.5) * CELL +- LANE_WIDTH / 2`, z from
 *     `((hall.y0 + hall.y1) / 2) * CELL` to `(entrance.y + 1) * CELL`;
 *   - every doorway column `c`: x from `(c - 1) * CELL` to `(c + 2) * CELL`,
 *     z over the column's floor rows, one cell wide on each side of it.
 * - **Taken.** `footprintOf` of every fixture, `decorFootprint` of every
 *   piece of decor, and every box of `room.scaffold`, with nulls dropped.
 * - **Corner zones.** For the hall and then each bay rectangle `r`, in that
 *   order: the four 2x2 blocks at `(r.x0, r.y0)`, `(r.x1 - 2, r.y0)`,
 *   `(r.x0, r.y1 - 2)` and `(r.x1 - 2, r.y1 - 2)`, keyed `"x0,y0"` of the
 *   block. Their cells, row by row, become spots with `wall: null`, except a
 *   cell next to a doorway cell (a floor cell in a doorway column, as a
 *   4-neighbour), which is left out. A zone is kept even when that leaves it
 *   fewer than four spots, or none.
 * - **Wall-side cells.** Every floor cell of the hall or a bay, the hall
 *   first and then each bay, row by row:
 *   - not in a zone and not next to a doorway cell;
 *   - that has a wall edge: the first side in `n`, `e`, `s`, `w` order
 *     whose edge is a wall edge;
 *   - whose wall edge is not a fixture edge. There is no fallback to a later
 *     side: a cell whose first wall edge carries a fixture is left out.
 *
 *   Whether a wall prop takes that edge later is decided by the pass, not
 *   here.
 * - **Long walls** (ruling 11). When the hall's width is at least its
 *   depth, the hall's `n` edges (west to east) and its `s` edges (east to
 *   west); otherwise its `w` edges (south to north) and its `e` edges (north
 *   to south). Only wall edges inside the hall rectangle count, in
 *   `wallRuns` order.
 *
 * Coordinates follow `Decor` (ruling 1): cells for spots and anchors,
 * metres for boxes. Pure and deterministic.
 *
 * This is the generator side: it imports `footprints.ts`, `layout.ts`,
 * `props.ts`, `types.ts` and `units.ts`, and never `move.ts`, `generate.ts`
 * or `interact.ts` (ruling 20). `sites.test.ts` keeps it so.
 */

import { decorFootprint, footprint, footprintOf } from "./footprints";
import { STEP, doorwayColumns, isFloor, wallRuns } from "./layout";
import { LANE_DEPTH, LANE_WIDTH } from "./props";
import type { Box, Rect, RoomSpec, Side, WallSlot } from "./types";
import { CELL } from "./units";

/**
 * A room as the dressing pass sees it: everything of a `RoomSpec` except
 * its props, which are what the pass is about to make. The sites are
 * worked out from fixtures, decor and scaffolding alone, so this type is
 * valid before `RoomSpec` gains `props` and after.
 */
export type RoomBase = Omit<RoomSpec, "props">;

/** A floor cell a floor prop may stand in, centred on it or backed to a wall. */
export interface FloorSpot {
  cx: number;
  cy: number;
  /** The free wall edge it may back onto, or null for a corner-zone cell. */
  wall: Side | null;
  /** The corner zone it belongs to (`"x0,y0"` of the zone), or null. */
  zone: string | null;
}

/**
 * Everything the dressing pass needs to know about where props may go,
 * worked out once per room by `dressingSites`. See the module doc for the
 * rules behind each field.
 */
export interface DressingSites {
  /** Every straight wall run of the grid, in `wallRuns` order. */
  runs: WallSlot[][];
  /** Wall edges wall props may take (ruling 6), keyed by `edgeKey`. */
  free: Set<string>;
  /** Fixture slot edges (every fixture, the placard too), keyed by `edgeKey`. */
  fixtureEdges: Set<string>;
  /** Edges no run may cover: fixture edges, entrance +-1, doorway-column edges. */
  noRun: Set<string>;
  /** Clear lanes floor props never enter (ruling 10), in metres. */
  lanes: Box[];
  /** Footprints floor props never overlap: fixtures, decor, scaffolding. */
  taken: Box[];
  /** Corner zones of the hall and each bay: zone key and its floor spots. */
  zones: { key: string; spots: FloorSpot[] }[];
  /** Wall-side cells of the hall and bays outside the zones, one spot per cell. */
  wallSide: FloorSpot[];
  /** The hall's two long walls, each its hall wall edges in walk order (ruling 11). */
  longWalls: [WallSlot[], WallSlot[]];
}

/**
 * The key of a wall edge, `"x,y,side"`: the same string for the same edge
 * wherever it comes from (a run, a fixture's slot, a prop's anchor), so sets
 * of edges can be compared and subtracted.
 */
export function edgeKey(e: WallSlot): string {
  return `${e.x},${e.y},${e.side}`;
}

/**
 * The quarter turn a wall-anchored thing gets so that its frame equals the
 * slot frame of the wall it is on (ruling 1): `s` 0, `w` 1, `n` 2, `e` 3.
 * A prop built at turn 0 faces north, which is a prop on a south wall
 * looking into the room; each turn is a quarter clockwise seen from above.
 */
export function turnForSide(side: Side): number {
  return TURN_FOR_SIDE[side];
}

const TURN_FOR_SIDE = { s: 0, w: 1, n: 2, e: 3 } satisfies Record<Side, number>;

/**
 * Where a wall or ceiling prop on edge `e` is anchored (ruling 1): the edge's
 * wall point, the origin of `frameForSlot(e)`, in `Decor`'s continuous cell
 * units, and the turn from `turnForSide`. For `n` it is `(x + 0.5, y)`, for
 * `s` `(x + 0.5, y + 1)`, for `w` `(x, y + 0.5)` and for `e`
 * `(x + 1, y + 0.5)`: the middle of the wall at floor level.
 */
export function wallAnchor(e: WallSlot): {
  x: number;
  y: number;
  turn: number;
} {
  const turn = turnForSide(e.side);
  switch (e.side) {
    case "n":
      return { x: e.x + 0.5, y: e.y, turn };
    case "s":
      return { x: e.x + 0.5, y: e.y + 1, turn };
    case "w":
      return { x: e.x, y: e.y + 0.5, turn };
    case "e":
      return { x: e.x + 1, y: e.y + 0.5, turn };
  }
}

/**
 * True when two boxes share floor. Touching edges do not count, so two
 * props standing side by side, or a prop against a lane's border, are fine.
 */
export function overlaps(a: Box, b: Box): boolean {
  return a.x0 < b.x1 && b.x0 < a.x1 && a.z0 < b.z1 && b.z0 < a.z1;
}

/**
 * Slack for a box edge that lies on a cell border but came out of float
 * arithmetic a hair past it, so the box does not claim the next cell.
 */
const EPS = 1e-9;

/**
 * True when a floor prop's box stands on the room's floor: every cell under
 * the box (a box ending exactly on a cell border does not reach the next
 * cell) is floor inside the hall or a bay. That rules out void, the doorway
 * columns (they lie between the hall and the bays, and between the corridor
 * and the hall) and the corridor. An empty box fits nowhere.
 */
export function fitsFloor(room: RoomBase, box: Box): boolean {
  if (!(box.x1 > box.x0 && box.z1 > box.z0)) return false;
  const rects = roomRects(room);
  const cx0 = Math.floor((box.x0 + EPS) / CELL);
  const cx1 = Math.ceil((box.x1 - EPS) / CELL) - 1;
  const cy0 = Math.floor((box.z0 + EPS) / CELL);
  const cy1 = Math.ceil((box.z1 - EPS) / CELL) - 1;
  for (let y = cy0; y <= cy1; y++)
    for (let x = cx0; x <= cx1; x++) {
      if (!isFloor(room.grid, x, y)) return false;
      if (!rects.some((r) => inside(r, x, y))) return false;
    }
  return true;
}

/**
 * Where props may go in a room: runs, free edges, lanes, taken footprints,
 * corner zones, wall-side cells and long walls. See the module doc for the
 * rules.
 */
export function dressingSites(room: RoomBase): DressingSites {
  const runs = wallRuns(room.grid);
  const edges = runs.flat();
  const wallEdges = new Set(edges.map(edgeKey));
  const cols = doorwayColumns(room);
  const rects = roomRects(room);
  const entrance: WallSlot = { ...room.entrance, side: "s" };

  const fixtureEdges = new Set(room.fixtures.map((f) => edgeKey(f.slot)));
  const noRun = new Set(fixtureEdges);
  for (let dx = -1; dx <= 1; dx++)
    noRun.add(edgeKey({ ...entrance, x: entrance.x + dx }));
  for (const e of edges) if (cols.has(e.x)) noRun.add(edgeKey(e));
  const free = new Set([...wallEdges].filter((k) => !noRun.has(k)));

  const lanes: Box[] = [];
  const size = { along: LANE_WIDTH, out: LANE_DEPTH };
  for (const f of room.fixtures) {
    switch (f.kind) {
      case "door":
      case "hatch":
      case "portal":
      case "terminal":
      case "machine":
        lanes.push(footprint(f.slot, size));
        break;
      case "poster":
      case "placard":
        break;
    }
  }
  const ex = (entrance.x + 0.5) * CELL;
  lanes.push({
    x0: ex - LANE_WIDTH / 2,
    x1: ex + LANE_WIDTH / 2,
    z0: ((room.hall.y0 + room.hall.y1) / 2) * CELL,
    z1: (entrance.y + 1) * CELL,
  });
  const doorway = new Set<string>();
  for (const c of [...cols].sort((a, b) => a - b)) {
    const rows: number[] = [];
    for (let y = 0; y < room.depth; y++) {
      if (!isFloor(room.grid, c, y)) continue;
      rows.push(y);
      doorway.add(cellKey(c, y));
    }
    const first = rows[0];
    const last = rows.at(-1);
    if (first === undefined || last === undefined) continue;
    lanes.push({
      x0: (c - 1) * CELL,
      x1: (c + 2) * CELL,
      z0: first * CELL,
      z1: (last + 1) * CELL,
    });
  }

  const taken: Box[] = [];
  for (const f of room.fixtures) {
    const box = footprintOf(f);
    if (box !== null) taken.push(box);
  }
  for (const d of room.decor) {
    const box = decorFootprint(d);
    if (box !== null) taken.push(box);
  }
  taken.push(...room.scaffold);

  const nextToDoorway = (x: number, y: number) =>
    Object.values(STEP).some(([dx, dy]) =>
      doorway.has(cellKey(x + dx, y + dy)),
    );

  const zones: DressingSites["zones"] = [];
  const zoneCells = new Set<string>();
  for (const r of rects) {
    const corners: [number, number][] = [
      [r.x0, r.y0],
      [r.x1 - 2, r.y0],
      [r.x0, r.y1 - 2],
      [r.x1 - 2, r.y1 - 2],
    ];
    for (const [zx, zy] of corners) {
      const key = cellKey(zx, zy);
      const spots: FloorSpot[] = [];
      for (let y = zy; y < zy + 2; y++)
        for (let x = zx; x < zx + 2; x++) {
          zoneCells.add(cellKey(x, y));
          if (!isFloor(room.grid, x, y) || nextToDoorway(x, y)) continue;
          spots.push({ cx: x, cy: y, wall: null, zone: key });
        }
      zones.push({ key, spots });
    }
  }

  const wallSide: FloorSpot[] = [];
  for (const r of rects)
    for (let y = r.y0; y < r.y1; y++)
      for (let x = r.x0; x < r.x1; x++) {
        if (!isFloor(room.grid, x, y)) continue;
        if (zoneCells.has(cellKey(x, y)) || nextToDoorway(x, y)) continue;
        const side = SIDES.find((s) =>
          wallEdges.has(edgeKey({ x, y, side: s })),
        );
        if (side === undefined) continue;
        if (fixtureEdges.has(edgeKey({ x, y, side }))) continue;
        wallSide.push({ cx: x, cy: y, wall: side, zone: null });
      }

  const hall = room.hall;
  const [a, b]: [Side, Side] =
    hall.x1 - hall.x0 >= hall.y1 - hall.y0 ? ["n", "s"] : ["w", "e"];
  const hallEdges = (side: Side) =>
    edges.filter((e) => e.side === side && inside(hall, e.x, e.y));

  return {
    runs,
    free,
    fixtureEdges,
    noRun,
    lanes,
    taken,
    zones,
    wallSide,
    longWalls: [hallEdges(a), hallEdges(b)],
  };
}

/** The order a floor cell's walls are tried in for a wall-side spot. */
const SIDES: readonly Side[] = ["n", "e", "s", "w"];

function cellKey(x: number, y: number) {
  return `${x},${y}`;
}

function inside(r: Rect, x: number, y: number) {
  return x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1;
}

/** The hall and then each bay, west to east, as the room states them. */
function roomRects(room: RoomBase): Rect[] {
  return [room.hall, ...room.bays];
}
