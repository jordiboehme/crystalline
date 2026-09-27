/**
 * The console room's fittings as models (2.6e C2): one mesh per fitting
 * kind, variant and look, built in code with the modelling kit like every
 * other model, and drawn instanced as a family of their own
 * (`interiorInstances` in `instances.ts`, keyed `interior:<kind>:<variant>`).
 *
 * A fitting is built once at the origin, in `frameAt([0, 0, 0], 0)`, and
 * every instance of it is turned by its quarter turn and moved to its
 * anchor on the GPU, so a recipe is a pure function of kind, variant and
 * look. Its slot is its kind's blink bank (`INTERIOR_BANK`), which its
 * blinking lights pulse with.
 *
 * A fitting's moving part is not in its instanced mesh: `buildInteriorMovers`
 * builds it as a mover in world space at the piece's anchor and turn, which
 * `buildRoomMesh` hands the renderer with the fixtures' and heroes'. No
 * kind has one yet.
 *
 * Every kind is the blockout for now (`interiorBlockout`), so the room can
 * be entered, walked and lit before its fittings are modelled. The family's
 * test (`interiorModels.test.ts`) builds every kind and variant, places it
 * at every turn it can take and checks its envelope (`interiorHalf`), the
 * winding, that nothing glows in mid-air, that no part floats clear of the
 * floor or its wall, the triangle budget (C19) and that blinking parts
 * appear exactly in the kinds whose bank blinks.
 */

import { INTERIOR_CATALOGUE } from "../../../world/consoleRoom";
import type { InteriorKind, InteriorPiece } from "../../../world/types";
import { createBuilder, type MeshData } from "../../geometry";
import { createKit, frameAt } from "../../kit";
import type { Look } from "../../looks";
import { surfaces, type KitAt, type Mover } from "../common";
import { interiorBlockout, type InteriorRecipe } from "./common";

/** Every fitting kind's recipe. */
const RECIPES = {
  "roundel-wall": interiorBlockout,
  "inner-doors": interiorBlockout,
  scanner: interiorBlockout,
  console: interiorBlockout,
} satisfies Record<InteriorKind, InteriorRecipe>;

/**
 * Builds variant `variant` of a fitting kind into the kits `kitAt` makes,
 * in `frameAt([0, 0, 0], 0)`: at the origin, facing north, as a piece on a
 * south wall looks into the room. Throws on a variant the catalogue does
 * not give the kind: a bug in the room should not pass silently.
 */
export function buildInterior(
  kitAt: KitAt,
  kind: InteriorKind,
  variant: number,
  look: Look,
): void {
  const variants = INTERIOR_CATALOGUE[kind].variants;
  if (!Number.isInteger(variant) || variant < 0 || variant >= variants) {
    throw new Error(`buildInterior: ${kind} has no variant ${String(variant)}`);
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
 * One fitting kind's variant as its own mesh, in the look's colours, ready
 * to be drawn instanced: the renderer builds one per kind, variant and look
 * and places each instance by its turn and anchor.
 */
export function buildInteriorMesh(
  kind: InteriorKind,
  variant: number,
  look: Look,
): MeshData {
  const b = createBuilder();
  buildInterior((f) => createKit(b, f), kind, variant, look);
  return b.build();
}

/**
 * The moving parts of the fitting at `index` in `room.interior`, as movers
 * in world space at its anchor and turn. None yet: every kind gives `[]`.
 */
export function buildInteriorMovers(
  _piece: InteriorPiece,
  _index: number,
  _look: Look,
): Mover[] {
  return [];
}
