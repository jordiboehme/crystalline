/**
 * What every model recipe shares: the context a room hands them, the
 * movers they give back, sub-frames for parts that do not stand square to
 * their wall, the look's surfaces and the sign plate a label is mounted on.
 *
 * Kept apart from `index.ts` so the recipe files can import it without an
 * import cycle through the dispatcher.
 */

import type { Rect } from "../../world/types";
import { FLAG, type MeshData, type Surface, type V3 } from "../geometry";
import { DECAL_LIFT, type Frame, type Kit } from "../kit";
import { ASPECT, LAYER } from "../layers";
import type { Look, Rgb } from "../looks";

/**
 * Makes a kit for a frame, all kits emitting into the same builder. The
 * room mesh passes `(f) => createKit(builder, f)`. A recipe asks for its
 * main frame (`frameForSlot` or `frameForDecor`) and for any sub-frame it
 * needs: a quarter turn to extrude a front-to-back profile, or a yaw for
 * the angled wings of the command console.
 */
export type KitAt = (frame: Frame) => Kit;

/**
 * Where a piece of text lives: its texture layer and the row of that layer
 * it takes, `v0` to `v1` (a whole-layer screen or poster gets 0 to 1).
 */
export interface TextSlot {
  layer: number;
  v0: number;
  v1: number;
}

/**
 * What a room tells its models: the look (its palette tints every part),
 * the ceiling height every model stays under, the main hall's rectangle
 * (a ceiling pipe run is sized to stay inside it) and where each text key
 * is drawn. `textLayer` takes the keys of `layers.ts` (`terminal:<i>`,
 * `door:<i>`, `portal:<i>`, `hatch:<i>`, `tag:<i>`, `poster:<i>`,
 * `placard`) and throws on a key it does not know.
 */
export interface ModelContext {
  look: Look;
  ceiling: number;
  hall: Rect;
  textLayer(key: string): TextSlot;
}

/**
 * The five kinds of moving part a way has: a door `leaf`, a door's
 * `spark` cluster and hazard `lamp` lens, a hatch's `lid` and a portal's
 * swirl `disc`.
 */
export type MoverPart = "leaf" | "spark" | "lamp" | "lid" | "disc";

/**
 * A part that moves, blinks or collapses: its own mesh in world space at
 * rest (a leaf or lid shut, a disc whole), drawn with per-draw uniforms
 * (`moverDraw` in `render/parts.ts`).
 *
 * - `key` names the part and its fixture: `door:<i>` for a leaf (both
 *   leaves of a door share it, as they open together, and it is the key
 *   the door's open fraction comes under), `spark:<i>`, `lamp:<i>`,
 *   `lid:<i>` and `disc:<i>`.
 * - `part` says which of the five it is, `fixture` the index `i` in
 *   `room.fixtures` (the key the fault frames come under).
 * - A leaf or lid is drawn offset by `axis * travel * open`: `axis` the
 *   world unit direction it slides, `travel` how many metres it slides
 *   when fully open. A spark, lamp or disc has travel 0 and axis the
 *   wall's `inward`.
 * - A disc is drawn scaled about `pivot`, its centre in world metres;
 *   every other part has pivot null and is never scaled.
 * - A spark, lamp or disc is drawn with a gain: `rest` is that gain while
 *   no fault frame names the fixture (1 for a leaf, lid or open disc, the
 *   lamp's idle glow, 0 for sparks, which are then not drawn at all, and
 *   the dim glow of a sealed disc).
 */
export interface Mover {
  key: string;
  part: MoverPart;
  fixture: number;
  mesh: MeshData;
  axis: V3;
  travel: number;
  pivot: V3 | null;
  rest: number;
}

/**
 * How far a flush fixture (door, portal, hatch, poster, placard, and the
 * wall-mounted parts of a terminal or machine such as a tag strip) may
 * stand out from its wall, in metres. Flush fixtures do not block the
 * player, so this stays under the player's radius (0.35 m): the player's
 * circle, stopped by the wall, never reaches into it.
 */
export const FLUSH_DEPTH = 0.3;

/** How far below the ceiling every model stays. */
export const HEADROOM = 0.05;

/** A frame moved by `a` along and `d` inward, same directions. */
export function offset(f: Frame, a: number, d: number): Frame {
  return {
    origin: [
      f.origin[0] + f.along[0] * a + f.inward[0] * d,
      f.origin[1],
      f.origin[2] + f.along[2] * a + f.inward[2] * d,
    ],
    along: [...f.along],
    inward: [...f.inward],
  };
}

/**
 * A frame at local `(a, d)`, turned about the vertical by `angle` radians:
 * a positive angle swings its `along` towards the old `inward`. Both axes
 * turn together, so `along x up = inward` still holds and every primitive
 * keeps its winding.
 */
export function yawed(f: Frame, a: number, d: number, angle: number): Frame {
  const o = offset(f, a, d);
  const c = Math.cos(angle);
  const s = Math.sin(angle);
  const mix = (p: V3, q: V3, x: number, y: number): V3 => [
    p[0] * x + q[0] * y,
    p[1] * x + q[1] * y,
    p[2] * x + q[2] * y,
  ];
  return {
    origin: o.origin,
    along: mix(f.along, f.inward, c, s),
    inward: mix(f.along, f.inward, -s, c),
  };
}

/**
 * The frame turned a quarter so its `inward` runs along the old `along`
 * and its `along` against the old `inward`. A point `(a', d')` in it is
 * `(d', -a')` in the old frame: this is what lets `extrude` push a
 * front-to-back profile (a sloped deck, a slanted table, an arch) along
 * the wall.
 */
