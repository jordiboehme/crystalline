/**
 * The exhibits' shape tests: the hand has no light and holds four fingers
 * and a thumb, the rocket's hull is a true chequer with three fins on its
 * plinth and four portholes centred on its front, and the hammer's head
 * is its true size with its crack flat on the floor.
 */

import { describe, expect, it } from "vitest";

import { FLAG, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { GLOWING, partsOf, toLocal, type Part } from "../../modelChecks";
import { HAMMER_HEAD, ROCKET } from "./exhibits";

const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));
const extent = (pts: V3[], i: 0 | 1 | 2) =>
  Math.max(...pts.map((q) => q[i])) - Math.min(...pts.map((q) => q[i]));
const same = (x: readonly number[] | null, y: readonly number[]) =>
  x !== null && x.every((v, i) => Math.abs(v - (y[i] ?? NaN)) < 1e-6);

describe("exhibit hero models", () => {
  it("builds the hand from stone alone, with no light", () => {
    // Mutation caught: a glowing part slipped into the pure-mass hero, and
    // a hand with no parts at all (the loop would pass on nothing).
    const parts = partsOf("stone-hand");
    expect(parts.length).toBeGreaterThan(0);
    for (const p of parts) expect(GLOWING, p.method).not.toContain(p.flag);
  });

  it("chequers the rocket's hull: every band alternates red and white round it, and neighbouring bands swap", () => {
    // Mutation caught: a stripe instead of a chequer (every band the same).
    const panels = partsOf("moon-rocket").filter((p) => p.method === "panel");
    expect(panels).toHaveLength(ROCKET.sectors * ROCKET.bands);
    const tints = panels.map((p) => p.tint);
    const first = tints[0] ?? null;
    for (let b = 0; b < ROCKET.bands; b++)
      for (let i = 0; i < ROCKET.sectors; i++) {
        const t = tints[b * ROCKET.sectors + i] ?? null;
        expect(same(t, first ?? []), `${b},${i}`).toBe((b + i) % 2 === 0);
      }
  });

  it("stands the rocket's three fins on its plinth", () => {
    // Mutation caught: fins floating over the plinth or one fin missing.
    const fins = partsOf("moon-rocket").filter(
      (p) => p.method === "extrude" && extent(local(p), 2) > 0.5,
    );
    expect(fins).toHaveLength(3);
    for (const f of fins)
      expect(Math.min(...local(f).map((q) => q[2]))).toBeCloseTo(
        ROCKET.plinth,
        4,
      );
  });

  it("sets four portholes in a row near the top, centred on the front", () => {
    // Mutation caught: three portholes (a lopsided row), the row turned
    // off the front, or a porthole slipped out of the top band.
    const ports = partsOf("moon-rocket")
      .filter((p) => p.method === "extrude" && extent(local(p), 2) < 0.1)
      .map((p) => {
        const pts = local(p);
        // The middle of its bounds (a mean of the vertices leans with
        // the triangulation, which is not mirror symmetric).
        const mid = (i: 0 | 1 | 2) =>
          (Math.max(...pts.map((q) => q[i])) +
            Math.min(...pts.map((q) => q[i]))) /
          2;
        return [mid(0), mid(1), mid(2)] as const;
      })
      .sort((x, y) => x[0] - y[0]);
    expect(ports).toHaveLength(4);
    const band = (ROCKET.h1 - ROCKET.h0) / ROCKET.bands;
    for (const [, d, h] of ports) {
      expect(d).toBeGreaterThan(0);
      expect(h).toBeGreaterThan(ROCKET.h1 - band);
      expect(h).toBeLessThan(ROCKET.h1);
    }
    // Mirror pairs about the front: the outer two and the inner two.
    for (const [i, j] of [
      [0, 3],
      [1, 2],
    ] as const) {
      const [p, q] = [ports[i], ports[j]];
      if (p === undefined || q === undefined) throw new Error("no port");
      expect(p[0] + q[0]).toBeCloseTo(0, 6);
      expect(p[1]).toBeCloseTo(q[1], 6);
    }
  });

  it("gives the hammer the screen prop's head and lays its crack flat round it", () => {
    // Mutation caught: the old oversized head, or a crack standing up.
    const parts = partsOf("thunder-hammer");
    const head = parts.find((p) => p.method === "bevelBox");
    if (head === undefined) throw new Error("no head");
    const pts = local(head);
    expect(extent(pts, 0)).toBeCloseTo(HAMMER_HEAD.long, 4);
    expect(extent(pts, 1)).toBeCloseTo(HAMMER_HEAD.side, 4);
    expect(extent(pts, 2)).toBeCloseTo(HAMMER_HEAD.side, 4);
    const cracks = parts.filter(
      (p) =>
        p.method === "box" && Math.max(...local(p).map((q) => q[2])) < 0.006,
    );
    expect(cracks.length).toBeGreaterThanOrEqual(6);
    for (const c of cracks) expect(c.flag).toBe(FLAG.lit);
  });
});
