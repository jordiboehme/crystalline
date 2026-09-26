/**
 * The finds' recipes (2.6d): the treasure radar, the capsule case and the
 * reactor case. Each is the checked blockout (`curioBlockout`) until its
 * model task replaces it.
 */

import type { CurioKind } from "../../../world/types";
import { curioBlockout, type CurioRecipe } from "./common";

/** The finds' recipes, each the blockout for now. */
export const FIND_RECIPES = {
  "treasure-radar": curioBlockout,
  "capsule-case": curioBlockout,
  "reactor-case": curioBlockout,
} satisfies Record<
  Extract<CurioKind, "treasure-radar" | "capsule-case" | "reactor-case">,
  CurioRecipe
>;