export function sideways(f: Frame): Frame {
  return {
    origin: [...f.origin],
    along: [-f.inward[0], -f.inward[1], -f.inward[2]],
    inward: [...f.along],
  };
}

/**
 * A thin rectangle in a 2D plane, centred at `(cx, cy)`, `length` long at
 * `angle` radians from the first axis and `width` wide across it: an
 * outline for `k.extrude`, used for a fan blade, a slanted louvre, a
 * leaning rail or a sagging cable link, or any other flat bar that does
 * not sit flush along one axis alone. The plane is whichever two axes the
 * caller extrudes across: `(a, h)` directly, or `(d, h)` through
 * `sideways`.
 */
export function tiltedBar(
  cx: number,
  cy: number,
  angle: number,
  length: number,
  width: number,
): [number, number][] {
  const dx = Math.cos(angle) * (length / 2);
  const dy = Math.sin(angle) * (length / 2);
  const nx = -Math.sin(angle) * (width / 2);
  const ny = Math.cos(angle) * (width / 2);
  return [
    [cx - dx + nx, cy - dy + ny],
    [cx + dx + nx, cy + dy + ny],
    [cx + dx - nx, cy + dy - ny],
    [cx - dx - nx, cy - dy - ny],
  ];
}

/**
 * Extrudes a profile of `[d, h]` points (depth out from the wall, height),
 * drawn in the frame's side view, along the wall from `a0` to `a1`: the
 * wedge of a keyboard deck, the slant of a nav table, a half ring standing
 * across a bed.
 */
export function profileAlong(
  kitAt: KitAt,
  f: Frame,
  profile: readonly (readonly [d: number, h: number])[],
  a0: number,
  a1: number,
  s: Surface,
): void {
  kitAt(sideways(f)).extrude(
    profile.map(([d, h]) => [-d, h] as const),
    a0,
    a1,
    s,
  );
}

/** A colour scaled towards black by `k` (1 keeps it, 0 is black). */
export function shade(c: Rgb, k: number): Rgb {
  return [c[0] * k, c[1] * k, c[2] * k];
}

/**
 * The look's surfaces the recipes draw with: `body` the machine shell,
 * `metal` bare metal, `dark` gunmetal for keys, grilles and trim, `panel`
 * the white wall panel colour, `hazard` the stripes, and `glow` for a
 * light of a given colour.
 */
export function surfaces(look: Look) {
  const p = look.palette;
  return {
    body: { layer: LAYER.panel, tint: p.machine, flag: FLAG.lit },
    panel: { layer: LAYER.panel, tint: p.panel, flag: FLAG.lit },
    metal: { layer: LAYER.metal, tint: p.metal, flag: FLAG.lit },
    dark: { layer: LAYER.metal, tint: shade(p.metal, 0.45), flag: FLAG.lit },
    hazard: { layer: LAYER.hazard, tint: [1, 1, 1], flag: FLAG.lit },
    glow: (tint: Rgb): Surface => ({
      layer: LAYER.panel,
      tint,
      flag: FLAG.emissive,
    }),
    tinted: (tint: Rgb, layer: number = LAYER.panel): Surface => ({
      layer,
      tint,
      flag: FLAG.lit,
    }),
  } satisfies Record<string, Surface | ((...args: never[]) => Surface)>;
}

/** The surfaces `surfaces` returns. */
export type Surfaces = ReturnType<typeof surfaces>;

/** The margin of a sign plate around its label, in metres. */
const PLATE_MARGIN = 0.02;

/**
 * A one-line label on a sign plate: the plate (dark metal) stands out from
 * the wall to depth `d`, and the label quad sits on its face with the
 * label's row of its text layer. The label is `a0..a1` wide and as tall as
 * `ASPECT.label` makes it, starting at `h0`, but never above the ceiling
 * less `HEADROOM`: under a low ceiling it is squeezed rather than pushed
 * through.
 */
export function label(
  kit: Kit,
  ctx: ModelContext,
  key: string,
  a0: number,
  a1: number,
  h0: number,
  d: number,
  tint: Rgb = [1, 1, 1],
): void {
  const top = ctx.ceiling - HEADROOM;
  const h1 = Math.min(h0 + (a1 - a0) / ASPECT.label, top - PLATE_MARGIN);
  kit.box(
    a0 - PLATE_MARGIN,
    a1 + PLATE_MARGIN,
    0,
    d,
    h0 - PLATE_MARGIN,
    h1 + PLATE_MARGIN,
    surfaces(ctx.look).dark,
  );
  textPanel(kit, ctx, key, a0, a1, d, h0, h1, {
    tint,
    flag: FLAG.emissive,
  });
}

/**
 * A quad showing a text key's row: the whole width of the layer, `v0` to
 * `v1` of its height, facing into the room on a backing whose face is at
 * depth `backing`. The quad itself stands `DECAL_LIFT` in front of that
 * face, so no caller picks its own lift and none can pick one too small
 * to survive the depth buffer at a distance.
 */
export function textPanel(
  kit: Kit,
  ctx: ModelContext,
  key: string,
  a0: number,
  a1: number,
  backing: number,
  h0: number,
  h1: number,
  look: { tint: Rgb; flag: Surface["flag"] },
): void {
  const slot = ctx.textLayer(key);
  kit.panel(
    a0,
    a1,
    backing + DECAL_LIFT,
    h0,
    h1,
    { layer: slot.layer, tint: look.tint, flag: look.flag },
    1,
    slot.v1 - slot.v0,
    0,
    slot.v0,
  );
}
