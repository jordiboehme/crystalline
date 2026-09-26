/**
 * The block-pixel font's tests: every glyph is a clean 3 by 5 grid, the
 * runs cover exactly the lit cells, a text's rows keep one dark column
 * between glyphs, each lower-case letter keeps its own shape on the
 * font's x-height, a character the font lacks is refused, and
 * `pixelBoxes` lays one thin box per run.
 */

import { describe, expect, it } from "vitest";

import { FLAG, createBuilder } from "../../geometry";
import { frameAt } from "../../kit";
import { recordingKitAt, type Part } from "../../modelChecks";
import { PIXEL_FONT, pixelBoxes, pixelRuns, textRows } from "./pixels";

/** The on-screen titles the cabinets carry: every letter must exist. */
const TITLES = ["TILEFALL", "ROCK RAIN", "MAZE HUNT", "VOID WING"];

describe("block-pixel font", () => {
  it("draws every glyph as 5 rows of 3 lit or dark cells", () => {
    const keys = Object.keys(PIXEL_FONT);
    for (const c of "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 .<=>")
      expect(keys, c).toContain(c);
    for (const [c, rows] of Object.entries(PIXEL_FONT)) {
      expect(rows, c).toHaveLength(5);
      for (const r of rows) expect(r, c).toMatch(/^[#.]{3}$/);
    }
    for (const t of TITLES)
      for (const c of t) expect(PIXEL_FONT[c], `${t} ${c}`).toBeDefined();
  });

  it("covers exactly the lit cells with horizontal runs", () => {
    expect(pixelRuns(["#.#", "###"])).toEqual([
      { col: 0, row: 0, len: 1 },
      { col: 2, row: 0, len: 1 },
      { col: 0, row: 1, len: 3 },
    ]);
    expect(pixelRuns(["...", ".##", "##."])).toEqual([
      { col: 1, row: 1, len: 2 },
      { col: 0, row: 2, len: 2 },
    ]);
    for (const [c, rows] of Object.entries(PIXEL_FONT)) {
      const lit = new Set<string>();
      for (const r of pixelRuns(rows))
        for (let i = 0; i < r.len; i++)
          lit.add(`${String(r.col + i)},${String(r.row)}`);
      const want = new Set<string>();
      rows.forEach((row, y) => {
        [...row].forEach((ch, x) => {
          if (ch === "#") want.add(`${String(x)},${String(y)}`);
        });
      });
      expect(lit, c).toEqual(want);
    }
  });

  it("sets a text's glyphs side by side with one dark column between them", () => {
    const rows = textRows("AB");
    expect(rows).toHaveLength(5);
    for (const r of rows) expect(r).toHaveLength(7);
    rows.forEach((r, y) => {
      expect(r.slice(0, 3)).toBe(PIXEL_FONT.A?.[y]);
      expect(r[3]).toBe(".");
      expect(r.slice(4)).toBe(PIXEL_FONT.B?.[y]);
    });
  });

  it("sets the laptop's mark from its three glyphs", () => {
    const rows = textRows("<=>");
    expect(rows).toEqual([
      "..#.....#..",
      ".#..###..#.",
      "#.........#",
      ".#..###..#.",
      "..#.....#..",
    ]);
    for (const c of "<=>") {
      const g = PIXEL_FONT[c];
      expect(g, c).toHaveLength(5);
      for (const r of g ?? []) expect(r, c).toMatch(/^[#.]{3}$/);
    }
  });

  it("draws the period as a single lit cell at the bottom middle", () => {
    expect(PIXEL_FONT["."]).toEqual(["...", "...", "...", "...", ".#."]);
  });

  it("refuses a character the font lacks", () => {
    expect(() => textRows("A?")).toThrow(/\?/);
    expect(() => textRows("a!")).toThrow(/!/);
  });

  it("lays one box per run with pixelBoxes, from d0 to d1", () => {
    // Mutation caught: a box per pixel (the merge lost), or the depth span
    // taken from the wrong arguments. At turn 0 the frame's inward (+d)
    // is world -z, so d 0.1 to 0.1015 is z -0.1015 to -0.1.
    const builder = createBuilder();
    const parts: Part[] = [];
    const k = recordingKitAt(builder, parts)(frameAt([0, 0, 0], 0));
    const rows = textRows("40");
    pixelBoxes(k, rows, 0, 0.05, 0.01, 0.1, 0.1015, (ch) =>
      ch === "#" ? { layer: 0, tint: [1, 0, 0], flag: FLAG.lit } : null,
    );
    expect(parts).toHaveLength(pixelRuns(rows).length);
    for (const p of parts) {
      const ds = p.points.map((q) => q[2]);
      expect(p.method).toBe("box");
      expect(Math.min(...ds)).toBeCloseTo(-0.1015, 6);
      expect(Math.max(...ds)).toBeCloseTo(-0.1, 6);
    }
  });
});

/**
 * Every lower-case glyph, pinned row by row (2.6d, the computers' lower-case
 * badge). The letters share one x-height: a short letter fills rows 2 to 4,
 * an ascender climbs to row 0, a dotted letter carries its dot in row 0
 * over a dark row 1, and a letter with a tail sits a row higher on
 * purpose, its bowl in rows 1 to 3 and its tail in row 4 (the 5-row grid
 * has no room below the baseline).
 */
const LOWER: Readonly<Record<string, readonly string[]>> = {
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
};

const ASCENDERS = "bdfhklt";
const DOTTED = "ij";
const TAILED = "gpqy";

describe("the block-pixel font's lower case", () => {
  it("holds the whole lower-case alphabet", () => {
    // Mutation caught: a lower-case glyph missing from the font, or one
    // the font has and this file does not pin.
    expect(
      Object.keys(PIXEL_FONT)
        .filter((k) => /[a-z]/.test(k))
        .sort()
        .join(""),
    ).toBe("abcdefghijklmnopqrstuvwxyz");
  });

  it.each(Object.keys(LOWER))(
    "draws %s in its own shape on the x-height",
    (c) => {
      // Mutation caught: a glyph edited or dropped, a lower-case letter drawn
      // as its capital (the badge would read as capitals), two letters
      // sharing one shape, or a letter off the shared x-height.
      const g = PIXEL_FONT[c];
      expect(g, c).toEqual(LOWER[c]);
      if (g === undefined) return;
      expect(g, c).not.toEqual(PIXEL_FONT[c.toUpperCase()]);
      for (const [other, rows] of Object.entries(PIXEL_FONT))
        if (other !== c) expect(rows, `${c} ${other}`).not.toEqual(g);
      const lit = (y: number) => (g[y] ?? "").includes("#");
      if (ASCENDERS.includes(c)) {
        expect(lit(0) && lit(4), c).toBe(true);
      } else if (DOTTED.includes(c)) {
        expect(lit(0) && !lit(1) && lit(2) && lit(4), c).toBe(true);
      } else if (TAILED.includes(c)) {
        expect(!lit(0) && lit(1) && lit(4), c).toBe(true);
      } else {
        expect(!lit(0) && !lit(1) && lit(2) && lit(4), c).toBe(true);
      }
    },
  );
});
