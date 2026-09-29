import { describe, expect, it } from "vitest";

import { MACHINE_KINDS } from "./generate";
import {
  ACCENT_COUNT,
  VARIANT_COUNTS,
  decorVariant,
  machineModelSeed,
  machineVariant,
  tagAccent,
  terminalVariant,
} from "./variants";
import type { DecorKind } from "./types";

describe("variant picks (2.7 C4, C5, C10)", () => {
  it("picks a machine's variant, model seed and accent from its tag alone", () => {
    // Mutation caught: a pick that reads anything but the tag (a room seed,
    // the kind's index), or a pick outside the kind's count.
    const tags = Array.from({ length: 500 }, (_, i) => `tag-${String(i)}`);
    expect(tags.length).toBeGreaterThan(0);
    for (const kind of MACHINE_KINDS)
      for (const tag of tags) {
        const v = machineVariant(tag, kind);
        expect(v).toBeGreaterThanOrEqual(0);
        expect(v).toBeLessThan(VARIANT_COUNTS.machine[kind]);
      }
    // Pinned: the canned bridge's REACTOR is fabricator variant 2.
    expect(machineVariant("reactor", "fabricator")).toBe(2);
    expect(machineModelSeed("reactor")).toBe(6706559873233248);
    expect(machineModelSeed("reactor")).not.toBe(machineModelSeed("reactors"));
    const accents = new Set(tags.map(tagAccent));
    expect([...accents].sort()).toEqual([0, 1, 2, 3, 4]);
    expect(ACCENT_COUNT).toBe(5);
  });

  it("keeps every count between 1 and 4", () => {
    // Mutation caught: a count of 0 (a modulo by zero) or above the spec's 4.
    const counts = [
      VARIANT_COUNTS.terminal,
      ...Object.values(VARIANT_COUNTS.machine),
      ...Object.values(VARIANT_COUNTS.decor),
    ];
    expect(counts.length).toBe(1 + 12 + 9);
    for (const n of counts) {
      expect(Number.isInteger(n)).toBe(true);
      expect(n).toBeGreaterThanOrEqual(1);
      expect(n).toBeLessThanOrEqual(4);
    }
  });

  it("picks a decor kind's variant from the room seed and the kind, and a terminal's from its seed, over the whole count", () => {
    // Mutation caught: a pick keyed by anything else (the pinned values
    // move), or one that misses a variant or leaves the count. That every
    // piece of a kind in one room shares its variant is `generate.test.ts`'s
    // one-variant test on the hub, since `decorVariant` never sees a piece.
    expect([1, 2, 3, 123].map((s) => decorVariant(s, "council-chair"))).toEqual(
      [0, 1, 1, 1],
    );
    expect([1, 2, 3, 7, 8, 9].map(terminalVariant)).toEqual([2, 1, 2, 2, 1, 0]);
    const seeds = Array.from({ length: 300 }, (_, i) => i + 1);
    for (const kind of Object.keys(VARIANT_COUNTS.decor) as DecorKind[]) {
      const seen = new Set(seeds.map((s) => decorVariant(s, kind)));
      expect(seen, kind).toEqual(
        new Set(
          Array.from({ length: VARIANT_COUNTS.decor[kind] }, (_, v) => v),
        ),
      );
    }
    expect(new Set(seeds.map(terminalVariant))).toEqual(
      new Set(Array.from({ length: VARIANT_COUNTS.terminal }, (_, v) => v)),
    );
  });
});
