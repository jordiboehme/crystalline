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
import { TANK_LENSES } from "./mechs";

const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));
const lo = (p: Part) => Math.min(...local(p).map((q) => q[2]));
const hi = (p: Part) => Math.max(...local(p).map((q) => q[2]));

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
    // Mutation caught: a leg off the floor, or the lenses the wrong way round.
    const wheels = partsOf("spider-tank").filter(
      (p) => p.method === "extrude" && lo(p) < 1e-4,
    );
    expect(wheels).toHaveLength(4);
    expect(TANK_LENSES).toHaveLength(3);
    const lowest = [...TANK_LENSES].sort((x, y) => x.h - y.h)[0];
    for (const l of TANK_LENSES)
      if (l !== lowest) expect(lowest?.r ?? 0).toBeGreaterThan(l.r);
    expect(
      partsOf("spider-tank").filter((p) => p.flag >= FLAG.blink),
    ).toHaveLength(3);
  });

  it("hangs the garden robot's arms almost to the floor and gives it one eye", () => {
    // Mutation caught: short arms, or a second eye.
    const parts = partsOf("garden-robot");
    const arms = parts.filter((p) => {
      const pts = local(p);
      return Math.min(...pts.map((q) => Math.abs(q[0]))) > 0.62 && lo(p) < 0.4;
    });
    expect(arms.length).toBeGreaterThanOrEqual(2);
    expect(parts.filter((p) => p.flag >= FLAG.blink)).toHaveLength(1);
  });
});
