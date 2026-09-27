/**
 * The approved marks (2.6d, 2.6f C16): every string sets in the
 * block-pixel font, and the six pictures keep the shapes their recipes
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
  TREFOIL_MARK,
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
    // Mutation caught: the band a solid bar, rows of unequal width, or a
    // letter-shaped glyph from the font pasted in as one of the runes.
    expect(HAMMER_RUNES.length).toBeGreaterThanOrEqual(3);
    const width = HAMMER_RUNES[0]?.length ?? 0;
    for (const r of HAMMER_RUNES) expect(r).toHaveLength(width);
    const lit = (c: number) => HAMMER_RUNES.some((r) => r[c] !== ".");
    let gaps = 0;
    const marks: [number, number][] = [];
    let start = -1;
    for (let c = 0; c <= width; c++) {
      const on = c < width && lit(c);
      if (on && start === -1) start = c;
      if (!on && start !== -1) {
        marks.push([start, c]);
        if (c > 0 && lit(c - 1) && !lit(c)) gaps++;
        start = -1;
      }
    }
    expect(gaps).toBeGreaterThanOrEqual(4);
    for (const [c0, c1] of marks) {
      const mark = HAMMER_RUNES.map((r) => r.slice(c0, c1));
      for (const glyph of Object.values(PIXEL_FONT))
        expect(mark.join(), `${String(c0)}-${String(c1)}`).not.toBe(
          glyph.join(),
        );
    }
  });

  it("cuts the fuel case's trefoil into three blades around a filled centre (2.6f C14)", () => {
    // Mutation caught: a solid disc with no blades, a blade drawn straight
    // down (the case's own angles keep the lower blades either side, never
    // beneath centre), or the shape losing its left-right mirror.
    expect(TREFOIL_MARK.length % 2).toBe(1);
    const width = TREFOIL_MARK[0]?.length ?? 0;
    for (const r of TREFOIL_MARK) expect(r).toHaveLength(width);
    for (const r of TREFOIL_MARK) expect([...r].reverse().join(""), r).toBe(r);
    const midRow = Math.floor(TREFOIL_MARK.length / 2);
    const midCol = Math.floor(width / 2);
    expect(TREFOIL_MARK[midRow]?.[midCol]).toBe("#");
    expect(TREFOIL_MARK[midRow - 2]?.includes("#")).toBe(false);
    expect(TREFOIL_MARK[0]?.[midCol]).toBe("#");
    expect(TREFOIL_MARK[TREFOIL_MARK.length - 1]?.[midCol]).toBe(".");
  });
});
