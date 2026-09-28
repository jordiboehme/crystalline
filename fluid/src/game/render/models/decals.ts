/**
 * The decals drawn (2.7 C20, C21): every decal of `room.decals` becomes
 * flat quads in the static room mesh, on the decal atlas (`LAYER.decal`,
 * `decalLayer` in `textures.ts`) with `FLAG.decal`, whose alpha the scene
 * shader tests against an ordered threshold, so the shapes need no
 * blending and the room stays one draw call.
 *
 * - A **wall** decal is a `k.panel` in its edge's slot frame
 *   (`frameForSlot(edgeOf(d))`), moved `along`, from `h` to `h + length`
 *   and `DECAL_LIFT` off the wall. Where it would cross the accent stripe
 *   (`ACCENT_STRIPE`, the same lift on the same wall) it is drawn in two
 *   pieces, below and above the band, its uv split with it, so the two
 *   never fight for one depth: the stripe reads as painted over it. On a
 *   room under construction it is cut out of the hazard baseboard's band
 *   (0 to `BASEBOARD`, the same lift) the same way.
 * - A **face** decal is a `k.panel` in the crate's own frame at its face
 *   (the decal's anchor lies on the face, its turn the way the face looks),
 *   `DECAL_LIFT` off it.
 * - A **floor** decal lies `DECAL_LIFT` above the floor (above a pad's
 *   plate on a hangar's pad, `floorTop`), `width` across and
 *   `length` along its turn; the tile's top (its high `v`) points along the
 *   turn, so an arrow points the way its turn says.
 * - **Chevrons** are hazard stripes at 45 degrees on any strip: the strip
 *   is cut into near-square pieces, each showing one 32 by 32 texel repeat
 *   of its tile (`CHEVRON_WINDOW`), so the stripes keep their angle and
 *   run on unbroken from piece to piece.
 * - A **stencil** is set mark by mark (`stencilMarks` in `marks.ts`, 2.7
 *   C19), each mark through `textRows` and laid with the font's own
 *   advance, a space after the word: every run of lit pixels (`runsOf`) is
 *   one quad. The wall stencil's lines are `STENCIL_LINE` tall, the last
 *   line's bottom at the decal's `h`; a floor stencil's pixels are as large
 *   as the widest possible line (`STENCIL_COLUMNS`) allows in its box, its
 *   top towards its turn. Every pixel quad samples one point of the solid
 *   tile (a uv of no extent), so it is full alpha at any distance and never
 *   stippled by a coarse mip level.
 *
 * - The airlock's hazard **ring** (M3 C24) is a black band round its
 *   centre with the chevrons' yellow stripes over it (`ring`), and its
 *   `CYCLE` stencils read a word (`wordMarks`) instead of a bay.
 *
 * Colours (`DECAL_TINT`, C20): chevrons yellow, arrows the room's accent
 * (`accentTint`), grime, streaks and rust their own dark shades, the wall
 * stencil dark and the floor stencil light, a word stencil dark and the
 * ring's stripes yellow over black. A decal adds no text layer and no
 * text key: its only text is the stencil's pixels.
 */

import { HERO_FRONT } from "../../world/footprints";
import { edgeOf } from "../../world/sites";
import type { Decal, RoomSpec } from "../../world/types";
import { CELL } from "../../world/units";
import {
  ACCENT_STRIPE,
  BASEBOARD,
  FLAG,
  accentTint,
  type Builder,
  type Surface,
  type V3,
} from "../geometry";
import { DECAL_LIFT, frameAt, frameForSlot, type Frame } from "../kit";
import { LAYER } from "../layers";
import type { Rgb } from "../looks";
import { DECAL_TILES, tileRect, tileWindow } from "../textures";
import type { KitAt } from "./common";
import { floorTop } from "./hangar";
import { runsOf, textRows } from "./heroes/pixels";
import { stencilMarks, wordMarks } from "./marks";

/**
 * The decals' colours (2.7 C20): hazard yellow chevrons, grime, streaks
 * and rust in their own dark shades, the wall stencil dark and the floor
 * stencil light. Arrows take the room's accent (`accentTint(1)`) instead.
 */
export const DECAL_TINT = {
  chevrons: [0.95, 0.72, 0.1],
  grime: [0.1, 0.09, 0.07],
  streak: [0.16, 0.13, 0.1],
  rust: [0.45, 0.2, 0.08],
  wallStencil: [0.1, 0.1, 0.1],
  floorStencil: [0.9, 0.88, 0.82],
  wordStencil: [0.1, 0.1, 0.1],
  ring: [0.95, 0.72, 0.1],
  ringBase: [0.06, 0.06, 0.06],
} as const satisfies Record<string, Rgb>;

