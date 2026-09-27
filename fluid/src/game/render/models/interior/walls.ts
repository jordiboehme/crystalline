/**
 * The console room's wall pieces (2.6e C5 to C7, C24a to C24c): the
 * roundel wall, the inner doors and the scanner. Each is a flush piece,
 * built from its wall point outward along `d` with its back on the wall at
 * `d = 0`, `a` across it and `h` up, in the room's fixed white (C4)
 * whatever the look.
 *
 * A roundel is a round recess in a white slab: a 10-sided rim ring
 * standing `ROUNDEL.rimProud` proud of the slab (two half rings, since an
 * extruded outline may not have a hole) and, one `DECAL_LIFT` off the
 * slab, a shaded face showing through the ring's hole. The face is a
 * square just larger than the hole whose corners stay hidden under the
 * ring (`faceHalf`), so a roundel costs two triangles more than its ring.
 * The rim's inner wall catches the look's light, so the face reads sunk
 * into the wall.
 *
 * - The roundel wall is a slab of 3 by 5 roundels. A few of them glow
 *   (`GLOWING_ROUNDELS`, by variant): their face is a blinking light on
 *   the `breathe` bank, each on a group of its own, so they breathe out of
 *   step, and the rim stays white.
 * - The inner doors are a 4 m slab with a column of roundels at each side,
 *   in step with the roundel walls beside them, and a pair of tall white
 *   leaves standing proud of it, each with a smaller grid of roundels
 *   (C24b), a thin grey stile where they meet and a dark frame line round
 *   the pair. They never move (C9).
 * - The scanner is a roundel wall with a dark housing standing out of it,
 *   whose recessed screen glows a pale blue and shows a faint field of
 *   small lit dots. No roundel sits under the housing (C24c). No text.
 *
 * No recipe here sets text (C20).
 */

import { BLINK_GROUPS } from "../../blink";
import { DECAL_LIFT, type Kit } from "../../kit";
import type { Surface } from "../../geometry";
import type { Rgb } from "../../looks";
import { discOutline } from "../common";
import {
  CONSOLE_WALL,
  ROUNDEL_FACE,
  interiorHalf,
  type InteriorRecipe,
} from "./common";
import type { InteriorKind } from "../../../world/types";

/**
 * The roundel wall's grid (C5, with the research note's size, C24a): each
 * roundel `across` overall, a rim ring `rim` wide round a face `face`
 * across, the ring `rimProud` proud of the slab and `sides` facets round.
 * `cols` by `rows` roundels, centres `pitchA` apart across (the middle
 * column on the piece's centre) and `pitchH` apart up, the lowest row's
 * centres at `low`.
 */
export const ROUNDEL = {
  across: 0.49,
  rim: 0.08,
  face: 0.33,
  rimProud: 0.015,
  sides: 10,
  pitchA: 0.66,
  pitchH: 0.72,
  low: 0.5,
  cols: 3,
  rows: 5,
} as const;

/** A glowing roundel's face: a soft warm white (C5). */
export const ROUNDEL_GLOW: Rgb = [1.0, 0.97, 0.88];

/**
 * The roundels that glow on each variant of the roundel wall, as
 * `[col, row]` cells of `ROUNDEL`'s grid (column 0 on the left, row 0 at
 * the bottom): none on variant 0, two each on variants 1 and 2 at
 * different places, so about two walls in three carry a few soft glows.
 */
export const GLOWING_ROUNDELS: readonly (readonly [
  col: number,
  row: number,
])[][] = [
  [],
  [
    [0, 2],
    [2, 3],
  ],
  [
    [1, 1],
    [2, 4],
  ],
];

/**
 * The inner doors (C6, the leaf height from the research note, C24b): two
 * leaves `leafWidth` by `leafHeight`, standing `proud` of the surround,
 * each with `cols` by `rows` roundels `across` overall (the wall's rim
 * round a smaller face, so every roundel keeps at least 0.05 m to its
 * leaf's edges and its neighbour) whose rows are `pitchH` apart round the
 * leaf's middle. A grey meeting stile `stile` wide over the join and a
 * dark frame line `frame` wide round the pair.
 */
