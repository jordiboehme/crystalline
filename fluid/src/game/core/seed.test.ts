import { describe, expect, it } from "vitest";

import { createRng, cyrb53, seedFor } from "./seed";

describe("cyrb53", () => {
  it("matches the reference values of the published function", () => {
    // From bryc/code jshash: cyrb53('a') and cyrb53('b') with seed 0.
    expect(cyrb53("a")).toBe(7929297801672961);
    expect(cyrb53("b")).toBe(8684336938537663);
  });

  it("stays inside the safe integer range", () => {
    for (const text of ["", "x", "a long permalink/with/segments"]) {
      const h = cyrb53(text);
      expect(Number.isSafeInteger(h)).toBe(true);
      expect(h).toBeGreaterThanOrEqual(0);
    }
  });
});

describe("seedFor", () => {
  it("is deterministic", () => {
    expect(seedFor(1, "eng", "notes/alpha")).toBe(
      seedFor(1, "eng", "notes/alpha"),
    );
  });

  it("does not confuse where the parts split", () => {
    expect(seedFor("a/b", "c")).not.toBe(seedFor("a", "b/c"));
    expect(seedFor("ab", "c")).not.toBe(seedFor("a", "bc"));
  });

  it("tells numbers from their digits' neighbours", () => {
    expect(seedFor(1, "x")).not.toBe(seedFor(2, "x"));
  });
});

describe("createRng", () => {
  it("replays the same sequence from the same seed", () => {
    const a = createRng(42);
    const b = createRng(42);
    const left = Array.from({ length: 20 }, () => a.next());
    const right = Array.from({ length: 20 }, () => b.next());
    expect(left).toEqual(right);
  });

  it("differs between seeds", () => {
    expect(createRng(1).next()).not.toBe(createRng(2).next());
  });

  it("keeps next() in [0, 1) and int() inside its bounds", () => {
    const rng = createRng(7);
    for (let i = 0; i < 2000; i++) {
      const x = rng.next();
      expect(x).toBeGreaterThanOrEqual(0);
      expect(x).toBeLessThan(1);
      const n = rng.int(3, 5);
      expect([3, 4, 5]).toContain(n);
    }
  });

  it("picks every item of a list eventually", () => {
    const rng = createRng(9);
    const seen = new Set<string>();
    for (let i = 0; i < 200; i++) seen.add(rng.pick(["a", "b", "c"]));
    expect([...seen].sort()).toEqual(["a", "b", "c"]);
  });

  it("refuses to pick from an empty list", () => {
    expect(() => createRng(1).pick([])).toThrow();
  });
});