/** The most triangles a room's decals may take (2.7 C21). */
export const DECAL_BUDGET = 1500;

/** A wall stencil's line height, in metres (2.7 C19): five pixel rows. */
const STENCIL_LINE = 0.12;

/**
 * The most font columns a stencil line can take: seven glyphs ("DECK 99"
 * or "BAY 99" and a letter), three columns each and one between. A floor
 * stencil's pixels are sized to fit this in its box, so every floor
 * stencil is set at one size.
 */
const STENCIL_COLUMNS = 27;

/** The font's glyph height in rows. */
const GLYPH_ROWS = 5;

/**
 * The texels of a chevron tile one piece of a strip shows: one whole
 * repeat of the stripes (32 texels both ways), inside the tile's margin,
 * so pieces laid side by side continue each other's stripes.
 */
const CHEVRON_WINDOW = { from: 16, to: 48 } as const;

/** A uv rectangle: `u0` to `u1` left to right, `v0` to `v1` bottom to top. */
interface UvRect {
  u0: number;
  v0: number;
  u1: number;
  v1: number;
}

/**
 * A quad lying `DECAL_LIFT` above the floor's top `base` (0 on the floor,
 * a pad's plate in a hangar, `floorTop`), centred on `(cx, cz)` in
 * metres: `x0..x1` along `right` and `y0..y1` along `front` (its top),
 * facing up, wound counter-clockwise seen from above like the floor, with
 * `uv` from its bottom-left to its top-right corner.
 */
function floorQuad(
  b: Builder,
  base: number,
  cx: number,
  cz: number,
  front: readonly [number, number],
  x0: number,
  x1: number,
  y0: number,
  y1: number,
  uv: UvRect,
  s: Surface,
): void {
  const [fx, fz] = front;
  const [rx, rz] = [-fz, fx];
  const at = (x: number, y: number): V3 => [
    cx + rx * x + fx * y,
    base + DECAL_LIFT,
    cz + rz * x + fz * y,
  ];
  const corners = [
    [at(x0, y0), uv.u0, uv.v0],
    [at(x1, y0), uv.u1, uv.v0],
    [at(x1, y1), uv.u1, uv.v1],
    [at(x0, y1), uv.u0, uv.v1],
  ] as const;
  for (const i of [0, 1, 2, 0, 2, 3]) {
    const c = corners[i];
    if (c) b.vertex(c[0], [0, 1, 0], c[1], c[2], s);
  }
}

/**
 * The uv rectangles `n` pieces of a strip show: its tile's whole shape, or
 * a chevron repeat each.
 */
function piecesOf(d: Decal, tile: number): { n: number; uv: UvRect } {
  if (d.kind !== "chevrons") return { n: 1, uv: tileRect(tile) };
  return {
    n: Math.max(1, Math.round(d.width / d.length)),
    uv: tileWindow(tile, CHEVRON_WINDOW.from, CHEVRON_WINDOW.to),
  };
}

/**
 * A vertical decal in `frame`: `a0..a1` along, `h0..h1` up, `DECAL_LIFT`
 * off the surface, with `uv` over it. The parts inside the `cuts` bands
 * (heights, low to high, not overlapping) are left out and the rest drawn
 * as pieces, each with its own share of the uv.
 */
function upright(
  kitAt: KitAt,
  frame: Frame,
  a0: number,
  a1: number,
  h0: number,
  h1: number,
  uv: UvRect,
  s: Surface,
  cuts: readonly (readonly [number, number])[],
): void {
  const k = kitAt(frame);
  const spans: [number, number][] = [];
  let from = h0;
  for (const [c0, c1] of cuts) {
    if (c1 <= from || c0 >= h1) continue;
    spans.push([from, Math.min(h1, c0)]);
    from = Math.max(from, c1);
  }
  spans.push([from, h1]);
  const vAt = (h: number) => uv.v0 + ((h - h0) / (h1 - h0)) * (uv.v1 - uv.v0);
  for (const [lo, hi] of spans) {
    if (hi - lo <= 1e-6) continue;
    const v0 = vAt(lo);
    k.panel(
      a0,
      a1,
      DECAL_LIFT,
      lo,
      hi,
      s,
      uv.u1 - uv.u0,
      vAt(hi) - v0,
      uv.u0,
      v0,
    );
  }
}

/**
 * The bands a wall decal is cut out of (2.7 C9, C21): the accent stripe's
 * always, and on a room under construction the hazard baseboard's (0 to
 * `BASEBOARD`), both standing `DECAL_LIFT` off the wall like the decal, so
 * they read as painted over it rather than fighting it for depth. A wall
 * decal never lies on a way's edge, where the baseboard is left out. A
 * face decal is cut out of nothing.
 */
