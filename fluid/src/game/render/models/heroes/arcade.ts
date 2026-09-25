/**
 * The arcade heroes' recipes: the arcade cabinet and the recruit cabinet.
 * What they share is the cabinet: a tall body with side art, a marquee, a
 * control deck with a stick and buttons, and an attract screen that swaps
 * between its game's title card and its demo (H14). A cabinet's variant is
 * its game.
 *
 * Every kind is a checked blockout (`heroBlockout`) for now; its model
 * task replaces the entries of this table and touches nothing else.
 */

import type { HeroKind } from "../../../world/types";
import { heroBlockout, type HeroRecipe } from "./common";

/** The arcade kinds' recipes. */
export const ARCADE_RECIPES = {
  "arcade-cabinet": heroBlockout,
  "recruit-cabinet": heroBlockout,
} satisfies Record<
  Extract<HeroKind, "arcade-cabinet" | "recruit-cabinet">,
  HeroRecipe
>;
