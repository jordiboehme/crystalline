/**
 * The other rare props (2.6d): the ooze canister cluster, two standing
 * canisters and a tipped one (variant 0) or one of each (variant 1), with
 * a glowing puddle inside its footprint (C14); the poster, a flat sheet on
 * its wall; and the designer tower, a slim PC tower that stands beside a
 * desk (C13).
 *
 * Each is the checked blockout (`rareBlockout`) for now, until its model
 * task replaces it. The floor kinds take their box from their footprint
 * (`FOOTPRINTS.prop`) and their height from C3; the poster's sheet hangs
 * from `POSTER_H` on its wall, `POSTER_DEPTH` deep. The canisters'
 * blockout carries a blink cap, since their bank is `breathe`
 * (`PROP_BANK`, C15).
 */

import { FOOTPRINTS } from "../../../world/footprints";
import type { FloorPropKind, PropKind } from "../../../world/types";
import { rareBlockout, type PropRecipe } from "./common";

/** The canister cluster's height, in metres (C3). */
const CANISTERS_H = 0.42;

/** The designer tower's height, in metres (C3). */
const TOWER_H = 0.62;

/** The poster sheet's half width along its wall, in metres (C3: 0.6 wide). */
const POSTER_HALF = 0.3;

/** How far the poster sheet stands off its wall, in metres (C3: at most 0.05). */
const POSTER_DEPTH = 0.02;

/** The poster sheet's bottom and top edge, in metres (C11). */
const POSTER_H = [1.1, 2.0] as const;

/** A floor kind's blockout over its variant's footprint, from the floor up to `h`. */
function floorBlockout(
  r: Parameters<PropRecipe>[0],
  kind: FloorPropKind,
  h: number,
): void {
  const size = FOOTPRINTS.prop[kind][r.variant];
  if (size === undefined)
    throw new Error(`${kind}: no variant ${String(r.variant)}`);
  rareBlockout(
    r.k,
    r.s,
    kind,
    [-size.width / 2, size.width / 2],
    [-size.depth / 2, size.depth / 2],
    [0, h],
  );
}

/** The other rare props' recipes, each the blockout for now. */
export const RARE_RECIPES = {
  "ooze-canisters": (r) => {
    floorBlockout(r, "ooze-canisters", CANISTERS_H);
  },
  "designer-tower": (r) => {
    floorBlockout(r, "designer-tower", TOWER_H);
  },
  "saucer-poster": ({ k, s, kind }) => {
    rareBlockout(
      k,
      s,
      kind,
      [-POSTER_HALF, POSTER_HALF],
      [0, POSTER_DEPTH],
      POSTER_H,
    );
  },
} satisfies Record<
  Extract<PropKind, "ooze-canisters" | "saucer-poster" | "designer-tower">,
  PropRecipe
>;
