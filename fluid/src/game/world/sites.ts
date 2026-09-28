/**
 * Where set dressing may go: the free wall edges, the clear lanes, the
 * footprints already taken, and the floor spots of a finished room. What
 * goes where is decided by the dressing pass (`dress.ts`); this module only
 * works out the places, from a room that has its fixtures, decor and
 * scaffolding (`SiteBase`: no heroes, no props and no curios are read, so
 * the hero pass and the dressing see the same sites). The curio pass reads
 * the finished room less its curios (`CurioBase`, C2).
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
 *   - for every door, hatch and portal, sealed or not:
 *     `footprint(slot, { along: LANE_WIDTH, out: LANE_DEPTH })` (4 m deep);
 *   - for every terminal and machine:
 *     `footprint(slot, { along: LANE_WIDTH, out: USE_LANE_DEPTH })`
 *     (2.5 m deep, E6): a fixture is used from within `REACH` of its wall
 *     point, so the shallower lane still holds its whole use range;
 *   - for every poster and the placard, a viewing lane:
 *     `footprint(slot, { along: SHEET_LANE_WIDTH, out: SHEET_LANE_DEPTH })`
 *     (1.2 m by 1.5 m), so no floor prop stands in front of the sheet;
 *   - the wall stencil's edge (`stencilEdge`, 2.7 C19), when it is a wall
 *     edge and no fixture's: a viewing lane of the same size,
 *     `footprint(edge, STENCIL_STRIP)`, so no floor prop or hero stands in
 *     front of the deck and bay stencil and hides it;
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
 * - **Cluster blocks** (D6), in a large hall only (`isLargeHall`): the
 *   hall's interior band (`interiorBand`) tiled from its north-west corner
 *   into `CLUSTER_BLOCK` by `CLUSTER_BLOCK` blocks, row by row, partial
 *   blocks dropped. Each is keyed `"x,y"` by its north-west cell, and its
 *   spots are its inner `CLUSTER_INNER` by `CLUSTER_INNER` cells, one in
 *   from its north-west corner, row by row, with `wall: null` and
 *   `zone: null`; an inner cell that is not floor is left out, and a block
 *   is kept even when that leaves it none. A small hall has no blocks.
 * - **Span lines** (D9), in a large hall only: first every row `y` from
 *   `hall.y0 + 1` to `hall.y1 - 2`, north to south, whose cells from
 *   `hall.x0 + 1` to `hall.x1 - 2` are cut into `SPAN_CELLS`-cell segments
 *   from the west end, a leftover odd cell at the east end dropped; then
 *   every column `x` from `hall.x0 + 1` to `hall.x1 - 2`, west to east, cut
 *   the same way from the north end over the rows `hall.y0 + 1` to
 *   `hall.y1 - 2`. A line is kept only when none of its segments'
 *   `spanBox`es overlaps a decor footprint, a box of `room.scaffold`, a
 *   lamp's box (`lampBoxes`) or a pipe run's box (`pipeRunBox`): the lab
 *   island's duct, the specimen tanks and the scaffold poles reach the
 *   ceiling, a span under a lamp would hide it, and a pipe run hangs in the
 *   span band (E3). A line is dropped whole, never cut short. Every span
 *   stays at least one full cell (2 m) off every hall wall, past the
 *   ceiling band along the walls (1.2 m) where the runs, loops, beacon and
 *   loose cables hang. Which palette hangs a span, and on which line, is
 *   the pass's choice; this module only lists the lines.
 * - **Long walls** (ruling 11). When the hall's width is at least its
 *   depth, the hall's `n` edges (west to east) and its `s` edges (east to
 *   west); otherwise its `w` edges (south to north) and its `e` edges (north
 *   to south). Only wall edges inside the hall rectangle count, in
 *   `wallRuns` order.
 *
 * Coordinates follow `Decor` (ruling 1): cells for spots and anchors,
 * metres for boxes. Pure and deterministic.
 *
 * It also holds the small helpers the dressing and the hero pass share, so
 * neither keeps a copy: `EPS`, `round3`, `grow`, `pickByRoll` and `inside`.
 * `edgeOf`, `wallAnchor`'s inverse, is the same kind of shared helper for
 * `dev/` (which reads a single-edge wall or ceiling prop's edge back from
 * its anchor point and turn) rather than for the generator side.
 *
 * `Reserved` (`{ boxes, edges }`), `NO_RESERVE` and `mergeReserved` are
 * what a pass that runs before the dressing, such as the hero pass, hands
 * `dress.ts` to keep its own boxes and wall edges out of the set dressing;
 * see `Reserved`'s own doc comment for what each field keeps clear of.
 *
 * `Near`, `NO_NEAR` and `skipNear` are what the hero and curio passes
 * share for their neighbours (2.6f C7, C8): the kinds the rooms a room's
 * ways lead to draw raw (`nearOf` in `neighbours.ts` fills them), and a
 * weighted list less those kinds as far as the list allows.
 *
 * This is the generator side: it imports `footprints.ts`, `lamps.ts`,
 * `layout.ts`, `props.ts`, `types.ts` and `units.ts`, and never `move.ts`,
 * `generate.ts` or `interact.ts` (ruling 20). `sites.test.ts` keeps it so.
 */

