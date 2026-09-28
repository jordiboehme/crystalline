/**
 * The hangar's structure as floor geometry (M3 C15 to C17): the pads' boxes,
 * the gantry legs' wall edges and boxes, and where the pad stencils lie.
 * Pure arithmetic on a `HangarSpec`, shared by the site rules
 * (`dressingSites`: the pads and the legs are taken boxes, the leg edges
 * carry nothing), the decals (`placeDecals` lays the pad stencils) and the
 * room mesh (Task 8 draws the pads and the gantries from the same numbers).
 *
 * - A pad's box is its cells in metres.
 * - Each gantry stands on a leg at each end of its beam: on the west wall
 *   edge `(0, y, w)` and the east wall edge `(width - 1, y, e)` of its row.
 *   A leg's box is `GANTRY_LEG` against that edge, centred on the row.
 * - A pad's stencil lies 1 m inside its south rim, centred across it,
 *   reading from the south; the pads are lettered 1, 2, ... in pad order.
 *
 * A leaf: it imports `types.ts` and `units.ts` only, so `sites.ts` can
 * import it without a cycle through the modules that import `sites.ts`.
 */

import type { Box, HangarSpec, Pad, RoomSpec, WallSlot } from "./types";
import { CELL } from "./units";

/**
 * A gantry leg's floor footprint in metres (M3 C15): `along` its wall and
 * `out` from it. The legged frame (two posts, cross-braced) stands flush
 * on the wall line and takes no collision; the box only keeps the
 * dressing out of it.
 */
export const GANTRY_LEG = { along: 1.0, out: 0.6 } as const;

/** A pad's floor box in metres: its cells, `x1` and `y1` exclusive. */
export function padBox(p: Pad): Box {
  return {
    x0: p.x0 * CELL,
    x1: p.x1 * CELL,
    z0: p.y0 * CELL,
    z1: p.y1 * CELL,
  };
}

/**
 * The wall edges the gantry legs stand on (M3 C15): for each gantry in
 * order, its west end `(0, y, w)` then its east end `(width - 1, y, e)`.
 * None on a room with no hangar.
 */
export function gantryLegEdges(
  room: Pick<RoomSpec, "width" | "hangar">,
): WallSlot[] {
  if (room.hangar === undefined) return [];
  return room.hangar.gantries.flatMap((g): WallSlot[] => [
    { x: 0, y: g.y, side: "w" },
    { x: room.width - 1, y: g.y, side: "e" },
  ]);
}

/**
 * The gantry legs' floor boxes in metres, in `gantryLegEdges` order: each
 * `GANTRY_LEG` against its wall, centred on its row. None on a room with
 * no hangar.
 */
export function gantryLegs(room: Pick<RoomSpec, "width" | "hangar">): Box[] {
  return gantryLegEdges(room).map((e) => {
    const z = (e.y + 0.5) * CELL;
    const wall = e.side === "w" ? e.x * CELL : (e.x + 1) * CELL;
    const inward = e.side === "w" ? GANTRY_LEG.out : -GANTRY_LEG.out;
    return {
      x0: Math.min(wall, wall + inward),
      x1: Math.max(wall, wall + inward),
      z0: z - GANTRY_LEG.along / 2,
      z1: z + GANTRY_LEG.along / 2,
    };
  });
}

/**
 * Where each pad's stencil lies (M3 C17), in cell units: centred across
 * the pad, 1 m (half a cell) inside its south rim, reading from the south;
 * `letter` 1 for the first pad, 2 for the second, and so on.
 */
export function padStencilSpots(
  hangar: HangarSpec,
): { x: number; y: number; letter: number }[] {
  return hangar.pads.map((p, i) => ({
    x: (p.x0 + p.x1) / 2,
    y: p.y1 - 1 / CELL,
    letter: i + 1,
  }));
}
