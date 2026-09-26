/**
 * The mechs' shape tests: the head's one horn sweeps forward and up over
 * two eyes; the tank's four legs end in wheels on the floor and its lower
 * lens is the largest; the garden robot's arms hang almost to the floor
 * and it has one eye.
 */

import { describe, expect, it } from "vitest";

import { FLAG, blinkFlag, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { partsOf, toLocal, type Part } from "../../modelChecks";

const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));
const lo = (p: Part) => Math.min(...local(p).map((q) => q[2]));
const hi = (p: Part) => Math.max(...local(p).map((q) => q[2]));
const extent = (pts: V3[], i: 0 | 1 | 2) =>
  Math.max(...pts.map((q) => q[i])) - Math.min(...pts.map((q) => q[i]));

describe("mech hero models", () => {
  it("sweeps the head's single horn forward and up, over two eyes", () => {
    // Mutation caught: the horn swept back, or a second horn.
    const parts = partsOf("mech-head");
    const top = Math.max(...parts.map(hi));
    const horn = parts.filter((p) => hi(p) > top - 0.05);
    expect(horn).toHaveLength(1);
    const pts = local(horn[0] as Part);
    const tip = pts.reduce((m, q) => (q[2] > m[2] ? q : m));
    const root = pts.reduce((m, q) => (q[2] < m[2] ? q : m));
    expect(tip[1]).toBeGreaterThan(root[1] + 0.2);
    expect(
      parts.filter((p) => p.flag === blinkFlag(0) || p.flag === blinkFlag(1)),
    ).toHaveLength(2);
  });

  it("stands the tank on four wheels and makes its lower lens the largest", () => {
    // Mutation caught: a leg off the floor, the lenses the wrong way round,
    // or a recipe that ignores `r` (measured from the built glow discs, not
    // read off the table, so a fixed radius cannot pass).
    const wheels = partsOf("spider-tank").filter(
      (p) => p.method === "extrude" && lo(p) < 1e-4,
    );
    expect(wheels).toHaveLength(4);
    const lenses = partsOf("spider-tank").filter((p) => p.flag >= FLAG.blink);
    expect(lenses).toHaveLength(3);
    const radius = (p: Part) => extent(local(p), 0) / 2;
    const height = (p: Part) => (lo(p) + hi(p)) / 2;
    const lowest = [...lenses].sort((x, y) => height(x) - height(y))[0];
    if (lowest === undefined) throw new Error("no lens");
    for (const l of lenses)
      if (l !== lowest) expect(radius(lowest)).toBeGreaterThan(radius(l));
  });

  it("hangs the garden robot's arms almost to the floor and gives it one eye", () => {
    // Mutation caught: short arms (a wrist 0.2 m higher lifts the fingers
    // from 0.1 m to 0.3 m), one arm missing, or a second eye.
    const parts = partsOf("garden-robot");
    const arms = parts.filter((p) => {
      const pts = local(p);
      return Math.min(...pts.map((q) => Math.abs(q[0]))) > 0.62 && lo(p) < 0.2;
    });
    for (const sign of [-1, 1])
      expect(
        arms.filter((p) => Math.sign(local(p)[0]?.[0] ?? 0) === sign).length,
      ).toBeGreaterThan(0);
    expect(parts.filter((p) => p.flag >= FLAG.blink)).toHaveLength(1);
  });
});
