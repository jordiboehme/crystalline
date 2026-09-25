/**
 * The living heroes' recipes: the mess table, the helper robot, the sleep
 * ring and the dome planters. What they share is the crew's life aboard:
 * long low tops and benches (the mess table, the planters' beds), rounded
 * shells (the robot's body, the sleep pods, the domes) and soft lights set
 * into them (the robot's face, the pods' ring, the grow lights).
 *
 * Every kind is a checked blockout (`heroBlockout`) for now; its model
 * task replaces the entries of this table and touches nothing else.
 */

import type { HeroKind } from "../../../world/types";
import { heroBlockout, type HeroRecipe } from "./common";

/** The living kinds' recipes. */
export const LIVING_RECIPES = {
  "mess-table": heroBlockout,
  "helper-robot": heroBlockout,
  "sleep-ring": heroBlockout,
  "dome-planters": heroBlockout,
} satisfies Record<
  Extract<
    HeroKind,
    "mess-table" | "helper-robot" | "sleep-ring" | "dome-planters"
  >,
  HeroRecipe
>;
