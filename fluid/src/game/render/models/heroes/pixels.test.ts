/**
 * The block-pixel font's tests: every glyph is a clean 3 by 5 grid, the
 * runs cover exactly the lit cells, a text's rows keep one dark column
 * between glyphs, and a character the font lacks is refused.
 */

import { describe, expect, it } from "vitest";

import { PIXEL_FONT, pixelRuns, textRows } from "./pixels";

/** The on-screen titles the cabinets carry: every letter must exist. */
const TITLES = ["TILEFALL", "ROCK RAIN", "MAZE HUNT", "VOID WING"];

describe("block-pixel font", () => {
  it("draws every glyph as 5 rows of 3 lit or dark cells", () => {
    const keys = Object.keys(PIXEL_FONT);
    for (const c of "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 .")
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

  it("draws the period as a single lit cell at the bottom middle", () => {
    expect(PIXEL_FONT["."]).toEqual(["...", "...", "...", "...", ".#."]);
  });

  it("refuses a character the font lacks", () => {
    expect(() => textRows("A?")).toThrow(/\?/);
    expect(() => textRows("a")).toThrow();
  });
});