export const INNER_DOORS = {
  leafWidth: 1.1,
  leafHeight: 2.7,
  proud: 0.03,
  cols: 2,
  rows: 4,
  across: 0.46,
  pitchH: 0.62,
  stile: 0.02,
  frame: 0.03,
} as const;

/** The inner doors' meeting stile: a light grey (C6). */
export const DOOR_STILE: Rgb = [0.7, 0.71, 0.7];

/** The dark line framing the inner doors' pair of leaves (C6). */
export const DOOR_FRAME: Rgb = [0.22, 0.23, 0.24];

/**
 * The scanner (C7): a screen `width` by `height` centred `centre` above
 * the floor, in a dark housing `bezel` wider on every side whose front
 * stands `proud` of the slab, with the screen set `recess` back into it.
 * `dots` small lit dots `dot` square on the screen.
 */
export const SCANNER = {
  width: 1.2,
  height: 0.8,
  centre: 1.8,
  bezel: 0.06,
  proud: 0.08,
  recess: 0.02,
  dots: 40,
  dot: 0.014,
} as const;

/** The scanner's housing: a dark grey (C7). */
export const SCANNER_HOUSING: Rgb = [0.3, 0.31, 0.32];

/** The scanner's screen: a pale blue (C7). */
export const SCANNER_GLOW: Rgb = [0.55, 0.7, 0.85];

/** The scanner's dots: a paler near-white on the screen (C7). */
export const SCANNER_DOT: Rgb = [0.85, 0.9, 0.95];

/** How far the dots keep from the screen's edge, in metres. */
const DOT_MARGIN = 0.05;

/**
 * The half side of a roundel's square face: halfway between the hole's
 * reach along an axis (the inner outline's corner on `+a`) and the
 * largest square whose corners stay inside the outer outline. The outline
 * starts on `+a` (`discOutline`'s default), so a corner of the square, on
 * a diagonal, lies a twentieth of a turn from the nearest edge's middle.
 */
function faceHalf(outer: number, inner: number, sides: number): number {
  const step = (2 * Math.PI) / sides;
  const apothem = outer * Math.cos(step / 2);
  const corner = Math.min(
    ...[1, 3, 5, 7].map((q) => {
      const t = (q * Math.PI) / 4;
      const mid = Math.round((t - step / 2) / step) * step + step / 2;
      return apothem / Math.cos(t - mid);
    }),
  );
  const most = corner / Math.SQRT2;
  if (most <= inner) throw new Error("faceHalf: no room under the rim");
  return (inner + most) / 2;
}

/**
 * One roundel centred at `(a, h)` on a face at depth `d`, `across`
 * overall: the rim ring from `d` out by `ROUNDEL.rimProud`, in two half
 * rings, and the face, one `DECAL_LIFT` off the slab, in `face`.
 */
function roundel(
  k: Kit,
  a: number,
  h: number,
  d: number,
  across: number,
  rim: Surface,
  face: Surface,
): void {
  const outer = across / 2;
  const inner = outer - ROUNDEL.rim;
  const n = ROUNDEL.sides;
  const o = discOutline(a, h, outer, n);
  const i = discOutline(a, h, inner, n);
  for (const from of [0, n / 2]) {
    const arc = Array.from({ length: n / 2 + 1 }, (_, j) => (from + j) % n);
    const ring = [...arc.map((j) => o[j]!), ...arc.reverse().map((j) => i[j]!)];
    k.extrude(ring, d, d + ROUNDEL.rimProud, rim);
  }
  const s = faceHalf(outer, inner, n);
  k.panel(a - s, a + s, d + DECAL_LIFT, h - s, h + s, face, 2 * s, 2 * s);
}

/** A roundel wall cell's centre `(a, h)`, column 0 on the left, row 0 at the bottom. */
const cellAt = (col: number, row: number) => ({
  a: (col - (ROUNDEL.cols - 1) / 2) * ROUNDEL.pitchA,
  h: ROUNDEL.low + row * ROUNDEL.pitchH,
});

