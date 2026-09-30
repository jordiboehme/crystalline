/**
 * The exhibits' shape tests: the hand has no light and holds four fingers
 * and a thumb, the rocket's hull is a true chequer with three fins on its
 * plinth and four portholes centred on its front, and the hammer's head
 * is its true size with its crack flat on the floor and its runes cut
 * into its two long sides between the knotwork bands.
 */

import { describe, expect, it } from "vitest";

import { FLAG, type V3 } from "../../geometry";
import { DECAL_LIFT, frameAt } from "../../kit";
import { GLOWING, inked, partsOf, toLocal, type Part } from "../../modelChecks";
import { HAMMER_RUNES } from "../marks";
import { FLOOR_CRACK, HAMMER, HAMMER_HEAD, ROCKET, RUNE_INK } from "./exhibits";
import { runsOf } from "./pixels";

const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));
const extent = (pts: V3[], i: 0 | 1 | 2) =>
  Math.max(...pts.map((q) => q[i])) - Math.min(...pts.map((q) => q[i]));

/**
 * The head faces the runes are cut into: the two long sides (`+d` and
 * `-d`), one band each, as the original carries them.
 */
const RUNE_FACES = 2;

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
    // Mutation caught: a head larger than `HAMMER_HEAD`, or a crack rising
    // above `HAMMER.crack.h`.
    const parts = partsOf("thunder-hammer");
    const head = parts.find((p) => p.method === "bevelBox");
    if (head === undefined) throw new Error("no head");
    const pts = local(head);
    expect(extent(pts, 0)).toBeCloseTo(HAMMER_HEAD.long, 4);
    expect(extent(pts, 1)).toBeCloseTo(HAMMER_HEAD.side, 4);
    expect(extent(pts, 2)).toBeCloseTo(HAMMER_HEAD.side, 4);
    const cracks = parts.filter(
      (p) => p.tint !== null && p.tint.every((c, i) => c === FLOOR_CRACK[i]),
    );
    expect(cracks.length).toBeGreaterThanOrEqual(6);
    for (const c of cracks) {
      expect(c.flag).toBe(FLAG.lit);
      expect(Math.max(...local(c).map((q) => q[2]))).toBeLessThanOrEqual(
        HAMMER.crack.h + 1e-6,
      );
    }
  });

  it("cuts the runes into the hammer head's faces, inside its size (2.6f C13)", () => {
    // Mutation caught: the runes missing, on one face only when the
    // original carries them on both, or standing off the head.
    // `pixelPanel` lays one quad per run of equal characters (`runsOf`).
    const runs = runsOf(HAMMER_RUNES).filter((r) => r.ch !== ".").length;
    expect(runs).toBeGreaterThan(3);
    const ink = inked(partsOf("thunder-hammer"), RUNE_INK, "panel");
    expect(ink).toHaveLength(RUNE_FACES * runs);
    for (const p of ink)
      for (const q of local(p)) {
        expect(Math.abs(q[0])).toBeLessThanOrEqual(
          HAMMER_HEAD.long / 2 + 0.005,
        );
        expect(q[2]).toBeLessThanOrEqual(HAMMER_HEAD.side + 0.005);
      }
  });

  it("sets one rune band on each long side, one lift proud, between the knotwork bands (2.6f C17)", () => {
    // Mutation caught: both bands on the same side, a band floating off
    // its face or sunk into it, and a band run over a knotwork band.
    const ink = inked(partsOf("thunder-hammer"), RUNE_INK, "panel");
    expect(ink.length).toBeGreaterThan(0);
    const face = HAMMER_HEAD.side / 2 + DECAL_LIFT;
    const inner = Math.min(...HAMMER.bands.map(Math.abs)) - HAMMER.bandHalf;
    let front = 0;
    for (const p of ink) {
      const pts = local(p);
      if ((pts[0]?.[1] ?? 0) > 0) front++;
      for (const q of pts) {
        expect(Math.abs(q[1])).toBeCloseTo(face, 6);
        expect(Math.abs(q[0])).toBeLessThanOrEqual(inner + 1e-9);
        expect(q[2]).toBeGreaterThanOrEqual(0);
      }
    }
    expect(front * 2).toBe(ink.length);
  });
});
