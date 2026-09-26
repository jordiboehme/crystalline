/**
 * The exhibits: the stone hand, the moon rocket and the thunder hammer,
 * each a single striking object standing free in the band.
 *
 * Blockouts until Task 4 of the 2.6c plan: every kind here is
 * `heroBlockout` for now.
 */

import type { HeroKind } from "../../../world/types";
import { heroBlockout, type HeroRecipe } from "./common";

/** The exhibit kinds' recipes. */
export const EXHIBIT_RECIPES = {
  "stone-hand": heroBlockout,
  "moon-rocket": heroBlockout,
  "thunder-hammer": heroBlockout,
} satisfies Record<
  Extract<HeroKind, "stone-hand" | "moon-rocket" | "thunder-hammer">,
  HeroRecipe
>;
