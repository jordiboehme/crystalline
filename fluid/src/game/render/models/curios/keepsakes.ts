/**
 * The keepsake curios' recipes (2.6b): the two collectible balls (the
 * orange one with stars inside and the red-and-white catch ball) and the
 * two under-desk curios (the trap box with its pedal and the fuel case).
 * Each kind starts as the blockout (`curioBlockout`); the batch's model
 * task replaces them, keeping its colours and helpers in this file and
 * importing only `common.ts`.
 */

import type { CurioKind } from "../../../world/types";
import { curioBlockout, type CurioRecipe } from "./common";

/** The keepsake kinds' recipes, one per kind. */
export const KEEPSAKE_RECIPES = {
  "star-ball": curioBlockout,
  "catch-ball": curioBlockout,
  "trap-box": curioBlockout,
  "fuel-case": curioBlockout,
} satisfies Record<
  Extract<CurioKind, "star-ball" | "catch-ball" | "trap-box" | "fuel-case">,
  CurioRecipe
>;