import {
  decorFootprint,
  footprint,
  footprintOf,
  pipeRunBox,
} from "./footprints";
import { lampBoxes } from "./lamps";
import { BAND_MARGIN, STEP, doorwayColumns, isFloor, wallRuns } from "./layout";
import {
  CLUSTER_BLOCK,
  CLUSTER_INNER,
  LANE_DEPTH,
  LANE_WIDTH,
  SHEET_LANE_DEPTH,
  SHEET_LANE_WIDTH,
  SPAN_CELLS,
  SPAN_HALF,
  USE_LANE_DEPTH,
} from "./props";
import type {
  Box,
  CurioKind,
  HeroKind,
  Rect,
  RoomSpec,
  Side,
  WallSlot,
} from "./types";
import { CELL } from "./units";

/**
 * A room as the dressing pass sees it: everything of a `RoomSpec` except
 * its props, which are what the pass is about to make, and its curios,
 * which come after it (C2), its finish (2.7 C8, C11), which the
 * generator writes last and no pass reads, and its decals (2.7 C13), laid
 * after the curios. It carries the heroes, which the
 * dressing reads to keep off what they reserve (`heroReserve` in
 * `heroes.ts`, H3). Every `RoomBase` is a `SiteBase`.
 */
export type RoomBase = Omit<RoomSpec, "props" | "curios" | "finish" | "decals">;

/**
 * A room as the site rules and the hero pass see it: the sites are worked
 * out from fixtures, decor and scaffolding alone, so this type leaves out
 * the props, the heroes, the curios, the finish and the decals. The hero pass (`heroes.ts`) reads
 * the sites before any hero stands, and the dressing reads the same sites
 * and then keeps off what the heroes reserved (`heroReserve`).
 */
export type SiteBase = Omit<
  RoomSpec,
  "props" | "heroes" | "curios" | "finish" | "decals"
>;

/**
 * A room as the curio pass (`placeCurios` in `curios.ts`) sees it (C2):
 * everything of a `RoomSpec` but the curios, which are what the pass is
 * about to make, the finish, which no pass reads, and the decals, laid
 * after it. The pass runs last, after the dressing, and reads the
 * fixtures, decor, heroes and props it stands curios on without changing
 * any of them. Since neither `SiteBase` nor `RoomBase` carries the curios,
 * neither the hero pass nor the dressing can see one, so a curio never
 * moves anything else in a room.
 */
export type CurioBase = Omit<RoomSpec, "curios" | "finish" | "decals">;

/**
 * A room as the decal pass (`placeDecals` in `decals.ts`, 2.7 C13) sees
 * it: everything of a `RoomSpec` but the decals, which are what the pass
 * is about to make. It runs after the curios and the finish and reads
 * the fixtures, decor, scaffolding, heroes and props to keep every decal
 * off text and off every footprint, without changing any of them.
 */
export type DecalBase = Omit<RoomSpec, "decals">;

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
 * A mid-hall cluster block (D6): a `CLUSTER_BLOCK` by `CLUSTER_BLOCK` tile
 * of the hall's interior band. `x` and `y` are its north-west cell and
 * `key` is `"x,y"`, which also keys the block's seed. A cluster stands only
 * on `cells`, the block's inner `CLUSTER_INNER` by `CLUSTER_INNER` cells
 * that are floor, one in from its north-west corner, row by row, as spots
 * with no wall and no zone; the block's empty first row and column keep
 * two clusters' inner cells at least one cell apart (E4).
 */
export interface ClusterBlock {
  key: string;
  x: number;
  y: number;
  cells: FloorSpot[];
}

/**
 * A clear line a ceiling span may hang on (D9): a row (`axis` "x", the
 * line runs west to east at row `index`) or a column (`axis` "y", north to
 * south at column `index`) of a large hall, one cell in from every hall
 * wall. `segments` holds each `SPAN_CELLS`-cell segment's first cell, from
 * the west or north end; its plan box is `spanBox(axis, segment)`.
 */