/**
 * The front of every wall piece's slab: the roundel wall's depth less the
 * rim, so a roundel wall's rims end on its front plane. The inner doors
 * and the scanner share it, so their slabs run flush with the roundel
 * walls beside them and their leaves and housing stand out of it.
 */
const SLAB = interiorHalf("roundel-wall").d1 - ROUNDEL.rimProud;

/**
 * The slab and its grid of `ROUNDEL` roundels, less the cells `skip`
 * names (by the cell's centre), the cells `glowing` names lit on the
 * `breathe` bank, each on the group its cell's index gives. The slab is
 * the piece's own width and height (`interiorHalf(kind)`).
 */
function roundelSlab(
  r: Parameters<InteriorRecipe>[0],
  glowing: readonly (readonly [number, number])[],
  skip: (a: number, h: number) => boolean,
): void {
  const { k, s, kind } = r;
  const { hw, top } = interiorHalf(kind);
  const d = SLAB;
  const white = s.tinted(CONSOLE_WALL);
  const shaded = s.tinted(ROUNDEL_FACE);
  k.box(-hw, hw, 0, d, 0, top, white);
  for (let col = 0; col < ROUNDEL.cols; col++)
    for (let row = 0; row < ROUNDEL.rows; row++) {
      const { a, h } = cellAt(col, row);
      if (skip(a, h)) continue;
      const lit = glowing.some(([c, w]) => c === col && w === row);
      const face = lit
        ? s.blink(ROUNDEL_GLOW, (col + ROUNDEL.cols * row) % BLINK_GROUPS)
        : shaded;
      roundel(k, a, h, d, ROUNDEL.across, white, face);
    }
}

/**
 * The roundel wall (C5): a white slab of 3 by 5 recessed roundels, the
 * variant's `GLOWING_ROUNDELS` breathing a soft warm white.
 */
const roundelWall: InteriorRecipe = (r) => {
  roundelSlab(r, GLOWING_ROUNDELS[r.variant] ?? [], () => false);
};

/**
 * The inner doors (C6): the 4 m slab with a column of five roundels at
 * each side, at the spacing the neighbouring roundel walls keep (a column
 * `hw - (1 - pitchA)` out, as the next wall's near column stands), then
 * the two leaves on the floor, the frame line round them, the stile over
 * their join and each leaf's grid of roundels. Nothing glows.
 */
const innerDoors: InteriorRecipe = ({ k, s, kind }) => {
  const { hw, top } = interiorHalf(kind);
  const wallHw = interiorHalf("roundel-wall").hw;
  const d = SLAB;
  const white = s.tinted(CONSOLE_WALL);
  const shaded = s.tinted(ROUNDEL_FACE);
  k.box(-hw, hw, 0, d, 0, top, white);
  const side = hw - (wallHw - ROUNDEL.pitchA);
  for (const a of [-side, side])
    for (let row = 0; row < ROUNDEL.rows; row++)
      roundel(
        k,
        a,
        ROUNDEL.low + row * ROUNDEL.pitchH,
        d,
        ROUNDEL.across,
        white,
        shaded,
      );

  const D = INNER_DOORS;
  const w = D.leafWidth;
  const front = d + D.proud;
  const frame = s.tinted(DOOR_FRAME);
  const lineFront = d + D.proud / 2;
  k.box(-w - D.frame, -w, d, lineFront, 0, D.leafHeight + D.frame, frame);
  k.box(w, w + D.frame, d, lineFront, 0, D.leafHeight + D.frame, frame);
  k.box(-w, w, d, lineFront, D.leafHeight, D.leafHeight + D.frame, frame);

  const across = D.across;
  const gap = (w - D.cols * across) / (D.cols + 1);
  for (const sign of [-1, 1]) {
    const edge = sign * w;
    k.bevelBox(
      Math.min(0, edge),
      Math.max(0, edge),
      d,
      front,
      0,
      D.leafHeight,
      0.01,
      white,
    );
    for (let col = 0; col < D.cols; col++) {
      const a = sign * (w - gap - across / 2 - col * (across + gap));
      for (let row = 0; row < D.rows; row++) {
        const h = D.leafHeight / 2 + (row - (D.rows - 1) / 2) * D.pitchH;
        roundel(k, a, h, front, across, white, shaded);
      }
    }
  }
  k.box(
    -D.stile / 2,
    D.stile / 2,
    front,
    front + ROUNDEL.rimProud,
    0,
    D.leafHeight,
    s.tinted(DOOR_STILE),
  );
};

