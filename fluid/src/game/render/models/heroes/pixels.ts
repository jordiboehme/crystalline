/**
 * The block-pixel font and the pixel pictures built from it: titles and
 * demos on the arcade cabinets' attract screens and marquees (H14), the
 * pocket console's two screen pictures (C16), the small "<=>" mark on
 * the back of the beige laptop's lid (C17), and the 2.6d props' marks
 * (`marks.ts`, 2.6d C16).
 *
 * A picture is a list of rows of characters, row 0 at the top, one
 * character per pixel; `.` is dark, any other character names a colour the
 * caller chooses. The font's glyphs use `#` for a lit cell. A picture is
 * drawn as geometry, not as a texture: `pixelPanel` lays one flat
 * `k.panel` quad (two triangles) per horizontal run of equal characters,
 * so a lit bar of pixels costs the same as one pixel and no texture layer
 * or text key is needed. That merge is what keeps a cabinet's text inside
 * the hero triangle budget. `pixelBoxes` lays the same runs as thin boxes
 * instead, for a mark too small to float a panel over its face. `fit`
 * centres a picture in a box with square pixels, and `blinkPicture` draws
 * one as blinking pixels, a group per column quarter: the one way a screen
 * that swaps two pictures is drawn.
 */

import type { Surface } from "../../geometry";
import type { Kit } from "../../kit";
import type { Rgb } from "../../looks";
import type { Surfaces } from "../common";

/**
 * The block-pixel font: `A` to `Z`, `a` to `z`, `0` to `9`, the space, a
 * one-pixel period and the three marks `<`, `=` and `>` (the laptop's
 * "<=>", C17), each glyph 5 rows of 3 cells from the top, `#` lit and `.`
 * dark (the period glyph's own single lit cell sits at its bottom
 * middle). It is the smallest grid a capital letter still reads in, the
 * size an old attract screen used, and every on-screen title of the
 * station is set in it.
 *
 * The lower case (2.6d, for a badge whose original wordmark is lower
 * case) shares one x-height: a short letter fills rows 2 to 4, an
 * ascender climbs to row 0, and `i` and `j` carry their dot in row 0 over
 * a dark row 1, their stems in rows 2 and 3 like the short letters, `j`
 * hooking left in row 4. A letter with a tail (`g`, `p`, `q`, `y`) sits a
 * row higher on purpose, its bowl in rows 1 to 3 and its tail in row 4,
 * since the grid has no room below the baseline. `s` and `z` mirror each
 * other. No lower-case glyph is its capital's shape.
 */
