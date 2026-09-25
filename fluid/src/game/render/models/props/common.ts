/**
 * What every prop recipe shares: the envelope each anchor's props must
 * stay inside, the context a prop is built with and the shape of a recipe.
 *
 * A prop mesh is built once per kind, variant and look, in
 * `frameAt([0, 0, 0], 0)`, and drawn many times: the renderer turns it by
 * the instance's quarter turn (`turnPoint` and `TURN_XZ` in `../../kit.ts`)
 * and adds the instance's anchor. So a recipe works in the kit's local
 * `(a, d, h)` terms around the anchor and never sees the room:
 * - a wall prop's anchor is its wall edge's wall point at floor level, with
 *   `d` running into the room;
 * - a floor prop's anchor is the centre of its footprint on the floor;
 * - a ceiling prop's anchor is the wall point raised to the room's ceiling,
 *   so it hangs at negative `h`.
 *
 * The envelopes are plan ruling 5's height bands, chosen so no two layers
 * (wall props, wall runs, floor props, ceiling props) can intersect
 * whatever the room puts next to what. The prop test checks every kind,
 * variant and turn against them.
 *
 * Kept apart from `index.ts` so the recipe files can import it without an
 * import cycle through the dispatcher.
 */

import type { PropKind } from "../../../world/types";
import type { Kit } from "../../kit";
import type { Look } from "../../looks";
import type { KitAt, Surfaces } from "../common";

/**
 * How far along its wall a wall prop (or a ceiling prop that is not a run)
 * may reach either side of its anchor, in metres. A wall edge is one 2 m
 * cell, so 0.9 m keeps 0.2 m clear between the props of two neighbouring
 * edges and they never touch.
 */
export const WALL_REACH = 0.9;

/**
 * How far along its wall a run segment (cable tray, pipe bundle, duct,
 * ceiling tray) reaches either side of its anchor: the whole 2 m edge, so
 * the segments of neighbouring edges meet and read as one run (ruling 3).
 */
export const RUN_REACH = 1.0;

/**
 * The highest a wall prop reaches, in metres. The runs above it start at
 * `RUN_BAND.h0`, 2.45 m, so a wall prop and a run on the same edge never
 * meet.
 */
export const WALL_TOP = 2.4;

/**
 * The height band of a wall run, in metres: above every wall prop
 * (`WALL_TOP`) and under the lowest ceiling the generator makes, 3.0 m,
 * less `HEADROOM`, with room to spare for a ceiling prop's setback. Runs
 * skip every fixture edge (ruling 4), since tag strips and labels sit in
 * this band too.
 */
export const RUN_BAND = { h0: 2.45, h1: 2.9 } as const;

/**
 * The highest a floor prop reaches, in metres. It stays below the lowest
 * ceiling prop, which hangs at most `CEILING_DROP` under a 3.0 m ceiling
 * (2.25 m), even where a floor prop stands under one; its floor is its
 * footprint.
 */
export const FLOOR_TOP = 2.2;

/**
 * How far below the ceiling a ceiling prop may hang, in metres: its `h` runs
 * from `-CEILING_DROP` to `-HEADROOM`. The ceiling varies from 3.0 to 5.0 m
 * but a prop mesh is built once per look, so the height travels in the
 * instance, and 0.75 m keeps the lowest one at 2.25 m, over `FLOOR_TOP`.
 */
export const CEILING_DROP = 0.75;

/**
 * The least a ceiling prop stands out from its wall, in metres: past the
 * wall band (`FLUSH_DEPTH`, 0.3 m) so it never meets a wall run or a tall
 * wall prop under a low ceiling.
 */
export const CEILING_SETBACK = 0.35;

/**
 * The most a ceiling prop stands out from its wall, in metres: ceiling
 * props hang along the walls, where the ceiling runs go, and never over
 * the middle of a hall, so the ceiling props of facing walls stay well
 * apart in the narrowest hall.
 */
export const CEILING_OUT = 1.2;

/**
 * What a prop is built with: only the look, whose palette tints every
 * part. A prop takes no room, no ceiling and no text, since one mesh
 * serves every instance of its kind and variant in every room of that look.
 */
export interface PropContext {
  look: Look;
}

/**
 * One kind's recipe: builds variant `variant` of `kind` with `k`, a kit in
 * `frameAt([0, 0, 0], 0)`, and `kitAt` for any sub-frame a part needs. `s`
 * is the look's surfaces. A recipe must be a pure function of kind, variant
 * and look: no randomness seeded from anything else, since one mesh serves
 * every instance.
 */
export type PropRecipe = (r: {
  k: Kit;
  kitAt: KitAt;
  s: Surfaces;
  ctx: PropContext;
  variant: number;
  kind: PropKind;
}) => void;

/** The bevel of a blockout box, in metres. */
const BLOCKOUT_BEVEL = 0.02;

/**
 * A blockout, as in level design: the plain bevelled box a kind is drawn
 * as until its real model is built, inside the kind's envelope, so every
 * check runs on it from the start.
 */
export function blockout(
  k: Kit,
  s: Surfaces,
  a: readonly [number, number],
  d: readonly [number, number],
  h: readonly [number, number],
): void {
  k.bevelBox(a[0], a[1], d[0], d[1], h[0], h[1], BLOCKOUT_BEVEL, s.body);
}
