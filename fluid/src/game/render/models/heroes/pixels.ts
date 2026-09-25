/**
 * The block-pixel font and the pixel pictures built from it: titles and
 * demos on the arcade cabinets' attract screens and marquees (H14).
 *
 * A picture is a list of rows of characters, row 0 at the top, one
 * character per pixel; `.` is dark, any other character names a colour the
 * caller chooses. The font's glyphs use `#` for a lit cell. A picture is
 * drawn as geometry, not as a texture: `pixelPanel` lays one flat
 * `k.panel` quad (two triangles) per horizontal run of equal characters,
 * so a lit bar of pixels costs the same as one pixel and no texture layer
 * or text key is needed. That merge is what keeps a cabinet's text inside
 * the hero triangle budget.
 */

import type { Surface } from "../../geometry";
import type { Kit } from "../../kit";

/**
 * The block-pixel font: `A` to `Z`, `0` to `9` and the space, each glyph
 * 5 rows of 3 cells from the top, `#` lit and `.` dark. It is the smallest
 * grid a capital letter still reads in, the size an old attract screen
 * used, and every on-screen title of the station is set in it.
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
 * of: the one scan both `pixelRuns` and `pixelPanel` share.
 */
function runsOf(rows: readonly string[]): (PixelRun & { ch: string })[] {
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
 * character the font lacks (lower case included): a title that cannot be
 * drawn is a bug, not something to skip silently.
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
