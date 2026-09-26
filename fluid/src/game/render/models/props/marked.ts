/**
 * The rare props that carry a painted mark (2.6d): the marked crate, a
 * crate with the maker's round mark on its faces that the rare step's
 * mark relabels from one accepted crate (C12), alone (variant 0) or on
 * top of a crate stack (variant 1), and the gravity console, a standing
 * wall-side console with a readout and a few lit buttons (C13).
 *
 * Each is the checked blockout (`rareBlockout`) for now, a box over its
 * variant's footprint (`FOOTPRINTS.prop`) up to its height (C3), until its
 * model task replaces it. The console's blockout carries a blink cap, since
 * its bank is `status` (`PROP_BANK`, C15).
 */

import { FOOTPRINTS } from "../../../world/footprints";
import type { FloorPropKind } from "../../../world/types";
import { rareBlockout, type PropRecipe } from "./common";

/** The marked crate's height by variant, in metres (C3): alone, and on its stack. */
const MARKED_CRATE_H = [0.8, 1.65] as const;

/** The gravity console's height, in metres (C3). */
const CONSOLE_H = 1.15;

/** A floor kind's footprint box, centred on the anchor, as `a` and `d` ranges. */
function footprintBox(
  kind: FloorPropKind,
  variant: number,
): { a: readonly [number, number]; d: readonly [number, number] } {
  const size = FOOTPRINTS.prop[kind][variant];
  if (size === undefined)
    throw new Error(`${kind}: no variant ${String(variant)}`);
  return {
    a: [-size.width / 2, size.width / 2],
    d: [-size.depth / 2, size.depth / 2],
  };
}

/** The marked crate, a blockout over its variant's footprint and height. */
const markedCrate: PropRecipe = ({ k, s, variant, kind }) => {
  const { a, d } = footprintBox("marked-crate", variant);
  const h = MARKED_CRATE_H[variant];
  if (h === undefined)
    throw new Error(`${kind}: no variant ${String(variant)}`);
  rareBlockout(k, s, kind, a, d, [0, h]);
};

/** The gravity console, a blockout over its footprint and height. */
const gravityConsole: PropRecipe = ({ k, s, variant, kind }) => {
  const { a, d } = footprintBox("gravity-console", variant);
  rareBlockout(k, s, kind, a, d, [0, CONSOLE_H]);
};

/** The marked rare props' recipes, each the blockout for now. */
export const MARKED_RECIPES = {
  "marked-crate": markedCrate,
  "gravity-console": gravityConsole,
} satisfies Record<
  Extract<FloorPropKind, "marked-crate" | "gravity-console">,
  PropRecipe
>;
