/**
 * What goes into which layer of the one texture array.
 *
 * The station draws everything with one shader and one `TEXTURE_2D_ARRAY`,
 * after the demoscene habit: a handful of procedural layers made once at
 * start (panels, floor plates, ceiling, metal, hazard stripes, the portal's
 * swirl, grime, pictograms, the ribbed and plated wall patterns and the
 * decal atlas, 2.7 C11 and C12), followed by the room's text. A terminal
 * screen, a poster and the placard each get a whole layer; the one-line
 * labels of doors, portals, hatches and machine tags are packed six to a
 * layer, one label per row, so a hub with forty tags and twenty doors does
 * not ask the GPU for sixty layers. `layerPlan` is the one place that
 * decides where each piece of text goes: the geometry asks it for a key's
 * layer and v range, and the text renderer draws each request into the row
 * the plan gave it, so a label always lands on the quad it was drawn for.
 *
 * Every layer is square; a quad is not. A whole layer is drawn stretched
 * vertically by its quad's aspect and the quad squeezes it back. A label
 * row is a 256 by 42 band, already close to a label's six to one, and the
 * same squeeze is applied per row. `ASPECT` is that shared fact.
 */

import type { RoomSpec } from "../world/types";

/**
 * The procedural layers, made once at start and shared by every room. Their
 * indices are fixed so the geometry can name a surface's texture without
 * asking the texture code, and the text layers start right after them at
 * `TEXT_BASE`. `ribbed` and `plated` are the shell's two alternative wall
 * patterns (2.7 C11), read only by the geometry's wall, lintel and stripe
 * quads; every model keeps `panel`. `decal` is the decal atlas (2.7 C12,
 * C20, `decalLayer` in `textures.ts`), its shapes in their alpha.
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
  ribbed: 8,
  plated: 9,
  decal: 10,
} as const;

/**
 * The first text layer: the whole layers (placard, screens, posters) start
 * here in `textRequests` order, and the label layers follow them.
 */
export const TEXT_BASE = 11;

/**
 * The side of every layer in texels. One size for all layers is what a
 * texture array demands; 256 keeps a room's worth of text layers small, and
 * a room with more text counts more layers instead of growing them.
 */
export const LAYER_SIZE = 256;

/** How many one-line labels share one text layer, one per row. */
export const LABEL_ROWS = 6;

/**
 * The height of one label row in texels: the layer split into
 * `LABEL_ROWS` whole-texel bands (42 of 256), with the few texels left over
 * at the bottom of the layer unused, so no row straddles a texel.
 */
export const ROW_HEIGHT = Math.floor(LAYER_SIZE / LABEL_ROWS);

/**
 * How far a label's v range stays inside its row, in texels: half a texel
 * at each edge, so linear filtering and the coarser mip levels sample the
 * label's own row and not the edge of its neighbour.
 */
export const ROW_INSET = 0.5;

/**
 * Width over height of each kind of text quad. The text renderer stretches
 * its lines vertically by this factor (inside the square layer, or inside
 * its row for a label) and the quad of that aspect squeezes them back, so
 * letters keep their shape. A hatch label is a label; a poster is shaped
 * like the placard.
 */
export const ASPECT = {
  screen: 1.25,
  label: 6,
  hatch: 6,
  placard: 1.4,
  poster: 1.4,
} as const;

/** The kinds of text, which set a request's aspect and its drawing style. */
export type TextKind = keyof typeof ASPECT;

/**
 * One piece of text to draw: a stable key naming the quad it belongs to
 * (`placard`, `terminal:<i>`, `door:<i>`, `portal:<i>`, `hatch:<i>`,
 * `tag:<i>`, `poster:<i>` with `i` the fixture's index in
 * `room.fixtures`), the kind of quad, which sets its aspect and style, the
 * lines to draw, and for a label (`label` or `hatch`) its row within its
 * layer; a whole-layer request (`screen`, `placard`, `poster`) has `row`
 * null.
 */
export interface TextRequest {
  key: string;
  kind: TextKind;
  lines: string[];
  row: number | null;
}

/**
 * Where every piece of a room's text goes. `count` is the number of layers
 * the texture array needs (procedural, whole and label layers), `text` every
 * request in drawing order, and `lookup` a key's layer and the part of its
 * height the key's quad shows, `v0` to `v1` in texture v (0 at the bottom of
 * the layer, as `flipRows` uploads it). A whole layer shows 0 to 1; a
 * label row is inset by `ROW_INSET` at both edges, so it shows its row
 * less one texel. `lookup`
 * throws on a key the room has no text for: a quad asking for text that was
 * never drawn is a bug, and a silent fallback would hide it.
 */
export interface LayerPlan {
  count: number;
  text: TextRequest[];
  lookup(key: string): { layer: number; v0: number; v1: number };
}

/** True for the kinds packed into label rows. */
function isLabel(kind: TextKind): boolean {
  return kind === "label" || kind === "hatch";
}

/**
 * The text a room needs, in plan order: the placard first, then the
 * fixtures in `room.fixtures` order - a terminal gives a screen with its
 * heading on the first line, a poster a placard-shaped sheet with its
 * category on the first line, a door, a portal and a machine a one-line
 * label (a sealed door or portal says why it is sealed instead), and a
 * hatch a one-line hatch label. Labels and hatch labels are numbered in
 * this order, the `n`th taking row `n % LABEL_ROWS`; whole-layer requests
 * have no row.
 */
export function textRequests(room: RoomSpec): TextRequest[] {
  const out: Omit<TextRequest, "row">[] = [];
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
        out.push({ key: `hatch:${i}`, kind: "hatch", lines: [f.label] });
        break;
      case "poster":
        out.push({
          key: `poster:${i}`,
          kind: "poster",
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
  let labels = 0;
  return out.map((r) => ({
    ...r,
    row: isLabel(r.kind) ? labels++ % LABEL_ROWS : null,
  }));
}

/**
 * The layer plan of a room: the whole-layer requests take one layer each
 * from `TEXT_BASE` in `textRequests` order, and the labels fill the layers
 * after them, six to a layer in the same order. See `LayerPlan`.
 */
export function layerPlan(room: RoomSpec): LayerPlan {
  const text = textRequests(room);
  const wholes = text.filter((r) => r.row === null).length;
  const labels = text.length - wholes;
  const slots = new Map<string, { layer: number; v0: number; v1: number }>();
  let whole = 0;
  let label = 0;
  for (const r of text) {
    if (r.row === null) {
      slots.set(r.key, { layer: TEXT_BASE + whole++, v0: 0, v1: 1 });
    } else {
      const layer = TEXT_BASE + wholes + Math.floor(label++ / LABEL_ROWS);
      slots.set(r.key, {
        layer,
        v0: 1 - ((r.row + 1) * ROW_HEIGHT - ROW_INSET) / LAYER_SIZE,
        v1: 1 - (r.row * ROW_HEIGHT + ROW_INSET) / LAYER_SIZE,
      });
    }
  }
  return {
    count: TEXT_BASE + wholes + Math.ceil(labels / LABEL_ROWS),
    text,
    lookup(key) {
      const slot = slots.get(key);
      if (slot === undefined) throw new Error(`no text layer for "${key}"`);
      return { ...slot };
    },
  };
}
