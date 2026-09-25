/**
 * The gear curios' recipes (2.6b): the light sword (lying in its cradle,
 * or lit upright in its stand in blue or green), the green pistol, the pink
 * gadget (alone or in a cluster) and the winged meter. Each kind starts as
 * the blockout (`curioBlockout`); the batch's model task replaces them,
 * keeping its colours and helpers in this file and importing only
 * `common.ts`.
 */

import type { CurioKind } from "../../../world/types";
import { curioBlockout, type CurioRecipe } from "./common";

/** The gear kinds' recipes, one per kind. */
export const GEAR_RECIPES = {
  "light-sword": curioBlockout,
  "green-pistol": curioBlockout,
  "pink-gadget": curioBlockout,
  "wing-meter": curioBlockout,
} satisfies Record<
  Extract<
    CurioKind,
    "light-sword" | "green-pistol" | "pink-gadget" | "wing-meter"
  >,
  CurioRecipe
>;
