/**
 * The approved marks (2.6d, 2.6f C16): every string sets in the
 * block-pixel font, and the five pictures keep the shapes their recipes
 * rely on.
 */

import { describe, expect, it } from "vitest";

import { PIXEL_FONT, textRows } from "./heroes/pixels";
import {
  CAPSULE_LOGO,
  COMPUTER_LOGO,
  HAMMER_RUNES,
  MARKS,
  SIGNATURE,
  SPACE_BADGE,
  stencilMarks,
} from "./marks";

const strings = (): string[] =>
  Object.values(MARKS).flatMap((v): string[] =>
    typeof v === "string" ? [v] : [...v],
  );

describe("the approved marks", () => {
  it("sets every string in the block-pixel font", () => {
    // Mutation caught: a character the font lacks, in any mark.
    for (const t of strings()) expect(() => textRows(t), t).not.toThrow();
  });

  it("draws the round C open to the right, in a ring symmetric top to bottom", () => {
    // Mutation caught: the C closed into an O, or the ring lopsided.
    expect(CAPSULE_LOGO).toHaveLength(11);
    for (const row of CAPSULE_LOGO) expect(row).toHaveLength(11);
    expect([...CAPSULE_LOGO].reverse()).toEqual(CAPSULE_LOGO);
    for (const row of CAPSULE_LOGO) {
      expect(row.slice(7).includes("#"), row).toBe(false);
      const ring = row.replaceAll("#", ".");
      expect([...ring].reverse().join(""), row).toBe(ring);
    }
    expect(CAPSULE_LOGO.some((r) => r.includes("#"))).toBe(true);
  });

  it("draws the signature as one unbroken stroke across every column", () => {
    // Mutation caught: a gap that breaks the stroke into letters.
    const cols = SIGNATURE[0]?.length ?? 0;
    for (let c = 0; c < cols; c++)
      expect(
        SIGNATURE.some((r) => r[c] === "#"),
        `column ${String(c)}`,
      ).toBe(true);
    for (let c = 1; c < cols; c++) {
      const rows = (x: number) =>
        SIGNATURE.flatMap((r, y) => (r[x] === "#" ? [y] : []));
      expect(
        rows(c).some((y) => rows(c - 1).some((z) => Math.abs(y - z) <= 1)),
        `column ${String(c)}`,
      ).toBe(true);
    }
  });

  it("crosses the badge's planet with its orbit", () => {
    // Mutation caught: the orbit drawn apart from the planet.
    expect(SPACE_BADGE).toHaveLength(5);
    expect(SPACE_BADGE.join("").includes("p")).toBe(true);
    expect(SPACE_BADGE.join("").includes("o")).toBe(true);
    expect(SPACE_BADGE.some((r) => /p.*o|o.*p/.test(r))).toBe(true);
  });

  it("draws the computers' logo as a C open to the right with a flag at each end of its mouth", () => {
    // Mutation caught: the C closed, the mouth filled, a flag dropped, or
    // the logo lopsided top to bottom.
    expect(COMPUTER_LOGO).toHaveLength(5);
    for (const row of COMPUTER_LOGO) expect(row).toHaveLength(6);
    expect([...COMPUTER_LOGO].reverse()).toEqual(COMPUTER_LOGO);
    const mid = COMPUTER_LOGO[2] ?? "";
    expect(mid[0]).toBe("#");
    expect(mid.slice(1).includes("#")).toBe(false);
    for (const y of [1, 2, 3]) expect(COMPUTER_LOGO[y]?.[0]).toBe("#");
    for (const y of [0, 1, 3, 4])
      expect(COMPUTER_LOGO[y]?.slice(3).includes("#"), `row ${String(y)}`).toBe(
        true,
      );
  });

  it("draws the hammer's runes as one band of separate marks, none a letter (2.6f C14, C15)", () => {
    // Mutation caught: the band a solid bar, rows of unequal width, a
    // colour key other than the one ink (a stray character `lit` would
    // still count as lit), a band of four marks with one widened to five
    // columns, or a letter-shaped glyph from the font pasted in as one of
    // the runes.
    expect(HAMMER_RUNES).toHaveLength(5);
    const width = HAMMER_RUNES[0]?.length ?? 0;
    expect(width).toBe(14);
    for (const r of HAMMER_RUNES) {
      expect(r).toHaveLength(width);
      expect(r).toMatch(/^[#.]+$/);
    }
    const lit = (c: number) => HAMMER_RUNES.some((r) => r[c] !== ".");
    const marks: [number, number][] = [];
    let start = -1;
    for (let c = 0; c <= width; c++) {
      const on = c < width && lit(c);
      if (on && start === -1) start = c;
      if (!on && start !== -1) {
        marks.push([start, c]);
        start = -1;
      }
    }
    expect(marks.map(([a, b]) => b - a)).toEqual([2, 2, 2, 2, 2]);
    for (const [c0, c1] of marks) {
      const mark = HAMMER_RUNES.map((r) => r.slice(c0, c1));
      for (const glyph of Object.values(PIXEL_FONT))
        expect(mark.join(), `${String(c0)}-${String(c1)}`).not.toBe(
          glyph.join(),
        );
    }
  });

  it("sets a stencil word by word and character by character, the digits in order (2.7 C19)", () => {
    // Mutation caught: the digits reversed, the letter left out or put on
    // the deck line, the deck line missing on a wall stencil, or the words
    // swapped.
    expect(
      stencilMarks({ deck: 23, bay: 7, letter: 0, lines: 2 }).map((l) =>
        l.join("|"),
      ),
    ).toEqual(["DECK|2|3", "BAY|7"]);
    expect(
      stencilMarks({ deck: 5, bay: 40, letter: 2, lines: 1 }).map((l) =>
        l.join("|"),
      ),
    ).toEqual(["BAY|4|0|B"]);
    expect(MARKS.numerals).toHaveLength(10);
    expect(MARKS.bayLetters).toHaveLength(4);
  });

  it("refuses a stencil it cannot set (2.7 C19)", () => {
    // Mutation caught: a letter past D or a number past 99 set anyway,
    // from an entry that is not there (an `undefined` mark).
    expect(() =>
      stencilMarks({ deck: 1, bay: 1, letter: 5, lines: 1 }),
    ).toThrow();
    expect(() =>
      stencilMarks({ deck: 100, bay: 1, letter: 0, lines: 2 }),
    ).toThrow();
    expect(() =>
      stencilMarks({ deck: 1, bay: 0, letter: 0, lines: 1 }),
    ).toThrow();
  });
});
