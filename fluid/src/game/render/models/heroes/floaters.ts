/**
 * The hovering heroes (C4): the question block, the hoverboard and the flying
 * cloud. Each is built at its lift (`heroHalf`'s `lift`), so its lowest
 * vertex is the height it hovers at.
 *
 * Blockouts until Task 3 of the 2.6c plan: every kind here is
 * `heroBlockout` for now.
 */

import type { HeroKind } from "../../../world/types";
import { heroBlockout, type HeroRecipe } from "./common";

/** The floating kinds' recipes. */
export const FLOATER_RECIPES = {
  "question-block": heroBlockout,
  hoverboard: heroBlockout,
  "flying-cloud": heroBlockout,
} satisfies Record<
  Extract<HeroKind, "question-block" | "hoverboard" | "flying-cloud">,
  HeroRecipe
>;
