/**
 * The curios' models (2.6b): one mesh per curio kind, variant and look,
 * built in code with the modelling kit like every other model, and drawn
 * instanced like the props and heroes (`instances.ts`), in their own key
 * space (`curio:<kind>:<variant>`).
 *
 * A curio is built once at the origin, in `frameAt([0, 0, 0], 0)`, centred
 * with its base at `h` 0 (the hovering drone from its lift, `CURIO_LIFT`),
 * and every instance of it is turned by its quarter turn and moved to its
 * anchor on the GPU, at the height of the surface it stands on, so a
 * recipe is a pure function of kind, variant and look. Its slot is its
 * kind's blink bank (`CURIO_BANK`), which its blinking lights pulse with.
 *
 * The recipes live in six batch files on disjoint lists of kinds:
 * `gear.ts`, `retro.ts`, `keepsakes.ts`, `finds.ts`, `desktop.ts` and
 * `critters.ts`, all built on `common.ts`. The
 * curio test (`curioModels.test.ts`) builds every kind and variant, places
 * it at every turn on a surface the way the GPU does and checks the
 * envelope (its turned size and its top), that it reaches its top and
 * sits on its surface (the hovering drone from its lift), the winding, the
 * triangle budget, that nothing glows in mid-air, that no part floats
 * clear of its base or another part, and that blinking parts appear
 * exactly in the kinds whose bank blinks.
 */

import { CURIO_CATALOGUE } from "../../../world/curios";
import type { CurioKind } from "../../../world/types";
import { createBuilder, type MeshData } from "../../geometry";
import { createKit, frameAt } from "../../kit";
import type { Look } from "../../looks";
import { surfaces, type KitAt } from "../common";
import type { CurioRecipe } from "./common";
import { CRITTER_RECIPES } from "./critters";
import { DESKTOP_RECIPES } from "./desktop";
import { FIND_RECIPES } from "./finds";
import { GEAR_RECIPES } from "./gear";
import { KEEPSAKE_RECIPES } from "./keepsakes";
import { RETRO_RECIPES } from "./retro";

/** Every curio kind's recipe, whatever its batch. */
const RECIPES = {
  ...GEAR_RECIPES,
  ...RETRO_RECIPES,
  ...KEEPSAKE_RECIPES,
  ...FIND_RECIPES,
  ...DESKTOP_RECIPES,
  ...CRITTER_RECIPES,
} satisfies Record<CurioKind, CurioRecipe>;

/**
 * Builds variant `variant` of a curio kind into the kits `kitAt` makes, in
 * `frameAt([0, 0, 0], 0)`: centred on the origin, its base at `h` 0 (the
 * hovering drone from its lift) and its front towards `+d`. Throws
 * `buildCurio: <kind> has no variant <n>` on a variant the catalogue does
 * not give the kind, as `buildHero` does: a generator bug should not pass
 * silently.
 */
export function buildCurio(
  kitAt: KitAt,
  kind: CurioKind,
  variant: number,
  look: Look,
): void {
  const variants = CURIO_CATALOGUE[kind].variants;
  if (!Number.isInteger(variant) || variant < 0 || variant >= variants) {
    throw new Error(`buildCurio: ${kind} has no variant ${String(variant)}`);
  }
  RECIPES[kind]({
    k: kitAt(frameAt([0, 0, 0], 0)),
    kitAt,
    s: surfaces(look),
    look,
    variant,
    kind,
  });
}

/**
 * One curio kind's variant as its own mesh, in the look's colours, ready to
 * be drawn instanced: the renderer builds one per kind, variant and look
 * and places each instance by its turn, its anchor and its height.
 */
export function buildCurioMesh(
  kind: CurioKind,
  variant: number,
  look: Look,
): MeshData {
  const b = createBuilder();
  buildCurio((f) => createKit(b, f), kind, variant, look);
  return b.build();
}
