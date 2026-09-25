/**
 * The optics heroes' recipes: the turret, the black slab, the eye panel,
 * the photo console and the laser desk. What they share is a lens or a
 * screen: a glowing eye or lens set in a housing (the turret, the eye
 * panel, the laser desk's emitter), a screen bank on a desk (the photo
 * console, the laser desk's terminal), and the slab, which is all housing
 * and no light.
 *
 * Every kind is a checked blockout (`heroBlockout`) for now; its model
 * task replaces the entries of this table and touches nothing else.
 */

import type { HeroKind } from "../../../world/types";
import { heroBlockout, type HeroRecipe } from "./common";

/** The optics kinds' recipes. */
export const OPTICS_RECIPES = {
  turret: heroBlockout,
  "black-slab": heroBlockout,
  "eye-panel": heroBlockout,
  "photo-console": heroBlockout,
  "laser-desk": heroBlockout,
} satisfies Record<
  Extract<
    HeroKind,
    "turret" | "black-slab" | "eye-panel" | "photo-console" | "laser-desk"
  >,
  HeroRecipe
>;
