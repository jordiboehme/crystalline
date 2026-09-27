import { describe, expect, it } from "vitest";

import {
  BLINK_BANKS,
  BLINK_CHANNELS,
  BLINK_GROUPS,
  BLINK_LOW,
  CHASE_TICS,
  SOFT_FLOOR,
  SWAP_TICS,
  bankSlot,
  createBlink,
} from "./blink";
import { FLAG, blinkFlag } from "./geometry";

const at = (
  g: Float32Array,
  bank: (typeof BLINK_BANKS)[number],
  group: number,
) => g[bankSlot(bank) * BLINK_GROUPS + group] ?? NaN;

describe("blink banks", () => {
  it("has slot 0 as the steady bank, all 1, on every tick", () => {
    expect(bankSlot("steady")).toBe(0);
    expect(BLINK_CHANNELS).toBe(BLINK_BANKS.length * BLINK_GROUPS);
    const b = createBlink();
    for (let t = 0; t < 400; t++, b.tick())
      for (let g = 0; g < BLINK_GROUPS; g++)
        expect(at(b.gains, "steady", g)).toBe(1);
  });

  it("keeps every gain between BLINK_LOW and 1", () => {
    const b = createBlink();
    for (let t = 0; t < 400; t++, b.tick())
      for (const v of b.gains) {
        expect(v).toBeGreaterThanOrEqual(BLINK_LOW - 1e-6);
        expect(v).toBeLessThanOrEqual(1 + 1e-6);
      }
  });

  it("swaps groups 0-3 against 4-7 every SWAP_TICS, never both lit", () => {
    const b = createBlink();
    let flips = 0;
    let was = at(b.gains, "swap", 0);
    for (let t = 0; t < 4 * SWAP_TICS; t++, b.tick()) {
      const first = at(b.gains, "swap", 0);
      const second = at(b.gains, "swap", 4);
      expect(first === 1 && second === 1).toBe(false);
      expect(Math.max(first, second)).toBe(1);
      for (let g = 1; g < 4; g++) expect(at(b.gains, "swap", g)).toBe(first);
      for (let g = 5; g < 8; g++) expect(at(b.gains, "swap", g)).toBe(second);
      if (first !== was) flips++;
      was = first;
    }
    expect(flips).toBe(3);
  });

  it("chases one lit group round the bank, one step every CHASE_TICS", () => {
    const b = createBlink();
    const lit: number[] = [];
    for (let t = 0; t < BLINK_GROUPS * CHASE_TICS; t += CHASE_TICS) {
      const on = Array.from({ length: BLINK_GROUPS }, (_, g) =>
        at(b.gains, "chase", g),
      ).flatMap((v, g) => (v === 1 ? [g] : []));
      expect(on).toHaveLength(1);
      lit.push(on[0] ?? -1);
      for (let k = 0; k < CHASE_TICS; k++) b.tick();
    }
    expect(lit).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
  });

  it("moves the breathe, status and twinkle banks, the same way on every state", () => {
    const a = createBlink();
    const b = createBlink();
    for (const bank of ["breathe", "status", "twinkle"] as const) {
      const seen = new Set<number>();
      const a2 = createBlink();
      for (let t = 0; t < 350; t++, a2.tick()) seen.add(at(a2.gains, bank, 3));
      expect(seen.size, bank).toBeGreaterThan(1);
    }
    for (let t = 0; t < 350; t++, a.tick(), b.tick())
      expect(Array.from(a.gains)).toEqual(Array.from(b.gains));
  });
});

describe("the DOOM banks out of step", () => {
  it("has every DOOM bank's groups differ on at least one tick in 700", () => {
    for (const bank of ["breathe", "status", "twinkle"] as const) {
      const b = createBlink();
      let differ = 0;
      for (let t = 0; t < 700; t++, b.tick()) {
        const first = at(b.gains, bank, 0);
        for (let g = 1; g < BLINK_GROUPS; g++)
          if (at(b.gains, bank, g) !== first) {
            differ++;
            break;
          }
      }
      expect(differ, bank).toBeGreaterThan(0);
    }
  });

  it("breathes every group through the same glow, each at its own phase", () => {
    // Each breathe group runs DOOM's glow: the same set of levels, only
    // shifted in time, so a group's run over one long stretch holds the
    // same values as group 0's.
    const b = createBlink();
    const seen = Array.from({ length: BLINK_GROUPS }, () => new Set<number>());
    for (let t = 0; t < 700; t++, b.tick())
      for (let g = 0; g < BLINK_GROUPS; g++)
        seen[g]?.add(at(b.gains, "breathe", g));
    const ref = [...(seen[0] ?? [])].sort().join();
    for (let g = 1; g < BLINK_GROUPS; g++)
      expect([...(seen[g] ?? [])].sort().join(), String(g)).toBe(ref);
    expect(seen[0]?.size).toBeGreaterThan(10);
  });
});

describe("the soft bank (2.6e C26)", () => {
  it("breathes with the breathe bank's glow, group for group, lifted to run from SOFT_FLOOR to 1", () => {
    // Mutation caught: the soft bank left at the glow's own low level (a
    // light on it dipping far under its peak), never reaching 1, or out of
    // step with the breathe bank's phases.
    expect(SOFT_FLOOR).toBe(0.9);
    const b = createBlink();
    let lo = Infinity;
    let hi = -Infinity;
    const pairs: [number, number][] = [];
    for (let t = 0; t < 700; t++, b.tick())
      for (let g = 0; g < BLINK_GROUPS; g++) {
        const soft = at(b.gains, "soft", g);
        lo = Math.min(lo, soft);
        hi = Math.max(hi, soft);
        pairs.push([at(b.gains, "breathe", g), soft]);
      }
    expect(lo).toBeCloseTo(SOFT_FLOOR, 6);
    expect(hi).toBeCloseTo(1, 6);
    // The same glow at the same phase: ordered by the breathe gain, the
    // soft gain never goes down, and it moves whenever the breathe does.
    pairs.sort((x, y) => x[0] - y[0] || x[1] - y[1]);
    for (let i = 1; i < pairs.length; i++) {
      const [b0, s0] = pairs[i - 1] ?? [0, 0];
      const [b1, s1] = pairs[i] ?? [0, 0];
      expect(s1).toBeGreaterThanOrEqual(s0 - 1e-9);
      if (b1 === b0) expect(s1).toBeCloseTo(s0, 9);
    }
  });
});

describe("blinkFlag", () => {
  it("maps groups 0 to 7 onto FLAG.blink onwards and refuses any other group", () => {
    for (let g = 0; g < BLINK_GROUPS; g++)
      expect(blinkFlag(g)).toBe(FLAG.blink + g);
    for (const g of [-1, BLINK_GROUPS, 0.5])
      expect(() => blinkFlag(g)).toThrow(/no group/);
    expect(FLAG.signal).toBeLessThan(FLAG.blink);
  });
});
