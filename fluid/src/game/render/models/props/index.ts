/**
 * The set dressing's models: one mesh per prop kind, variant and look,
 * built in code with the modelling kit like every other model, and drawn
 * instanced.
 *
 * Unlike a fixture, which is built where it stands into the room's one
 * vertex array, a prop is built once at the origin, in
 * `frameAt([0, 0, 0], 0)`, and every instance of it is turned by its
 * quarter turn (`TURN_XZ` in `../../kit.ts`) and moved to its anchor on
 * the GPU. So a recipe is a pure function of kind, variant and look.
 *
 * The recipes live by anchor in `wall.ts`, `floor.ts` and `ceiling.ts`,
 * and share the envelopes of `common.ts`. The prop test builds every kind
 * and variant, places it at every turn the way the GPU does and checks
 * the envelope, the winding, the triangle budget and that nothing glows
 * in mid-air.
 */

import { PROP_CATALOGUE } from "../../../world/props";
import type { PropKind } from "../../../world/types";
import { createBuilder, type MeshData } from "../../geometry";
import { createKit, frameAt } from "../../kit";
import type { Look } from "../../looks";
import { surfaces, type KitAt } from "../common";
import { CEILING_RECIPES } from "./ceiling";
import type { PropContext, PropRecipe } from "./common";
import { FLOOR_RECIPES } from "./floor";
import { WALL_RECIPES } from "./wall";

export {
  CEILING_DROP,
  CEILING_OUT,
  CEILING_SETBACK,
  FLOOR_TOP,
  RUN_BAND,
  RUN_REACH,
  SPAN_HALF,
  SPAN_REACH,
  WALL_REACH,
  WALL_TOP,
  WIDE_REACH,
  type PropContext,
  type PropRecipe,
} from "./common";
export { CEILING_RECIPES } from "./ceiling";
export { FLOOR_RECIPES } from "./floor";
export { WALL_RECIPES } from "./wall";

/** Every kind's recipe, whatever its anchor. */
const RECIPES = {
  ...WALL_RECIPES,
  ...FLOOR_RECIPES,
  ...CEILING_RECIPES,
} satisfies Record<PropKind, PropRecipe>;

/**
 * Builds variant `variant` of a prop kind into the kits `kitAt` makes, in
 * `frameAt([0, 0, 0], 0)`: at the origin, facing north, as a prop on a
 * south wall looks into the room. Throws on a variant the catalogue does
 * not give the kind, as `propFootprint` does: a generator bug should not
 * pass silently.
 */
export function buildProp(
  kitAt: KitAt,
  kind: PropKind,
  variant: number,
  ctx: PropContext,
): void {
  const variants = PROP_CATALOGUE[kind].variants;
  if (!Number.isInteger(variant) || variant < 0 || variant >= variants) {
    throw new Error(`buildProp: ${kind} has no variant ${String(variant)}`);
  }
  RECIPES[kind]({
    k: kitAt(frameAt([0, 0, 0], 0)),
    kitAt,
    s: surfaces(ctx.look),
    ctx,
    variant,
    kind,
  });
}

/**
 * One prop kind's variant as its own mesh, in the look's colours, ready to
 * be drawn instanced: the renderer builds one per kind, variant and look
 * and places each instance by its turn and anchor.
 */
export function buildPropMesh(
  kind: PropKind,
  variant: number,
  look: Look,
): MeshData {
  const b = createBuilder();
  buildProp((f) => createKit(b, f), kind, variant, { look });
  return b.build();
}
