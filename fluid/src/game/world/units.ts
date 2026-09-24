/**
 * The station's one unit of floor: how many metres a grid cell is.
 *
 * It lives in a module of its own, with no import at all, so the generator,
 * the footprints, the walking code and the renderer can all read it without
 * any of them importing another just for a number. `footprints.test.ts`
 * keeps this module free of imports.
 */

/** Metres per cell. */
export const CELL = 2;
