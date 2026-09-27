/**
 * The hero props' models: one mesh per hero kind, variant and look, built
 * in code with the modelling kit like every other model, and drawn
 * instanced like the props (`instances.ts`), in their own key space.
 *
 * A hero is built once at the origin, in `frameAt([0, 0, 0], 0)`, and every
 * instance of it is turned by its quarter turn and moved to its anchor on
 * the GPU, so a recipe is a pure function of kind, variant and look. Its
 * slot is its kind's blink bank (`HERO_BANK`), which its blinking lights
 * pulse with.
 *
 * A hero's moving parts (the police box's two door leaves, 2.6e C10) are
 * not in its instanced mesh: `buildHeroMovers` builds them as movers in
 * world space at the hero's anchor and turn, which `buildRoomMesh` hands
 * the renderer with the fixtures' movers. `buildHero` builds them in place
 * by default, so the model checks see the whole hero; `buildHeroMesh`
 * leaves them out.
 *
 * The recipes live in nine batch files grouped by the parts they share:
 * `optics.ts`, `living.ts`, `workshop.ts` and `arcade.ts`, and the large ones
 * in `floaters.ts` (the three hovering heroes), `exhibits.ts`, `mechs.ts` and
 * `street.ts`, all built on `common.ts`; `walker.ts` holds the slab walker's
 * recipe alone. The hero test (`heroModels.test.ts`)
 * builds every kind and variant, places it at every turn the way the GPU does
 * and checks the envelope (its footprint and its top), that it reaches its
 * top, the winding, the triangle budget, that nothing glows in mid-air, that
 * no part floats clear of the floor (or of its lift, for a hovering hero), its
 * wall or another part, that the mesh rests exactly at its lift, that blinking
 * parts appear exactly in the kinds whose bank blinks, and, for a kind with a
 * catalogue surface, that a grid over it lands on a real upward face with its
 * headroom clear. What that grid does not reach (a shape only a kind's own
 * recipe carries, a seam, a chase order) is the per-batch tests' job, one file
 * per batch under `models/heroes/`.
 */

import { HERO_CATALOGUE } from "../../../world/heroes";
import type { Hero, HeroKind } from "../../../world/types";
import { createBuilder, type MeshData } from "../../geometry";
import { createKit, frameAt } from "../../kit";
import type { Look } from "../../looks";
import { surfaces, type KitAt, type Mover } from "../common";
import { ARCADE_RECIPES } from "./arcade";
import type { HeroRecipe } from "./common";
import { EXHIBIT_RECIPES } from "./exhibits";
import { FLOATER_RECIPES } from "./floaters";
import { LIVING_RECIPES } from "./living";
import { MECH_RECIPES } from "./mechs";
import { OPTICS_RECIPES } from "./optics";
import { STREET_RECIPES, boxLeafMovers } from "./street";
import { WALKER_RECIPES } from "./walker";
import { WORKSHOP_RECIPES } from "./workshop";

/** Every hero kind's recipe, whatever its batch. */
const RECIPES = {
  ...OPTICS_RECIPES,
  ...LIVING_RECIPES,
  ...WORKSHOP_RECIPES,
  ...ARCADE_RECIPES,
  ...FLOATER_RECIPES,
  ...EXHIBIT_RECIPES,
  ...MECH_RECIPES,
  ...STREET_RECIPES,
  ...WALKER_RECIPES,
} satisfies Record<HeroKind, HeroRecipe>;

/**
 * Builds variant `variant` of a hero kind into the kits `kitAt` makes, in
 * `frameAt([0, 0, 0], 0)`: at the origin, facing north, as a hero on a
 * south wall looks into the room. With `movers` (the default) the kind's
 * moving parts are built in place, closed, so the mesh is the whole hero;
 * without, they are left to `buildHeroMovers`. Throws on a variant the
 * catalogue does not give the kind, as `heroFootprint` does: a generator
 * bug should not pass silently.
 */
export function buildHero(
  kitAt: KitAt,
  kind: HeroKind,
  variant: number,
  look: Look,
  movers = true,
): void {
  const variants = HERO_CATALOGUE[kind].variants;
  if (!Number.isInteger(variant) || variant < 0 || variant >= variants) {
    throw new Error(`buildHero: ${kind} has no variant ${String(variant)}`);
  }
  RECIPES[kind]({
    k: kitAt(frameAt([0, 0, 0], 0)),
    kitAt,
    s: surfaces(look),
    look,
    variant,
    kind,
    movers,
  });
}

/**
 * One hero kind's variant as its own mesh, in the look's colours, ready to
 * be drawn instanced: the renderer builds one per kind, variant and look
 * and places each instance by its turn and anchor. The kind's moving parts
 * are left out: `buildHeroMovers` builds them.
 */
export function buildHeroMesh(
  kind: HeroKind,
  variant: number,
  look: Look,
): MeshData {
  const b = createBuilder();
  buildHero((f) => createKit(b, f), kind, variant, look, false);
  return b.build();
}

/**
 * The moving parts of the hero at `index` in `room.heroes`, as movers in
 * world space at its anchor and turn: a police box's two door leaves
 * (`boxLeafMovers`, keyed `boxKey(index)`), nothing for every other kind.
 * Together with `buildHeroMesh` they make exactly the whole hero
 * `buildHero` builds.
 */
export function buildHeroMovers(
  hero: Hero,
  index: number,
  look: Look,
): Mover[] {
  return hero.kind === "police-box" ? boxLeafMovers(hero, index, look) : [];
}
