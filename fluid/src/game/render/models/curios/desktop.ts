/**
 * The desktop curios' recipes (2.6d): the breadbin computer, the slim
 * computer and the space bricks. Each is the checked blockout
 * (`curioBlockout`) until its model task replaces it.
 */

import type { CurioKind } from "../../../world/types";
import { curioBlockout, type CurioRecipe } from "./common";

/** The desktop curios' recipes, each the blockout for now. */
export const DESKTOP_RECIPES = {
  "breadbin-computer": curioBlockout,
  "slim-computer": curioBlockout,
  "space-bricks": curioBlockout,
} satisfies Record<
  Extract<CurioKind, "breadbin-computer" | "slim-computer" | "space-bricks">,
  CurioRecipe
>;
