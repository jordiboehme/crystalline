/**
 * Where a room's lamps hang: one ceiling light panel per light zone, and its
 * plan box. The airlock is the one exception: the renderer hangs no panel
 * there, since its iris light is its lamp (M3 C24). The renderer builds the panels from `lampCentre` and the lamp
 * size (`render/geometry.ts` imports them from here), and the dressing
 * keeps its ceiling spans clear of `lampBoxes`, so both sides read one
 * definition and a span never hides a lamp (D10).
 *
 * This is the generator side: it imports only `layout.ts`, `types.ts` and
 * `units.ts`, and never `move.ts` or `render/` (ruling 20).
 * `lamps.test.ts` keeps it so.
 */

import { doorwayColumns, isFloor } from "./layout";
import type { Box, RoomSpec } from "./types";
import { CELL } from "./units";

/** The lamp panel's half width (along x), in metres. */
export const LAMP_HALF_W = 0.8;
/** The lamp panel's half depth (along z), in metres. */
export const LAMP_HALF_D = 0.3;

/**
 * Where a zone's lamp hangs: centred on the zone's floor cells, when the
 * whole panel lies over floor that is not a doorway; otherwise over the
 * middle of the zone's floor cell nearest that centre, so a zone of odd
 * shape never hangs its lamp over a wall or across a lintel. Null for a
 * zone with no floor, which the generator never makes. The centre is in
 * metres, `[x, z]`.
 */
export function lampCentre(
  room: Pick<RoomSpec, "grid">,
  zone: { x0: number; y0: number; x1: number; y1: number },
  doorways: Set<number>,
): [number, number] | null {
  const cells: [number, number][] = [];
  for (let y = zone.y0; y < zone.y1; y++)
    for (let x = zone.x0; x < zone.x1; x++)
      if (isFloor(room.grid, x, y) && !doorways.has(x)) cells.push([x, y]);
  if (cells.length === 0) return null;
  const cx = (cells.reduce((a, [x]) => a + x, 0) / cells.length + 0.5) * CELL;
  const cz = (cells.reduce((a, [, y]) => a + y, 0) / cells.length + 0.5) * CELL;
  const open = (px: number, pz: number) => {
    const x = Math.floor(px / CELL);
    return isFloor(room.grid, x, Math.floor(pz / CELL)) && !doorways.has(x);
  };
  const fits = [-1, 1].every((sx) =>
    [-1, 1].every((sz) =>
      open(cx + sx * (LAMP_HALF_W - 1e-3), cz + sz * (LAMP_HALF_D - 1e-3)),
    ),
  );
  if (fits) return [cx, cz];
  let best: [number, number] = [cx, cz];
  let bestD = Infinity;
  for (const [x, y] of cells) {
    const mx = (x + 0.5) * CELL;
    const mz = (y + 0.5) * CELL;
    const d = Math.hypot(mx - cx, mz - cz);
    if (d < bestD) [best, bestD] = [[mx, mz], d];
  }
  return best;
}

/**
 * The plan box of every zone's lamp, in metres, in `room.lights` order: the
 * panel `LAMP_HALF_W` either side of its centre along x and `LAMP_HALF_D`
 * along z. A zone with no floor has no lamp and no box. The ceiling spans
 * (D9) keep clear of these, so a span never hangs under a lamp.
 */
export function lampBoxes(
  room: Pick<RoomSpec, "grid" | "hall" | "width" | "lights">,
): Box[] {
  const doorways = doorwayColumns(room);
  const out: Box[] = [];
  for (const z of room.lights) {
    const c = lampCentre(room, z, doorways);
    if (c === null) continue;
    out.push({
      x0: c[0] - LAMP_HALF_W,
      x1: c[0] + LAMP_HALF_W,
      z0: c[1] - LAMP_HALF_D,
      z1: c[1] + LAMP_HALF_D,
    });
  }
  return out;
}
