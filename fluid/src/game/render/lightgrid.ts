/**
 * The room's light as a grid: one texel per cell, holding the level of the
 * light zone the cell belongs to.
 *
 * Milestone 1 handed the shader every zone's rectangle and level as uniform
 * arrays and let each fragment search them, which capped a room at 16
 * zones. A hub with bays and a corridor has far more. Here the zones are
 * resolved once per room into `zoneOfCell`, and every tick
 * `fillLightTexels` writes the zones' current levels into a small R8 byte
 * array, one byte per cell, which the renderer uploads as a texture and the
 * shader samples at the fragment's cell. The cost no longer grows with the
 * number of zones.
 *
 * Cells are stored row by row, cell `(x, y)` at `y * width + x`, with the
 * grid's row 0 (north) first. That is also the texture's first row, so the
 * shader's `floor(world.xz / CELL)` indexes it directly, with no flip.
 */

import { isFloor } from "../world/layout";
import type { RoomSpec } from "../world/types";

/**
 * Which zone lights each cell of a room: `width` by `depth` cells as the
 * room's grid, and per cell (row by row) the index of its zone in
 * `room.lights`, or -1 for a void cell.
 */
export interface LightGrid {
  width: number;
  depth: number;
  zoneOfCell: Int16Array;
}

/**
 * The light grid of a room. Every floor cell takes the first zone in
 * `room.lights` whose rectangle contains it (the generator's zones tile the
 * grid, so there is exactly one); a floor cell no zone covers, which the
 * generator never makes, takes -1 like a void cell and stays dark.
 */
export function lightGrid(room: RoomSpec): LightGrid {
  const { width, depth } = room;
  const zoneOfCell = new Int16Array(width * depth).fill(-1);
  room.lights.forEach((z, i) => {
    for (let y = Math.max(0, z.y0); y < Math.min(depth, z.y1); y++) {
      for (let x = Math.max(0, z.x0); x < Math.min(width, z.x1); x++) {
        const cell = y * width + x;
        if (isFloor(room.grid, x, y) && zoneOfCell[cell] === -1)
          zoneOfCell[cell] = i;
      }
    }
  });
  return { width, depth, zoneOfCell };
}

/**
 * Writes one tick's light into `out`, one byte per cell of the grid: the
 * zone's level from `levels` (DOOM's 0 to 255 scale, in `room.lights`
 * order, as `LightState.levels` holds them), rounded and clamped to 0..255,
 * and 0 for a void cell. `out` must hold `width * depth` bytes; it is
 * written in place so the renderer can reuse one array every tick.
 */
export function fillLightTexels(
  grid: LightGrid,
  levels: Float32Array,
  out: Uint8Array,
): void {
  const cells = grid.width * grid.depth;
  for (let i = 0; i < cells; i++) {
    const zone = grid.zoneOfCell[i] ?? -1;
    const level = zone < 0 ? 0 : Math.round(levels[zone] ?? 0);
    out[i] = Math.max(0, Math.min(255, level));
  }
}
