import { describe, expect, it } from "vitest";

import { seedFor } from "../core/seed";
import { WALL_PATTERNS, finishFor, plainFinish } from "./finish";
import { ACCENT_COUNT } from "./variants";

describe("a room's finish (2.7 C8, C11)", () => {
  it("picks the accent and the hall's pattern by seed, and gives every bay and the corridor another pattern (2.7 C8, C11)", () => {
    // Mutation caught: a bay allowed the hall's pattern, a pick outside its
    // range, or a pick that reads a stream instead of seedFor.
    const seeds = Array.from({ length: 2000 }, (_, i) => seedFor("finish", i));
    expect(seeds.length).toBeGreaterThan(0);
    const accents = new Set<number>();
    const halls = new Set<number>();
    for (const s of seeds) {
      const f = finishFor(s, 4);
      expect(f.accent).toBeGreaterThanOrEqual(0);
      expect(f.accent).toBeLessThan(ACCENT_COUNT);
      accents.add(f.accent);
      halls.add(f.hallWalls);
      expect(f.bayWalls).toHaveLength(4);
      for (const b of [...f.bayWalls, f.corridorWalls]) {
        expect(b).not.toBe(f.hallWalls);
        expect(b).toBeGreaterThanOrEqual(0);
        expect(b).toBeLessThan(WALL_PATTERNS);
      }
      // A bay's pick never depends on how many bays come after it.
      expect(finishFor(s, 2).bayWalls).toEqual(f.bayWalls.slice(0, 2));
    }
    expect(accents.size).toBe(ACCENT_COUNT);
    expect(halls.size).toBe(WALL_PATTERNS);
    // Pinned, so a pick that reads anything but its seed moves them.
    expect(finishFor(seedFor("finish", 0), 4)).toEqual({
      accent: 1,
      hallWalls: 2,
      bayWalls: [0, 1, 1, 0],
      corridorWalls: 1,
    });
    expect(finishFor(seedFor("finish", 1), 4)).toEqual({
      accent: 0,
      hallWalls: 1,
      bayWalls: [2, 2, 2, 0],
      corridorWalls: 0,
    });
  });

  it("gives a hand-built room accent 0 and pattern 0 on every wall", () => {
    // Mutation caught: a plain finish that drops or adds a bay, or paints a
    // wall in another pattern.
    expect(plainFinish(3)).toEqual({
      accent: 0,
      hallWalls: 0,
      bayWalls: [0, 0, 0],
      corridorWalls: 0,
    });
    expect(plainFinish(0).bayWalls).toEqual([]);
  });
});
