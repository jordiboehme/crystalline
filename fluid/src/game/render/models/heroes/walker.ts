/**
 * The slab walker: a four-slab robot standing mid-stride (2.6d C17). Its
 * blockout, `heroBlockout` (`common.ts`), stands until Task 9 gives it its
 * own recipe.
 */

import type { HeroKind } from "../../../world/types";
import { heroBlockout, type HeroRecipe } from "./common";

/** The slab walker's recipe. */
export const WALKER_RECIPES = {
  "slab-walker": heroBlockout,
} satisfies Record<Extract<HeroKind, "slab-walker">, HeroRecipe>;
