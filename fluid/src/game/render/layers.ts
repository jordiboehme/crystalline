/**
 * What goes into which layer of the one texture array.
 *
 * The station draws everything with one shader and one `TEXTURE_2D_ARRAY`,
 * after the demoscene habit: a handful of procedural layers made once at
 * start (panels, floor plates, ceiling, metal, hazard stripes, the portal's
 * swirl, grime, pictograms), followed by one layer per piece of text in the
 * room - the placard, every terminal screen, every door, portal and tag
 * label. The geometry and the text renderer both derive the text layers from
 * `textRequests`, so a label always lands on the quad it was drawn for.
 *
 * Every text layer is square; a label quad is six times wider than tall, so
 * the text renderer draws it stretched vertically by the quad's aspect and
 * the quad squeezes it back. `ASPECT` is that shared fact.
 */

import type { RoomSpec } from "../world/types";

/**
 * The procedural layers, made once at start and shared by every room. Their
 * indices are fixed so the geometry can name a surface's texture without
 * asking the texture code, and the text layers start right after them at
 * `TEXT_BASE`.
 */
export const LAYER = {
  panel: 0,
  floor: 1,
  ceiling: 2,
  metal: 3,
  hazard: 4,
  portal: 5,
  grime: 6,
  pictogram: 7,
} as const;

/**
 * The first text layer. The text layer of request `i` from `textRequests` is
 * `TEXT_BASE + i`, a rule the geometry and the text renderer both follow.
 */
export const TEXT_BASE = 8;

/**
 * The side of every layer in texels. One size for all layers is what a
 * texture array demands; 256 keeps a room's worth of text layers small.
 */
export const LAYER_SIZE = 256;

/**
 * Width over height of each kind of text quad. The text renderer stretches
 * its lines vertically by this factor inside the square layer and the quad
 * of that aspect squeezes them back, so letters keep their shape.
 */
export const ASPECT = { screen: 1.25, label: 6, placard: 1.4 } as const;

/**
 * One piece of text to draw into a layer: a stable key naming the quad it
 * belongs to (`placard`, `terminal:<i>`, `door:<i>`, `portal:<i>`,
 * `hatch:<i>`, `tag:<i>`, `poster:<i>` with `i` the fixture's index in
 * `room.fixtures`), the kind of
 * quad, which sets its aspect and style, and the lines to draw.
 */
export interface TextRequest {
  key: string;
  kind: keyof typeof ASPECT;
  lines: string[];
}

/**
 * The text layers a room needs, in layer order: the placard first, then the
 * fixtures in `room.fixtures` order - a terminal gives a screen with its
 * heading on the first line, a door, a portal, a hatch and a machine a
 * one-line label (a sealed door or portal says why it is sealed instead),
 * and a poster a placard-shaped sheet with its category on the first line.
 * The order is the contract: request `i` lives in layer `TEXT_BASE + i`.
 */
export function textRequests(room: RoomSpec): TextRequest[] {
  const out: TextRequest[] = [];
  const placard = room.fixtures.find((f) => f.kind === "placard");
  if (placard?.kind === "placard") {
    out.push({ key: "placard", kind: "placard", lines: placard.lines });
  }
  room.fixtures.forEach((f, i) => {
    switch (f.kind) {
      case "terminal":
        out.push({
          key: `terminal:${i}`,
          kind: "screen",
          lines: [f.heading, ...f.lines],
        });
        break;
      case "door":
        out.push({
          key: `door:${i}`,
          kind: "label",
          lines: [f.sealedLabel ?? f.label],
        });
        break;
      case "portal":
        out.push({
          key: `portal:${i}`,
          kind: "label",
          lines: [f.sealedLabel ?? f.label],
        });
        break;
      case "hatch":
        out.push({ key: `hatch:${i}`, kind: "label", lines: [f.label] });
        break;
      case "poster":
        out.push({
          key: `poster:${i}`,
          kind: "placard",
          lines: [f.category, ...f.lines],
        });
        break;
      case "machine":
        out.push({ key: `tag:${i}`, kind: "label", lines: [f.tag] });
        break;
      case "placard":
        break;
    }
  });
  return out;
}

/**
 * How many layers the texture array needs for a room: the procedural ones
 * plus one per text request.
 */
export function layerCount(room: RoomSpec): number {
  return TEXT_BASE + textRequests(room).length;
}
