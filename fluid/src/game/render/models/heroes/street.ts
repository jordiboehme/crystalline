/**
 * The street pieces: the red bike and the blue police box.
 *
 * Blockouts until Task 6 of the 2.6c plan: every kind here is
 * `heroBlockout` for now.
 */

import type { HeroKind } from "../../../world/types";
import { heroBlockout, type HeroRecipe } from "./common";

/** The street kinds' recipes. */
export const STREET_RECIPES = {
  "red-bike": heroBlockout,
  "police-box": heroBlockout,
} satisfies Record<Extract<HeroKind, "red-bike" | "police-box">, HeroRecipe>;