export interface SpanLine {
  axis: "x" | "y";
  index: number;
  segments: { x: number; y: number }[];
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
  /** Mid-hall cluster blocks, row by row; empty unless the hall is large (D6). */
  clusterBlocks: ClusterBlock[];
  /** Clear span lines, rows then columns; empty unless the hall is large (D9). */
  spanLines: SpanLine[];
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

/** The side a wall-anchored thing's turn stands on, the inverse of `turnForSide`. */
const SIDE_FOR_TURN: readonly Side[] = ["s", "w", "n", "e"];

/**
 * The wall edge a single-edge wall-anchored thing is anchored on, the
 * inverse of `wallAnchor`: recovers the edge from the anchor point `(x, y)`
 * and the turn `wallAnchor` gave it. Only a single-edge thing (a wall or
 * ceiling prop; a wall-anchored hero with one edge) inverts this way; a
 * two-edge hero's combined anchor (`heroEdges`'s two-edge case in
 * `heroes.ts`) does not.
 */
export function edgeOf(p: { x: number; y: number; turn: number }): WallSlot {
  const side = SIDE_FOR_TURN[p.turn] ?? "s";
  switch (side) {
    case "n":
      return { x: p.x - 0.5, y: p.y, side };
    case "s":
      return { x: p.x - 0.5, y: p.y - 1, side };
    case "w":
      return { x: p.x, y: p.y - 0.5, side };
    case "e":
      return { x: p.x - 1, y: p.y - 0.5, side };
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
 * What a pass that runs before the dressing has claimed (H4): `boxes`, in
 * metres, that no floor prop overlaps, that a cluster member's ring keeps
 * clear of and that no span segment crosses; and `edges`, wall edges keyed
 * by `edgeKey`, that no wall prop (mandatory ones included), wall run,
 * ceiling run, cable loop or loose cable takes. The hero pass fills it
 * (`heroReserve` in `heroes.ts`).
 */
export interface Reserved {
  boxes: readonly Box[];
  edges: ReadonlySet<string>;
}

/** Nothing reserved: the dressing of a room without heroes. */
export const NO_RESERVE: Reserved = { boxes: [], edges: new Set<string>() };

/** Both reservations at once: their boxes and their edges together. */
export function mergeReserved(a: Reserved, b: Reserved): Reserved {
  return {
    boxes: [...a.boxes, ...b.boxes],
    edges: new Set([...a.edges, ...b.edges]),
  };
}

/**
 * Slack for a box edge that lies on a cell border but came out of float
 * arithmetic a hair past it, so the box does not claim the next cell (and,
 * in the hero pass, a band hero's box does not count as leaving the band).
 */
export const EPS = 1e-9;

/**
 * Rounds a coordinate to three decimals, so the goldens are the same on
 * every engine. The dressing and the hero pass round each anchor as the
 * thing is made, so acceptance measures exactly what is returned.
 */
export function round3(v: number): number {
  return Math.round(v * 1000) / 1000;
}

/** A box grown by `m` metres on every side: a cluster's ring, a hero's moat. */
export function grow(b: Box, m: number): Box {
  return { x0: b.x0 - m, x1: b.x1 + m, z0: b.z0 - m, z1: b.z1 + m };
}

/**
 * One pick of a weighted list by a roll in [0, 1): the first entry whose
 * running weight passes `roll` times the total weight, the last one when
 * float rounding runs past the end, and null for an empty list. The
 * dressing feeds it a fresh draw (`weighted` in `dress.ts`), the hero pass
 * a pool slot's own roll.
 */
export function pickByRoll<K>(
  roll: number,
  picks: readonly (readonly [K, number])[],
): K | null {
  let total = 0;
  for (const [, w] of picks) total += w;
  let r = roll * total;
  for (const [k, w] of picks) {
    r -= w;
    if (r < 0) return k;
  }
  const last = picks.at(-1);
  return last === undefined ? null : last[0];
}

/**
 * What a room knows of its neighbours' picks (2.6f C5 to C8): the raw hero
 * (`rawHero`) and raw curios (`rawCurios`) of every room its doors,
 * portals and hatches lead to, each from that room's own seed and
 * archetype (`nearOf` in `neighbours.ts`). `heroes` and `curios` hold
 * every neighbour's, which a pick skips on a collision (C7);
 * `heroesBelow` and `curiosBelow` only those of a neighbour with a lower
 * seed, which a solo draw reads (C8), so of two neighbours that both draw
 * one, the lower keeps it.
 */
export interface Near {
  heroes: ReadonlySet<HeroKind>;
  heroesBelow: ReadonlySet<HeroKind>;
  curios: ReadonlySet<CurioKind>;
  curiosBelow: ReadonlySet<CurioKind>;
}

/** A room with no neighbour: nothing is skipped. Every forced seam and hand-built room passes it (2.6f C9). */
export const NO_NEAR: Near = {
  heroes: new Set(),
  heroesBelow: new Set(),
  curios: new Set(),
  curiosBelow: new Set(),
};

/**
 * A weighted list less the kinds in `skip`, as far as the list allows
 * (2.6f C7): the entries whose kind is not in `skip`, or the whole list
 * when that would leave none, so a pick never comes up empty because of
 * its neighbours. Order and weights are kept.
 */
export function skipNear<K>(
  picks: readonly (readonly [K, number])[],
  skip: ReadonlySet<K>,
): readonly (readonly [K, number])[] {
  const left = picks.filter(([k]) => !skip.has(k));
  return left.length > 0 ? left : picks;
}

/**
 * True when a floor prop's box stands on the room's floor: every cell under
 * the box (a box ending exactly on a cell border does not reach the next
 * cell) is floor inside the hall or a bay. That rules out void, the doorway
 * columns (they lie between the hall and the bays, and between the corridor
 * and the hall) and the corridor. An empty box fits nowhere.
 */
export function fitsFloor(room: SiteBase, box: Box): boolean {
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
 * The wall edge the deck and bay stencil goes on (2.7 C19): the entrance's
 * east neighbour, `{ x: entrance.x + 1, y: entrance.y, side: "s" }`, which
 * is in `noRun`, so no wall prop or run takes it. The stencil is laid there
 * only when it is a wall edge and no fixture's (`placeDecals`).
 */
export function stencilEdge(room: Pick<SiteBase, "entrance">): WallSlot {
  return { x: room.entrance.x + 1, y: room.entrance.y, side: "s" };
}

/**
 * The clear strip in front of the wall stencil's edge (2.7 C19), as a
 * `footprint` size: a poster's viewing lane, `SHEET_LANE_WIDTH` along the
 * wall by `SHEET_LANE_DEPTH` out, wider than the stencil's 0.9 m. It is one
 * of the lanes, so floor props and heroes keep out of it.
 */
export const STENCIL_STRIP = {
  along: SHEET_LANE_WIDTH,
  out: SHEET_LANE_DEPTH,
} as const;

/**
 * Where props may go in a room: runs, free edges, lanes, taken footprints,
 * corner zones, wall-side cells, long walls, cluster blocks and span lines.
 * See the module doc for the rules.
 */
export function dressingSites(room: SiteBase): DressingSites {
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
  const way = { along: LANE_WIDTH, out: LANE_DEPTH };
  const use = { along: LANE_WIDTH, out: USE_LANE_DEPTH };
  const sheet = { along: SHEET_LANE_WIDTH, out: SHEET_LANE_DEPTH };
  for (const f of room.fixtures) {
    switch (f.kind) {
      case "door":
      case "hatch":
      case "portal":
        lanes.push(footprint(f.slot, way));
        break;
      case "terminal":
      case "machine":
        lanes.push(footprint(f.slot, use));
        break;
      case "poster":
      case "placard":
        lanes.push(footprint(f.slot, sheet));
        break;
    }
  }
  const sign = stencilEdge(room);
  const signKey = edgeKey(sign);
  if (wallEdges.has(signKey) && !fixtureEdges.has(signKey))
    lanes.push(footprint(sign, STENCIL_STRIP));
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

  const clusterBlocks: ClusterBlock[] = [];
  const band = isLargeHall(hall) ? interiorBand(hall) : null;
  if (band !== null)
    for (let by = band.y0; by + CLUSTER_BLOCK <= band.y1; by += CLUSTER_BLOCK)
      for (
        let bx = band.x0;
        bx + CLUSTER_BLOCK <= band.x1;
        bx += CLUSTER_BLOCK
      ) {
        const cells: FloorSpot[] = [];
        for (let y = by + 1; y <= by + CLUSTER_INNER; y++)
          for (let x = bx + 1; x <= bx + CLUSTER_INNER; x++)
            if (isFloor(room.grid, x, y))
              cells.push({ cx: x, cy: y, wall: null, zone: null });
        clusterBlocks.push({ key: cellKey(bx, by), x: bx, y: by, cells });
      }

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
    clusterBlocks,
    spanLines: spanLines(room),
  };
}

/**
 * The clear span lines of a large hall (D9), rows first and then columns;
 * none in a hall that is not large. See the module doc's "Span lines".
 */
function spanLines(room: SiteBase): SpanLine[] {
  const hall = room.hall;
  if (!isLargeHall(hall)) return [];
  const solid: Box[] = [];
  for (const d of room.decor) {
    const box = decorFootprint(d);
    if (box !== null) solid.push(box);
    const run = pipeRunBox(d, hall);
    if (run !== null) solid.push(run);
  }
  solid.push(...room.scaffold, ...lampBoxes(room));
  const lines: SpanLine[] = [];
  const keep = (line: SpanLine) => {
    const clear = line.segments.every((c) => {
      const b = spanBox(line.axis, c);
      return !solid.some((s) => overlaps(b, s));
    });
    if (clear) lines.push(line);
  };
  for (let y = hall.y0 + 1; y <= hall.y1 - 2; y++) {
    const segments: SpanLine["segments"] = [];
    for (
      let x = hall.x0 + 1;
      x + SPAN_CELLS - 1 <= hall.x1 - 2;
      x += SPAN_CELLS
    )
      segments.push({ x, y });
    keep({ axis: "x", index: y, segments });
  }
  for (let x = hall.x0 + 1; x <= hall.x1 - 2; x++) {
    const segments: SpanLine["segments"] = [];
    for (
      let y = hall.y0 + 1;
      y + SPAN_CELLS - 1 <= hall.y1 - 2;
      y += SPAN_CELLS
    )
      segments.push({ x, y });
    keep({ axis: "y", index: x, segments });
  }
  return lines;
}

/**
 * The plan box of a span segment whose first cell is `c`, in metres (D9):
 * `SPAN_CELLS` cells along its axis from the cell's west (axis "x") or
 * north (axis "y") edge, and `SPAN_HALF` either side of the cell's middle
 * across it. `sites.ts` keeps it clear of decor, pipe runs, scaffolding and
 * lamps.
 */
export function spanBox(axis: "x" | "y", c: { x: number; y: number }): Box {
  return axis === "x"
    ? {
        x0: c.x * CELL,
        x1: (c.x + SPAN_CELLS) * CELL,
        z0: (c.y + 0.5) * CELL - SPAN_HALF,
        z1: (c.y + 0.5) * CELL + SPAN_HALF,
      }
    : {
        x0: (c.x + 0.5) * CELL - SPAN_HALF,
        x1: (c.x + 0.5) * CELL + SPAN_HALF,
        z0: c.y * CELL,
        z1: (c.y + SPAN_CELLS) * CELL,
      };
}

/**
 * The hall's interior band: the hall less `BAND_MARGIN` cells on every
 * side, the band `decorFor` stands its furniture in and `scaffoldFor` draws
 * its frames inside. Null when nothing is left, which is a hall narrower or
 * shallower than `2 * BAND_MARGIN + 1` cells. The density measure counts
 * the floor props in it (`density.ts`), and the mid-hall clusters (D6)
 * are tiled across it.
 */
export function interiorBand(hall: Rect): Rect | null {
  const band = {
    x0: hall.x0 + BAND_MARGIN,
    y0: hall.y0 + BAND_MARGIN,
    x1: hall.x1 - BAND_MARGIN,
    y1: hall.y1 - BAND_MARGIN,
  };
  return band.x1 > band.x0 && band.y1 > band.y0 ? band : null;
}

/** A large hall's shorter side, at least, in cells (D5). */
const LARGE_SHORT = 8;
/** A large hall's longer side, at least, in cells (D5). */
const LARGE_LONG = 9;

/**
 * True when a hall is large (D5): its shorter side is at least 8 cells and
 * its longer side at least 9, either way round. Its interior band is then
 * at least 5 by 4 cells, enough for one cluster block. The workshop
 * (13x12), the hub (24x24) and the fullest probed room qualify; the bridge
 * (7x6), the empty hall (5x6) and the narrow hall (5x12) do not. Only a
 * large hall gets mid-hall clusters and spans, and the density ceiling of
 * `dress.test.ts` is held against large halls alone.
 */
export function isLargeHall(hall: Rect): boolean {
  const w = hall.x1 - hall.x0;
  const d = hall.y1 - hall.y0;
  return Math.min(w, d) >= LARGE_SHORT && Math.max(w, d) >= LARGE_LONG;
}

/** The order a floor cell's walls are tried in for a wall-side spot. */
const SIDES: readonly Side[] = ["n", "e", "s", "w"];

function cellKey(x: number, y: number) {
  return `${x},${y}`;
}

/** True when cell `(x, y)` lies in rectangle `r` (`x1` and `y1` exclusive). */
export function inside(r: Rect, x: number, y: number): boolean {
  return x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1;
}

/** The hall and then each bay, west to east, as the room states them. */
function roomRects(room: SiteBase): Rect[] {
  return [room.hall, ...room.bays];
}