export const PIXEL_FONT: Readonly<Record<string, readonly string[]>> = {
  A: [".#.", "#.#", "###", "#.#", "#.#"],
  B: ["##.", "#.#", "##.", "#.#", "##."],
  C: [".##", "#..", "#..", "#..", ".##"],
  D: ["##.", "#.#", "#.#", "#.#", "##."],
  E: ["###", "#..", "##.", "#..", "###"],
  F: ["###", "#..", "##.", "#..", "#.."],
  G: [".##", "#..", "#.#", "#.#", ".##"],
  H: ["#.#", "#.#", "###", "#.#", "#.#"],
  I: ["###", ".#.", ".#.", ".#.", "###"],
  J: ["..#", "..#", "..#", "#.#", ".#."],
  K: ["#.#", "#.#", "##.", "#.#", "#.#"],
  L: ["#..", "#..", "#..", "#..", "###"],
  M: ["#.#", "###", "###", "#.#", "#.#"],
  N: ["##.", "#.#", "#.#", "#.#", "#.#"],
  O: [".#.", "#.#", "#.#", "#.#", ".#."],
  P: ["##.", "#.#", "##.", "#..", "#.."],
  Q: [".#.", "#.#", "#.#", "##.", ".##"],
  R: ["##.", "#.#", "##.", "#.#", "#.#"],
  S: [".##", "#..", ".#.", "..#", "##."],
  T: ["###", ".#.", ".#.", ".#.", ".#."],
  U: ["#.#", "#.#", "#.#", "#.#", "###"],
  V: ["#.#", "#.#", "#.#", "#.#", ".#."],
  W: ["#.#", "#.#", "###", "###", "#.#"],
  X: ["#.#", "#.#", ".#.", "#.#", "#.#"],
  Y: ["#.#", "#.#", ".#.", ".#.", ".#."],
  Z: ["###", "..#", ".#.", "#..", "###"],
  a: ["...", "...", ".##", "#.#", ".##"],
  b: ["#..", "#..", "##.", "#.#", "##."],
  c: ["...", "...", ".##", "#..", ".##"],
  d: ["..#", "..#", ".##", "#.#", ".##"],
  e: ["...", "...", ".##", "###", ".##"],
  f: [".##", ".#.", "###", ".#.", ".#."],
  g: ["...", ".##", "#.#", ".##", "##."],
  h: ["#..", "#..", "##.", "#.#", "#.#"],
  i: [".#.", "...", ".#.", ".#.", ".#."],
  j: ["..#", "...", "..#", "..#", "##."],
  k: ["#..", "#..", "#.#", "##.", "#.#"],
  l: ["#..", "#..", "#..", "#..", ".##"],
  m: ["...", "...", "###", "###", "#.#"],
  n: ["...", "...", "##.", "#.#", "#.#"],
  o: ["...", "...", ".#.", "#.#", ".#."],
  p: ["...", "##.", "#.#", "##.", "#.."],
  q: ["...", ".##", "#.#", ".##", "..#"],
  r: ["...", "...", ".##", "#..", "#.."],
  s: ["...", "...", ".##", ".#.", "##."],
  t: [".#.", ".#.", "###", ".#.", ".##"],
  u: ["...", "...", "#.#", "#.#", ".##"],
  v: ["...", "...", "#.#", "#.#", ".#."],
  w: ["...", "...", "#.#", "###", "###"],
  x: ["...", "...", "#.#", ".#.", "#.#"],
  y: ["...", "#.#", "#.#", ".##", "##."],
  z: ["...", "...", "##.", ".#.", ".##"],
  "0": ["###", "#.#", "#.#", "#.#", "###"],
  "1": [".#.", "##.", ".#.", ".#.", "###"],
  "2": ["##.", "..#", ".#.", "#..", "###"],
  "3": ["##.", "..#", ".#.", "..#", "##."],
  "4": ["#.#", "#.#", "###", "..#", "..#"],
  "5": ["###", "#..", "##.", "..#", "##."],
  "6": [".##", "#..", "###", "#.#", "###"],
  "7": ["###", "..#", ".#.", ".#.", ".#."],
  "8": ["###", "#.#", "###", "#.#", "###"],
  "9": ["###", "#.#", "###", "..#", "##."],
  " ": ["...", "...", "...", "...", "..."],
  ".": ["...", "...", "...", "...", ".#."],
  "<": ["..#", ".#.", "#..", ".#.", "..#"],
  "=": ["...", "###", "...", "###", "..."],
  ">": ["#..", ".#.", "..#", ".#.", "#.."],
};

/** One horizontal run of equal characters: its first column, its row (0 at the top) and its length. */
export interface PixelRun {
  col: number;
  row: number;
  len: number;
}

/**
 * Every horizontal run of characters equal to each other in `rows`, row by
 * row from the top and left to right, with the character each run is made
 * of: the one scan `pixelRuns`, `pixelPanel` and any other picture made of
 * rows of characters (a cabinet's block side art) share.
 */
export function runsOf(rows: readonly string[]): (PixelRun & { ch: string })[] {
  const runs: (PixelRun & { ch: string })[] = [];
  rows.forEach((line, row) => {
    let col = 0;
    while (col < line.length) {
      const ch = line[col] ?? ".";
      let end = col + 1;
      while (end < line.length && line[end] === ch) end++;
      runs.push({ col, row, len: end - col, ch });
      col = end;
    }
  });
  return runs;
}

/**
 * The horizontal runs of lit cells (`#`) of a glyph-style picture, row 0
 * at the top: `["#.#", "###"]` gives the two single cells of row 0 and the
 * full row 1. Together they cover every lit cell exactly once.
 */
export function pixelRuns(rows: readonly string[]): PixelRun[] {
  return runsOf(rows)
    .filter((r) => r.ch === "#")
    .map(({ col, row, len }) => ({ col, row, len }));
}

/**
 * A text's 5 rows in the block-pixel font: its glyphs side by side with
 * one dark column between each two, so `"AB"` is 5 rows of 7. Throws on a
 * character the font lacks: a title that cannot be drawn is a bug, not
 * something to skip silently.
 */
export function textRows(text: string): string[] {
  const glyphs = [...text].map((c) => {
    const g = PIXEL_FONT[c];
    if (!g) throw new Error(`textRows: no glyph for ${JSON.stringify(c)}`);
    return g;
  });
  return Array.from({ length: 5 }, (_, y) =>
    glyphs.map((g) => g[y] ?? "...").join("."),
  );
}