function cutsOf(d: Decal, room: RoomSpec): (readonly [number, number])[] {
  if (d.on !== "wall") return [];
  const stripe = [ACCENT_STRIPE.h0, ACCENT_STRIPE.h1] as const;
  return room.condition === "construction"
    ? [[0, BASEBOARD], stripe]
    : [stripe];
}

/** The frame a wall or face decal lies in. */
function frameOf(d: Decal): Frame {
  if (d.on === "wall") return frameForSlot(edgeOf(d));
  return frameAt([d.x * CELL, 0, d.y * CELL], d.turn);
}

/** A decal's surface: the atlas layer, its tint and `FLAG.decal`. */
function surfaceOf(d: Decal): Surface {
  const tint: Rgb =
    d.kind === "arrow"
      ? accentTint(1)
      : d.kind === "stencil"
        ? d.word !== undefined
          ? DECAL_TINT.wordStencil
          : d.on === "floor"
            ? DECAL_TINT.floorStencil
            : DECAL_TINT.wallStencil
        : DECAL_TINT[d.kind];
  return { layer: LAYER.decal, tint, flag: FLAG.decal };
}

/**
 * A stencil's lines as rows of font cells: each line's marks through
 * `textRows`, one dark column between two marks and a space's advance
 * (the space glyph and its column) after the word, as the font sets a
 * line. Row 0 of each line is its top.
 */
function stencilRows(d: Decal): string[][] {
  const lines =
    d.word !== undefined
      ? wordMarks(d.word)
      : d.stencil !== undefined
        ? stencilMarks(d.stencil)
        : [];
  return lines.map((marks) => {
    const rows = Array.from({ length: GLYPH_ROWS }, () => "");
    marks.forEach((mark, i) => {
      const glyphs = textRows(mark);
      const gap = i === 0 ? "" : i === 1 ? "....." : ".";
      glyphs.forEach((g, r) => {
        rows[r] += gap + g;
      });
    });
    return rows;
  });
}

/**
 * Lays a stencil: every run of lit cells of its lines one quad, on the
 * wall in its slot frame or on the floor turned by its turn, every quad
 * sampling one point in the solid tile.
 */
function stencil(kitAt: KitAt, b: Builder, d: Decal, base: number): void {
  const lines = stencilRows(d);
  if (lines.length === 0) return;
  const s = surfaceOf(d);
  const solid = tileRect(DECAL_TILES.solid[0] ?? 15);
  const uc = (solid.u0 + solid.u1) / 2;
  const vc = (solid.v0 + solid.v1) / 2;
  const point: UvRect = { u0: uc, v0: vc, u1: uc, v1: vc };
  const tall = lines.length * (GLYPH_ROWS + 1) - 1;
  const px =
    d.on === "wall"
      ? STENCIL_LINE / GLYPH_ROWS
      : Math.min(d.length / tall, d.width / STENCIL_COLUMNS);
  const kit = d.on === "wall" ? kitAt(frameOf(d)) : null;
  const front = HERO_FRONT[((d.turn % 4) + 4) % 4] ?? [0, -1];
  // The top of the first line, in the decal's own terms: height on a wall,
  // the distance along the turn from the centre on the floor.
  const top = d.on === "wall" ? d.h + tall * px : (tall * px) / 2;
  lines.forEach((rows, li) => {
    const cols = rows[0]?.length ?? 0;
    const left = d.along - (cols * px) / 2;
    const lineTop = top - li * (GLYPH_ROWS + 1) * px;
    for (const r of runsOf(rows)) {
      if (r.ch !== "#") continue;
      const a0 = left + r.col * px;
      const a1 = a0 + r.len * px;
      const hi = lineTop - r.row * px;
      const lo = hi - px;
      if (kit !== null)
        kit.panel(a0, a1, DECAL_LIFT, lo, hi, s, 0, 0, point.u0, point.v0);
      else
        floorQuad(
          b,
          base,
          d.x * CELL,
          d.y * CELL,
          front,
          a0,
          a1,
          lo,
          hi,
          point,
          s,
        );
    }
  });
}

/**
 * One flat quad `lift` above the floor through the four points (metres,
 * `[x, z]`) with the four uvs, wound counter-clockwise seen from above
 * whichever way the points run, so it faces up like the floor.
 */
