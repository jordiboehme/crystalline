/**
 * The hangar's structure as floor geometry (M3 C15 to C17): the pads' boxes,
 * the gantry legs' wall edges and boxes, and where the pad stencils lie.
 * Pure arithmetic on a `HangarSpec`, read by the site rules
 * (`dressingSites`: the pads, the legs and the bay door's apron are taken
 * boxes, the leg edges carry nothing, the span lines keep off the beams)
 * and the decals (`placeDecals` lays the pad stencils). The numbers are the
 * structure's own, so whatever draws it can read them too.
 *
 * - A pad's box is its cells in metres.
 * - Each gantry stands on a leg at each end of its beam: on the west wall
 *   edge `(0, y, w)` and the east wall edge `(width - 1, y, e)` of its row.
 *   A leg's box is `GANTRY_LEG` against that edge, centred on the row.
 * - A gantry's beam in plan is its truss and its catwalk (`GANTRY_BEAM`),
 *   wall to wall; no ceiling span crosses or runs along it.
 * - The bay door's apron is the floor strip `BAY_APRON` deep in front of
 *   its span.
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

/**
 * A gantry's beam in plan, in metres (M3 C15): the truss `half` either side
 * of its row's centre line and the catwalk `catwalk` wide along its south
 * side: the room the gantry takes overhead, which no ceiling span is hung
 * across or along (`gantryBeams`).
 */
export const GANTRY_BEAM = { half: 0.4, catwalk: 1.0 } as const;

/**
 * The gantries' beams in plan, in metres, one box per gantry in order: wall
 * to wall, from `GANTRY_BEAM.half` north of its row's centre line to the
 * catwalk's south edge. The span lines keep off them (`dressingSites`).
 * None on a room with no hangar.
 */
export function gantryBeams(room: Pick<RoomSpec, "width" | "hangar">): Box[] {
  if (room.hangar === undefined) return [];
  return room.hangar.gantries.map((g) => {
    const z = (g.y + 0.5) * CELL;
    return {
      x0: 0,
      x1: room.width * CELL,
      z0: z - GANTRY_BEAM.half,
      z1: z + GANTRY_BEAM.half + GANTRY_BEAM.catwalk,
    };
  });
}

/**
 * How deep the floor strip in front of the bay door is kept clear, in
 * metres (M3 C15): one cell, so no crate or trolley stands against the
 * bay door and it reads open from the lift.
 */
export const BAY_APRON = 2.0;

/**
 * The bay door's apron in metres: the floor strip in front of the bay
 * door's span of the north wall, `BAY_APRON` deep. A taken box
 * (`dressingSites`), so nothing stands on it.
 */
export function bayDoorApron(hangar: HangarSpec): Box {
  return {
    x0: hangar.bayDoor.x0 * CELL,
    x1: hangar.bayDoor.x1 * CELL,
    z0: 0,
    z1: BAY_APRON,
  };
}

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
