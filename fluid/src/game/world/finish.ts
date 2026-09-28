/**
 * A room's finish (2.7 C8, C11): the accent it is picked out in and the
 * wall pattern of each of its spaces.
 *
 * The accent is an index into the look's accent set (`Look.accents`, five
 * colours per look, index `i` in the same colour family in every look), so
 * a room keeps its colour family in every look while the colour itself
 * follows the look (fixed in play; only a dev page picks another). The
 * renderer uploads the look's accent at that index (`accentFor` in
 * `render/looks.ts`), and every surface carrying the
 * accent mark (`accentTint` in `render/geometry.ts`) takes it.
 *
 * The wall patterns are `WALL_PATTERNS` texture and uv-scale sets for the
 * shell's wall quads. The hall takes one by seed; each overflow bay and the
 * backlink corridor take one of the two others, so a bay always reads as a
 * space of its own. A bay's pick is keyed by its own index, never by how
 * many bays there are, so a room that gains a bay keeps the patterns of the
 * bays it had.
 *
 * Every pick is `seedFor` of the room's seed and a name (2.7 Global
 * Constraints): no pick draws from an existing stream, so none moves
 * anything else of the room. A leaf module: it imports `core/seed.ts` and
 * `variants.ts` (itself a leaf) only, and `types.ts` re-states `Finish` as
 * the shape `RoomSpec.finish` carries.
 */

import { seedFor } from "../core/seed";
import type { Finish } from "./types";
import { ACCENT_COUNT } from "./variants";

/**
 * How many wall patterns there are (2.7 C11): 0 panels (the bevelled
 * squares), 1 ribbed and 2 plated.
 */
export const WALL_PATTERNS = 3;

/**
 * One of the two patterns other than the hall's, by `pick` (0 or 1): the
 * lower of the two for 0, the higher for 1.
 */
function otherPattern(hall: number, pick: number): number {
  const other = pick % (WALL_PATTERNS - 1);
  return other < hall ? other : other + 1;
}

/**
 * A generated room's finish from its seed and its number of overflow bays
 * (2.7 C8, C11): the accent `seedFor(seed, "accent") % ACCENT_COUNT`, the
 * hall's pattern `seedFor(seed, "walls", "hall") % WALL_PATTERNS`, each
 * bay `i`'s one of the two others by `seedFor(seed, "walls", "bay", i)`,
 * and the corridor's one of the two others by
 * `seedFor(seed, "walls", "corridor")`. A room with no corridor still
 * carries the corridor's pick, so the finish has one shape for every room.
 */
export function finishFor(seed: number, bays: number): Finish {
  const hallWalls = seedFor(seed, "walls", "hall") % WALL_PATTERNS;
  return {
    accent: seedFor(seed, "accent") % ACCENT_COUNT,
    hallWalls,
    bayWalls: Array.from({ length: bays }, (_, i) =>
      otherPattern(hallWalls, seedFor(seed, "walls", "bay", i)),
    ),
    corridorWalls: otherPattern(hallWalls, seedFor(seed, "walls", "corridor")),
  };
}

/**
 * The finish of a hand-built room (the gallery, the hero hall, the console
 * room): accent 0 and pattern 0 on every wall, one entry per bay.
 */
export function plainFinish(bays: number): Finish {
  return {
    accent: 0,
    hallWalls: 0,
    bayWalls: Array.from({ length: bays }, () => 0),
    corridorWalls: 0,
  };
}