function flatPiece(
  b: Builder,
  points: readonly (readonly [number, number])[],
  uvs: readonly (readonly [number, number])[],
  lift: number,
  s: Surface,
): void {
  const [p0, p1, p2] = points;
  if (!p0 || !p1 || !p2) return;
  // The normal's height of (p1 - p0) x (p2 - p0), with y up: positive when
  // the points run counter-clockwise seen from above.
  const up =
    (p1[1] - p0[1]) * (p2[0] - p0[0]) - (p1[0] - p0[0]) * (p2[1] - p0[1]);
  const order = up > 0 ? [0, 1, 2, 0, 2, 3] : [0, 2, 1, 0, 3, 2];
  for (const i of order) {
    const p = points[i];
    const uv = uvs[i];
    if (p && uv) b.vertex([p[0], lift, p[1]], [0, 1, 0], uv[0], uv[1], s);
  }
}

/**
 * The hazard ring (M3 C24): a black band `length` wide inside the outer
 * diameter `width`, round the decal's centre, `DECAL_LIFT` above the
 * floor's top `base` (`floorTop`), and over it, one
 * `DECAL_LIFT` higher, the chevrons' yellow stripes. The band is cut into
 * near-square pieces round the ring, each showing one repeat of the
 * chevron tile (`CHEVRON_WINDOW`), so the stripes run on round the ring;
 * the black shows through their gaps. The black samples one point of the
 * solid tile, as a stencil's pixels do.
 */
function ring(b: Builder, d: Decal, base: number): void {
  const cx = d.x * CELL;
  const cz = d.y * CELL;
  const outer = d.width / 2;
  const inner = Math.max(0, outer - d.length);
  const n = Math.max(12, Math.round((Math.PI * (outer + inner)) / d.length));
  const tiles = DECAL_TILES.chevrons;
  const stripes = tileWindow(
    tiles[d.variant % tiles.length] ?? tiles[0] ?? 0,
    CHEVRON_WINDOW.from,
    CHEVRON_WINDOW.to,
  );
  const solid = tileRect(DECAL_TILES.solid[0] ?? 15);
  const dot: [number, number] = [
    (solid.u0 + solid.u1) / 2,
    (solid.v0 + solid.v1) / 2,
  ];
  const black: Surface = {
    layer: LAYER.decal,
    tint: DECAL_TINT.ringBase,
    flag: FLAG.decal,
  };
  const top = surfaceOf(d);
  const at = (r: number, t: number): [number, number] => [
    cx + r * Math.cos(t),
    cz + r * Math.sin(t),
  ];
  for (let i = 0; i < n; i++) {
    const t0 = (2 * Math.PI * i) / n;
    const t1 = (2 * Math.PI * (i + 1)) / n;
    const points = [at(inner, t0), at(inner, t1), at(outer, t1), at(outer, t0)];
    flatPiece(b, points, [dot, dot, dot, dot], base + DECAL_LIFT, black);
    flatPiece(
      b,
      points,
      [
        [stripes.u0, stripes.v0],
        [stripes.u1, stripes.v0],
        [stripes.u1, stripes.v1],
        [stripes.u0, stripes.v1],
      ],
      base + 2 * DECAL_LIFT,
      top,
    );
  }
}

/**
 * Builds every decal of `room.decals` into the static mesh (2.7 C20, C21):
 * wall and face decals through `kitAt`, floor decals straight into `b`.
 * Reads the room's decals and its condition (a room under construction
 * cuts its wall decals out of the baseboard's band too). See the module
 * doc for how each is laid.
 */
export function buildDecals(kitAt: KitAt, b: Builder, room: RoomSpec): void {
  for (const d of room.decals) {
    // The floor's top under a floor decal: a pad's plate in a hangar.
    const base = d.on === "floor" ? floorTop(room, d.x * CELL, d.y * CELL) : 0;
    if (d.kind === "stencil") {
      stencil(kitAt, b, d, base);
      continue;
    }
    if (d.kind === "ring") {
      ring(b, d, base);
      continue;
    }
    const tiles = DECAL_TILES[d.kind];
    const tile = tiles[d.variant % tiles.length] ?? tiles[0] ?? 0;
    const s = surfaceOf(d);
    const { n, uv } = piecesOf(d, tile);
    const step = d.width / n;
    for (let i = 0; i < n; i++) {
      const a0 = d.along - d.width / 2 + i * step;
      const a1 = a0 + step;
      if (d.on === "floor") {
        const front = HERO_FRONT[((d.turn % 4) + 4) % 4] ?? [0, -1];
        floorQuad(
          b,
          base,
          d.x * CELL,
          d.y * CELL,
          front,
          a0,
          a1,
          -d.length / 2,
          d.length / 2,
          uv,
          s,
        );
      } else {
        upright(
          kitAt,
          frameOf(d),
          a0,
          a1,
          d.h,
          d.h + d.length,
          uv,
          s,
          cutsOf(d, room),
        );
      }
    }
  }
}
