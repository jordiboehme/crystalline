/**
 * The hand-built rooms' fittings as models (2.6e C2, M3 C24): one mesh per fitting
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
 * `buildRoomMesh` hands the renderer with the fixtures' and heroes'. Only
 * the console has one, its rotor (`rotorMover`); it is the console room's
 * only moving part (C9).
 *
 * The wall pieces (the roundel wall, the inner doors and the scanner) are
 * built in `walls.ts`, the console with its column's frame in
 * `console.ts`, the airlock's outer hatch, beacons, iris light and suit
 * lockers in `airlock.ts`. The family's
 * test (`interiorModels.test.ts`) builds every kind and variant, places it
 * at every turn it can take and checks its envelope (`interiorHalf`), the
 * winding, that nothing glows in mid-air, that no part floats clear of the
 * floor, its wall or (hung from it) the ceiling, the triangle budget (C19) and that blinking parts
 * appear exactly in the kinds whose bank blinks.
 */

import { INTERIOR_CATALOGUE } from "../../../world/consoleRoom";
import type { InteriorKind, InteriorPiece } from "../../../world/types";
import { createBuilder, type MeshData } from "../../geometry";
import { createKit, frameAt } from "../../kit";
import type { Look } from "../../looks";
import { surfaces, type KitAt, type Mover } from "../common";
import { AIRLOCK_RECIPES } from "./airlock";
import type { InteriorRecipe } from "./common";
import { CONSOLE_RECIPES, rotorMover } from "./console";
import { WALL_RECIPES } from "./walls";

/** Every fitting kind's recipe. */
const RECIPES = {
  ...WALL_RECIPES,
  ...CONSOLE_RECIPES,
  ...AIRLOCK_RECIPES,
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
 * in world space at its anchor and turn: the console's rotor
 * (`rotorMover`, keyed `rotor:<index>`), and `[]` for every other kind,
 * none of which moves (C9).
 */
export function buildInteriorMovers(
  piece: InteriorPiece,
  index: number,
  look: Look,
): Mover[] {
  return piece.kind === "console" ? [rotorMover(piece, index, look)] : [];
}
