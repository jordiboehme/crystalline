/**
 * The critters' recipes (2.6d): the hover drone, built from its lift up
 * (`CURIO_LIFT`), and the soot puffs on the floor. Each is the checked
 * blockout (`curioBlockout`) until its model task replaces it.
 */

import type { CurioKind } from "../../../world/types";
import { curioBlockout, type CurioRecipe } from "./common";

/** The critters' recipes, each the blockout for now. */
export const CRITTER_RECIPES = {
  "hover-drone": curioBlockout,
  "soot-puffs": curioBlockout,
} satisfies Record<
  Extract<CurioKind, "hover-drone" | "soot-puffs">,
  CurioRecipe
>;
