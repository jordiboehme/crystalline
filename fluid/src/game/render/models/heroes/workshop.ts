/**
 * The workshop heroes' recipes: the core wall, the gun rack, the gun
 * bench, the tube bench and the field pack. What they share is the
 * workbench and the wall rack: heavy benches with tools and cables (the
 * two benches), wall frames that hold a piece (the rack, the quilted core
 * wall), ribbed cylinders and hoses (the big gun, the pack) and glowing
 * cores, tubes and status lights.
 *
 * Every kind is a checked blockout (`heroBlockout`) for now; its model
 * task replaces the entries of this table and touches nothing else.
 */

import type { HeroKind } from "../../../world/types";
import { heroBlockout, type HeroRecipe } from "./common";

/** The workshop kinds' recipes. */
export const WORKSHOP_RECIPES = {
  "core-wall": heroBlockout,
  "gun-rack": heroBlockout,
  "gun-bench": heroBlockout,
  "tube-bench": heroBlockout,
  "field-pack": heroBlockout,
} satisfies Record<
  Extract<
    HeroKind,
    "core-wall" | "gun-rack" | "gun-bench" | "tube-bench" | "field-pack"
  >,
  HeroRecipe
>;
