/**
 * The machines: the giant robot head, the spider tank and the garden
 * robot, the big armoured machines of the engineering and lab halls.
 *
 * Blockouts until Task 5 of the 2.6c plan: every kind here is
 * `heroBlockout` for now.
 */

import type { HeroKind } from "../../../world/types";
import { heroBlockout, type HeroRecipe } from "./common";

/** The machine kinds' recipes. */
export const MECH_RECIPES = {
  "mech-head": heroBlockout,
  "spider-tank": heroBlockout,
  "garden-robot": heroBlockout,
} satisfies Record<
  Extract<HeroKind, "mech-head" | "spider-tank" | "garden-robot">,
  HeroRecipe
>;
