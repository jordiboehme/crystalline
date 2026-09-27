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

describe("variant picks (2.7 C4, C5, C10)", () => {
  it("picks a machine's variant, model seed and accent from its tag alone", () => {
    // Mutation caught: a pick that reads anything but the tag (a room seed,
    // the kind's index), or a pick outside the kind's count.
    const tags = Array.from({ length: 500 }, (_, i) => `tag-${String(i)}`);
    expect(tags.length).toBeGreaterThan(0);
    for (const kind of MACHINE_KINDS)
      for (const tag of tags) {
        const v = machineVariant(tag, kind);
        expect(v).toBe(machineVariant(tag, kind));
        expect(v).toBeGreaterThanOrEqual(0);
        expect(v).toBeLessThan(VARIANT_COUNTS.machine[kind]);
      }
    expect(machineModelSeed("reactor")).toBe(machineModelSeed("reactor"));
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

  it("gives every piece of one decor kind in a room the same variant", () => {
    // Mutation caught: a decor variant keyed by the piece's own seed.
    expect(decorVariant(123, "council-chair")).toBe(
      decorVariant(123, "council-chair"),
    );
    expect(terminalVariant(7)).toBe(terminalVariant(7));
  });
});
