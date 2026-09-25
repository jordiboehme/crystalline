/**
 * The station's detailed models, built in code with the modelling kit
 * (`../kit.ts`): terminals, doors, hatches, portals, the twelve machines,
 * posters, the placard and the archetypes' furniture. There are no model
 * files and no loader.
 *
 * The room mesh calls `buildFixture` for every fixture and `buildDecor` for
 * every piece of furniture, passing a kit factory that emits into its one
 * builder: `(f) => createKit(builder, f)`. A recipe asks the factory for
 * its main frame (`frameForSlot(fixture.slot)` or `frameForDecor(decor)`)
 * and for any sub-frame a part needs, so a sloped deck, a slanted table,
 * an arch or the angled wings of a console are built with the same
 * primitives as everything square. Everything static lands in the room's
 * one vertex array; door panels come back as movers, each its own small
 * mesh, for the renderer to slide open.
 *
 * Every model stays inside its footprint (`FOOTPRINTS` in
 * `world/footprints.ts`) or, for what is mounted on the wall, inside the
 * slot's cell and `FLUSH_DEPTH` from the wall, and below the ceiling less
 * `HEADROOM`.
 * Every glowing part sits on or in a body. The test builds each kind on
 * every wall and turn and checks those rules, the winding and a triangle
 * budget.
 */

import type { Fixture } from "../../world/types";
import type { KitAt, ModelContext, Mover } from "./common";
import { buildDoor } from "./doors";
import { buildHatch } from "./hatch";
import { buildMachine } from "./machines";
import { buildPortal } from "./portal";
import { buildTerminal } from "./terminal";
import { buildPlacard, buildPoster } from "./wall";

export {
  FLUSH_DEPTH,
  HEADROOM,
  type KitAt,
  type ModelContext,
  type Mover,
  type TextSlot,
} from "./common";
export { PIPE_DROP, buildDecor, pipeLength } from "./decor";
export {
  BLAST_DOWN_TRAVEL,
  BLAST_UP_TRAVEL,
  BULKHEAD_TRAVEL,
  HOUSING_DEPTH,
  OPENING,
  SLIDE_TRAVEL,
} from "./doors";

/**
 * Builds one fixture of a room: its static parts into the kits `kitAt`
 * makes, and its moving parts (the panels of a door that opens) returned
 * as movers keyed `door:<index>`. `index` is the fixture's position in
 * `room.fixtures`, which names its text layer key (`terminal:<index>`,
 * `tag:<index>` and so on) and its movers. Every kind but an open door
 * returns no movers.
 */
export function buildFixture(
  kitAt: KitAt,
  fixture: Fixture,
  index: number,
  ctx: ModelContext,
): Mover[] {
  switch (fixture.kind) {
    case "terminal":
      buildTerminal(kitAt, fixture, index, ctx);
      return [];
    case "door":
      return buildDoor(kitAt, fixture, index, ctx);
    case "portal":
      buildPortal(kitAt, fixture, index, ctx);
      return [];
    case "hatch":
      buildHatch(kitAt, fixture, index, ctx);
      return [];
    case "machine":
      buildMachine(kitAt, fixture, index, ctx);
      return [];
    case "poster":
      buildPoster(kitAt, fixture, index, ctx);
      return [];
    case "placard":
      buildPlacard(kitAt, fixture, ctx);
      return [];
  }
}