/**
 * Draws a pixel picture as flat quads facing `+d` at depth `d`: one
 * `k.panel` per horizontal run of equal characters whose surface
 * `surfaceOf` gives (a `null` leaves the run dark and draws nothing). Each
 * pixel is `px` square; column 0 starts at `a0` and row 0 (the top) ends
 * at `h1`, so the picture spans `a0` to `a0 + columns * px` and `h1 - rows
 * * px` to `h1`.
 */
export function pixelPanel(
  k: Kit,
  rows: readonly string[],
  a0: number,
  h1: number,
  px: number,
  d: number,
  surfaceOf: (ch: string) => Surface | null,
): void {
  for (const r of runsOf(rows)) {
    const s = surfaceOf(r.ch);
    if (s === null) continue;
    k.panel(
      a0 + r.col * px,
      a0 + (r.col + r.len) * px,
      d,
      h1 - (r.row + 1) * px,
      h1 - r.row * px,
      s,
    );
  }
}

/**
 * Draws a pixel picture as thin boxes standing on a face at depth `d0`:
 * the same runs `pixelPanel` lays, each one `k.box` from `d0` out to `d1`
 * (`surfaceOf` gives a run's surface, `null` leaves it dark), with the
 * same `a0`, `h1` and `px` placing. For a mark smaller than about 3 cm,
 * on a curio or on the designer tower (2.6d C16), where a panel lifted
 * `DECAL_LIFT` off its face would stand as far out as the mark is tall:
 * a box 1.5 mm proud sits on its face and reads as printed on it.
 */
export function pixelBoxes(
  k: Kit,
  rows: readonly string[],
  a0: number,
  h1: number,
  px: number,
  d0: number,
  d1: number,
  surfaceOf: (ch: string) => Surface | null,
): void {
  for (const r of runsOf(rows)) {
    const s = surfaceOf(r.ch);
    if (s === null) continue;
    k.box(
      a0 + r.col * px,
      a0 + (r.col + r.len) * px,
      d0,
      d1,
      h1 - (r.row + 1) * px,
      h1 - r.row * px,
      s,
    );
  }
}

/**
 * Where a pixel picture of `rows` lands when fitted into the box `a0..a1`
 * by `h0..h1`: square pixels as large as both extents allow, the picture
 * centred in the box. Returns the pixel size, the left edge and the top.
 */
export function fit(
  rows: readonly string[],
  a0: number,
  a1: number,
  h0: number,
  h1: number,
): { px: number; left: number; top: number } {
  const cols = rows[0]?.length ?? 0;
  const px = Math.min((a1 - a0) / cols, (h1 - h0) / rows.length);
  return {
    px,
    left: (a0 + a1) / 2 - (cols * px) / 2,
    top: (h0 + h1) / 2 + (rows.length * px) / 2,
  };
}

/**
 * Draws a pixel picture fitted into the box `a0..a1` by `h0..h1` at depth
 * `d` as blinking pixels: the columns from its first to its last lit one
 * are cut into four quarters, and quarter `q` blinks in group `group0 +
 * q`. `tintOf` gives a character's colour, or `null` for a dark one. The
 * arcade cabinets' attract screens (H14) and the pocket console's screen
 * (C16) swap two such pictures, one in groups 0 to 3 and one in 4 to 7.
 */
export function blinkPicture(
  k: Kit,
  s: Surfaces,
  rows: readonly string[],
  box: readonly [a0: number, a1: number, h0: number, h1: number],
  d: number,
  group0: number,
  tintOf: (ch: string) => Rgb | null,
): void {
  const { px, left, top } = fit(rows, ...box);
  const litCols = rows.flatMap((r) =>
    [...r].flatMap((ch, i) => (tintOf(ch) === null ? [] : [i])),
  );
  const lo = Math.min(...litCols);
  const span = Math.max(...litCols) + 1 - lo;
  for (let q = 0; q < 4; q++) {
    const c0 = lo + Math.round((q * span) / 4);
    const c1 = lo + Math.round(((q + 1) * span) / 4);
    pixelPanel(
      k,
      rows.map((r) => r.slice(c0, c1)),
      left + c0 * px,
      top,
      px,
      d,
      (ch) => {
        const t = tintOf(ch);
        return t === null ? null : s.blink(t, group0 + q);
      },
    );
  }
}