/**
 * The scanner's housing in the piece's `(a, h)`: the screen and its bezel
 * all round.
 */
export function scannerHousing(): {
  a0: number;
  a1: number;
  h0: number;
  h1: number;
} {
  const w = SCANNER.width / 2 + SCANNER.bezel;
  const h = SCANNER.height / 2 + SCANNER.bezel;
  return { a0: -w, a1: w, h0: SCANNER.centre - h, h1: SCANNER.centre + h };
}

/**
 * The scanner (C7): a roundel wall less every roundel its housing would
 * cover (C24c: the roundel's square bounds against the housing's box, so
 * the rows at 1.22 m and 1.94 m go in all three columns and nine
 * roundels stay), the dark housing (a body, and a bezel standing `recess`
 * out of it round the screen), the pale blue screen one `DECAL_LIFT` off
 * the body and its dots one more off the screen. The dots are spread by
 * the plastic sequence (two irrational steps), fixed, so every scanner
 * shows the same field.
 */
const scanner: InteriorRecipe = (r) => {
  const { k, s } = r;
  const box = scannerHousing();
  const half = ROUNDEL.across / 2;
  roundelSlab(
    r,
    [],
    (a, h) =>
      a - half < box.a1 &&
      a + half > box.a0 &&
      h - half < box.h1 &&
      h + half > box.h0,
  );
  const d0 = SLAB;
  const face = d0 + SCANNER.proud;
  const body = face - SCANNER.recess;
  const dark = s.tinted(SCANNER_HOUSING);
  k.box(box.a0, box.a1, d0, body, box.h0, box.h1, dark);
  const sw = SCANNER.width / 2;
  const s0 = SCANNER.centre - SCANNER.height / 2;
  const s1 = SCANNER.centre + SCANNER.height / 2;
  k.box(box.a0, box.a1, body, face, box.h0, s0, dark);
  k.box(box.a0, box.a1, body, face, s1, box.h1, dark);
  k.box(box.a0, -sw, body, face, s0, s1, dark);
  k.box(sw, box.a1, body, face, s0, s1, dark);
  const screen = body + DECAL_LIFT;
  k.panel(
    -sw,
    sw,
    screen,
    s0,
    s1,
    s.signal(SCANNER_GLOW),
    SCANNER.width,
    SCANNER.height,
  );
  const dot = s.signal(SCANNER_DOT);
  const g1 = 0.7548776662466927;
  const g2 = 0.5698402909980532;
  const spanA = SCANNER.width - 2 * DOT_MARGIN - SCANNER.dot;
  const spanH = SCANNER.height - 2 * DOT_MARGIN - SCANNER.dot;
  for (let i = 0; i < SCANNER.dots; i++) {
    const u = (0.5 + (i + 1) * g1) % 1;
    const v = (0.5 + (i + 1) * g2) % 1;
    const a = -sw + DOT_MARGIN + u * spanA;
    const h = s0 + DOT_MARGIN + v * spanH;
    k.panel(
      a,
      a + SCANNER.dot,
      screen + DECAL_LIFT,
      h,
      h + SCANNER.dot,
      dot,
      SCANNER.dot,
      SCANNER.dot,
    );
  }
};

/**
 * The wall pieces' recipes, for `interior/index.ts` to spread into its
 * table beside the console's.
 */
export const WALL_RECIPES = {
  "roundel-wall": roundelWall,
  "inner-doors": innerDoors,
  scanner,
} satisfies Record<Exclude<InteriorKind, "console">, InteriorRecipe>;
