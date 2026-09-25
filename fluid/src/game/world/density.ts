/**
 * The density measure: a handful of numbers that say how full a dressed
 * room is, so the tests can pin the set dressing's density between a floor
 * (the change is visible) and a ceiling (the room stays walkable). It reads
 * a finished `RoomSpec` and changes nothing.
 *
 * What each number means, and which test pins it (`dress.test.ts`,
 * `describe("density measure")`, over 60 workshop seeds in every
 * archetype unless a line says otherwise):
 *
 * - `floorPer100`, floor props per 100 floor cells of the hall and the
 *   bays: at most 22 in every large dressed room, checked on the canned
 *   workshop and hub in every archetype and condition, the canned bridge
 *   (never large), all 300 seed rooms and the over-cap room ("keeps every
 *   large dressed room at or under 22 floor props per 100 floor cells").
 *   The same measure has floors: at least 13 summed over the seeds, and at
 *   least 13 in the clean canned hub of every archetype (E9).
 * - `wallSideProps / wallSideSpots`, the floor props standing in a
 *   wall-side spot per spot, summed over the seeds: at least 0.15 and at
 *   most 0.35 ("puts at least 0.15 floor props per wall-side spot").
 * - `tallProps / floorProps`, the share of floor props whose kind is
 *   `tall` (E5), summed over the seeds: at least 0.25 ("makes at least
 *   0.25 of the floor props tall").
 * - `wideEdges / freeEdges`, the free wall edges carrying a wide wall prop,
 *   summed over the seeds: at least 0.12 and at most 0.6 ("covers at least
 *   0.12 of free edges with wide wall props").
 * - `bandProps`, the floor props inside the hall's interior band, where the
 *   mid-hall clusters stand, and `spanSegments`, the span segments hung
 *   across the hall: measured, pinned where the clusters and spans are.
 *
 * This is the generator side: it imports only `props.ts`, `sites.ts`,
 * `layout.ts` and `types.ts`, and never `move.ts` or `render/`.
 */

import { isFloor } from "./layout";
import { PROP_CATALOGUE } from "./props";
import { dressingSites, interiorBand, isLargeHall } from "./sites";
import type { Prop, RoomSpec } from "./types";

/** How full a dressed room is. See the module doc for each number's test. */
export interface Density {
  /** The hall is large (`isLargeHall`). */
  large: boolean;
  /** Floor cells of the hall and the bays (the corridor is never dressed). */
  floorCells: number;
  floorProps: number;
  /** Floor props whose kind is `tall` in `PROP_CATALOGUE` (E5). */
  tallProps: number;
  /** Floor props per 100 floor cells. */
  floorPer100: number;
  /** Wall-side spots (`dressingSites(room).wallSide`) and the floor props standing in one. */
  wallSideSpots: number;
  wallSideProps: number;
  /** Floor props inside the interior band: the clusters. */
  bandProps: number;
  /** Free wall edges, and those carrying a wide wall prop. */
  freeEdges: number;
  wideEdges: number;
  /** wideEdges / freeEdges, 0 when there are none. */
  wideShare: number;
  /** Span segments. */
  spanSegments: number;
}

/**
 * Measures a dressed room: its floor props against its floor cells, its
 * wall-side spots and interior band, and its free wall edges against the
 * wide wall props on them. A floor prop belongs to the cell its anchor is
 * in; a wall prop takes one free edge, so `wideEdges` is the count of wide
 * wall props. Pure: the same room gives the same numbers.
 */
export function measureDensity(room: RoomSpec): Density {
  const sites = dressingSites(room);
  const cellOf = (p: Prop) =>
    `${String(Math.floor(p.x))},${String(Math.floor(p.y))}`;
  const floor = room.props.filter((p) => p.anchor === "floor");
  let floorCells = 0;
  for (const r of [room.hall, ...room.bays])
    for (let y = r.y0; y < r.y1; y++)
      for (let x = r.x0; x < r.x1; x++)
        if (isFloor(room.grid, x, y)) floorCells++;
  const wallSide = new Set(
    sites.wallSide.map((s) => `${String(s.cx)},${String(s.cy)}`),
  );
  const band = interiorBand(room.hall);
  const inBand = (p: Prop) =>
    band !== null &&
    p.x >= band.x0 &&
    p.x < band.x1 &&
    p.y >= band.y0 &&
    p.y < band.y1;
  const wide = room.props.filter(
    (p) => p.anchor === "wall" && PROP_CATALOGUE[p.kind].wide,
  ).length;
  return {
    large: isLargeHall(room.hall),
    floorCells,
    floorProps: floor.length,
    tallProps: floor.filter((p) => PROP_CATALOGUE[p.kind].tall).length,
    floorPer100: floorCells === 0 ? 0 : (floor.length * 100) / floorCells,
    wallSideSpots: sites.wallSide.length,
    wallSideProps: floor.filter((p) => wallSide.has(cellOf(p))).length,
    bandProps: floor.filter(inBand).length,
    freeEdges: sites.free.size,
    wideEdges: wide,
    wideShare: sites.free.size === 0 ? 0 : wide / sites.free.size,
    spanSegments: room.props.filter((p) => PROP_CATALOGUE[p.kind].span).length,
  };
}
