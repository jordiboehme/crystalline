/**
 * The slab walker's shape tests: what `heroModels.test.ts` does not check
 * for every kind. Its two inner slabs stand on the floor and its two outer
 * slabs swing on their hinges, one foot forward and one back, just off the
 * floor; the four stand side by side across its width, the swung ones
 * outside; and one small indicator near the top of an inner slab is its
 * only light.
 */

import { describe, expect, it } from "vitest";

import { FLAG, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { partsOf, toLocal, type Part } from "../../modelChecks";

/** A part's points in the recipe's local `[a, d, h]`. */
const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));

/** The low end, high end and middle of some points along axis `i`. */
const span = (pts: readonly V3[], i: 0 | 1 | 2) => {
  const v = pts.map((q) => q[i]);
  return {
    lo: Math.min(...v),
    hi: Math.max(...v),
    mid: (Math.min(...v) + Math.max(...v)) / 2,
  };
};

describe("the slab walker's model", () => {
  const slabs = () =>
    partsOf("slab-walker").filter(
      (p) => span(local(p), 2).hi - span(local(p), 2).lo > 1.5,
    );

  it("stands the two inner slabs on the floor and swings the outer two, one foot forward and one back, just off the floor", () => {
    // Mutation caught: an outer slab left upright, both swung the same way,
    // or a foot on the floor (no mid-step).
    const all = slabs();
    const inner = all.filter((p) => span(local(p), 2).lo < 1e-6);
    const outer = all.filter((p) => span(local(p), 2).lo >= 1e-6);
    expect(inner).toHaveLength(2);
    expect(outer).toHaveLength(2);
    const feet = outer.map((p) =>
      local(p).reduce((m, q) => (q[2] < m[2] ? q : m)),
    );
    for (const f of feet) {
      expect(f[2]).toBeGreaterThan(0.05);
      expect(f[2]).toBeLessThan(0.15);
      expect(Math.abs(f[1])).toBeGreaterThan(0.4);
    }
    expect(feet.map((f) => Math.sign(f[1])).sort()).toEqual([-1, 1]);
  });

  it("sets the four slabs side by side across its width, the swung ones outside", () => {
    // Mutation caught: the inner and outer slabs swapped.
    const mids = slabs()
      .map((p) => span(local(p), 0).mid)
      .sort((x, y) => x - y);
    expect(mids).toHaveLength(4);
    [-0.35, -0.1167, 0.1167, 0.35].forEach((want, i) =>
      expect(mids[i]).toBeCloseTo(want, 2),
    );
    for (const p of slabs().filter((q) => span(local(q), 2).lo >= 1e-6))
      expect(Math.abs(span(local(p), 0).mid)).toBeGreaterThan(0.3);
  });

  it("glows at one small indicator near the top of an inner slab, and nowhere else", () => {
    // Mutation caught: a display strip added (the spec says none).
    const lit = partsOf("slab-walker").filter((p) => p.flag !== FLAG.lit);
    expect(lit).toHaveLength(1);
    const [light] = lit;
    if (light === undefined) throw new Error("one light");
    expect(light.flag).toBe(FLAG.signal);
    expect(span(local(light), 2).lo).toBeGreaterThan(1.5);
    expect(Math.abs(span(local(light), 0).mid)).toBeLessThan(0.22);
  });
});
