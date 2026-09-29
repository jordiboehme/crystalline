/**
 * The station's detailed models, built in code with the modelling kit
 * (`../kit.ts`): terminals, doors, hatches, portals, the twelve machines,
 * posters, the placard, the station's lift, wall screen and exit, and the
 * archetypes' furniture. There are no model files and no loader.
 *
 * The room mesh calls `buildFixture` for every fixture and `buildDecor` for
 * every piece of furniture, passing a kit factory that emits into its one
 * builder: `(f) => createKit(builder, f)`. A recipe asks the factory for
 * its main frame (`frameForSlot(fixture.slot)` or `frameForDecor(decor)`)
 * and for any sub-frame a part needs, so a sloped deck, a slanted table,
 * an arch or the angled wings of a console are built with the same
 * primitives as everything square. Everything static lands in the room's
 * one vertex array; the moving parts of every way (door leaves, a door's
 * hazard lamp and sparks, a hatch's lid, a portal's swirl disc, a lift's
 * leaves) come back as movers, each its own small mesh, for the renderer
 * to slide, blink or scale (`render/parts.ts`).
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
import { buildExit, buildLift, buildScreen } from "./lift";
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
  type MoverPart,
  type TextSlot,
} from "./common";
export { PIPE_DROP, buildDecor, pipeLength } from "./decor";
export {
  BLAST_DOWN_TRAVEL,
  BLAST_SPLIT,
  BLAST_UP_TRAVEL,
  BULKHEAD_TRAVEL,
  HOUSING_DEPTH,
  LAMP_IDLE,
  OPENING,
  SLIDE_TRAVEL,
} from "./doors";
export { LID_CRACK } from "./hatch";
export { DISC_SEALED_GAIN } from "./portal";

/**
 * Builds one fixture of a room: its static parts into the kits `kitAt`
 * makes, and its moving parts returned as movers, keyed as in `Mover`:
 * every door returns its two leaves (`door:<index>`, sealed or not), its
 * lamp (`lamp:<index>`) and its sparks (`spark:<index>`), every hatch its
 * lid (`lid:<index>`) and every portal its disc (`disc:<index>`). A lift
 * returns its two leaves (`door:<index>`) and an exit the sliding door's
 * movers under the same keys, its label drawn under `exit:<index>`
 * (`models/lift.ts`). `index` is the fixture's position in
 * `room.fixtures`, which names its text layer key (`terminal:<index>`,
 * `tag:<index>`, `lift:<index>` and so on) and its movers. Every other
 * kind, the station's wall screen among them, returns no movers.
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
      return buildPortal(kitAt, fixture, index, ctx);
    case "hatch":
      return buildHatch(kitAt, fixture, index, ctx);
    case "machine":
      buildMachine(kitAt, fixture, index, ctx);
      return [];
    case "poster":
      buildPoster(kitAt, fixture, index, ctx);
      return [];
    case "placard":
      buildPlacard(kitAt, fixture, ctx);
      return [];
    case "lift":
      return buildLift(kitAt, fixture, index, ctx);
    case "screen":
      return buildScreen(kitAt, fixture, index, ctx);
    case "exit":
      return buildExit(kitAt, fixture, index, ctx);
  }
}
