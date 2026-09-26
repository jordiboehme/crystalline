/**
 * The approved marks (2.6d C16): every string sets in the block-pixel
 * font, and the three pictures keep the shapes their recipes rely on.
 */

import { describe, expect, it } from "vitest";

import { textRows } from "./heroes/pixels";
import { CAPSULE_LOGO, MARKS, SIGNATURE, SPACE_BADGE } from "./marks";

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
});
