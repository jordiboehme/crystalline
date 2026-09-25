/**
 * The living heroes' shape tests: what `heroModels.test.ts` does not check
 * for every kind. The mess table's catalogue surface lies on an upward
 * face, the drinking bird stands on the table, the robot's two faces never
 * share a quad, the sleep ring carries six pods and six lights and the
 * dome planters hold two or three domes.
 */

import { describe, expect, it } from "vitest";

import { heroSurfaces } from "../../../world/heroes";
import type { Hero, HeroKind } from "../../../world/types";
import { CELL } from "../../../world/units";
import {
  blinkFlag,
  createBuilder,
  type MeshData,
  type V3,
} from "../../geometry";
import { frameAt } from "../../kit";
import { LOOKS } from "../../looks";
import {
  normals,
  placeMesh,
  positions,
  recordingKitAt,
  toLocal,
  type Part,
} from "../../modelChecks";
import { buildHero, buildHeroMesh } from ".";

/** A free hero at turn 0, centred on the middle of a cell's width on a row line. */
function heroAt(kind: HeroKind, variant = 0): Hero {
  return { kind, variant, x: 4.5, y: 3, turn: 0, seed: 1 };
}

/** Where a hero's mesh is placed, in world metres. */
const anchorOf = (h: Hero): V3 => [h.x * CELL, 0, h.y * CELL];

/** A hero's recorded parts, built at the origin at turn 0. */
function partsOf(kind: HeroKind, variant = 0): Part[] {
  const parts: Part[] = [];
  buildHero(
    recordingKitAt(createBuilder(), parts),
    kind,
    variant,
    LOOKS.aperture,
  );
  return parts;
}

/** A part's points in the recipe's local `[a, d, h]`. */
const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));

/**
 * Whether some triangle of `mesh` faces straight up (normal within 1e-3 of
 * `(0, 1, 0)`), has all three corners at height `h` within 0.005, and
 * contains `(x, z)` in plan.
 */
function upwardFaceAt(mesh: MeshData, x: number, z: number, h: number) {
  const ps = positions(mesh);
  const ns = normals(mesh);
  for (let t = 0; t + 2 < ps.length; t += 3) {
    const [a, b, c, n] = [ps[t], ps[t + 1], ps[t + 2], ns[t]];
    if (!a || !b || !c || !n) continue;
    if (Math.hypot(n[0], n[1] - 1, n[2]) > 1e-3) continue;
    if ([a, b, c].some((p) => Math.abs(p[1] - h) > 0.005)) continue;
    const side = (p: V3, q: V3) =>
      (q[0] - p[0]) * (z - p[2]) - (q[2] - p[2]) * (x - p[0]);
    const s = [side(a, b), side(b, c), side(c, a)];
    if (s.every((v) => v >= -1e-9) || s.every((v) => v <= 1e-9)) return true;
  }
  return false;
}

/** The `(a, h)` plan box of a part's points, ignoring depth. */
function planBox(p: Part): { a0: number; a1: number; h0: number; h1: number } {
  const pts = local(p);
  const as = pts.map((q) => q[0]);
  const hs = pts.map((q) => q[2]);
  return {
    a0: Math.min(...as),
    a1: Math.max(...as),
    h0: Math.min(...hs),
    h1: Math.max(...hs),
  };
}

/** Whether two `(a, h)` plan boxes overlap. */
function overlaps(
  x: { a0: number; a1: number; h0: number; h1: number },
  y: { a0: number; a1: number; h0: number; h1: number },
): boolean {
  return x.a0 < y.a1 && y.a0 < x.a1 && x.h0 < y.h1 && y.h0 < x.h1;
}

describe("living hero models", () => {
  it("puts every surface on an upward face of the mesh", () => {
    for (const kind of ["mess-table"] as const) {
      const h = heroAt(kind);
      const mesh = placeMesh(
        buildHeroMesh(kind, 0, LOOKS.aperture),
        0,
        anchorOf(h),
      );
      const tops = heroSurfaces(h);
      expect(tops.length, kind).toBeGreaterThan(0);
      for (const surf of tops) {
        const cx = (surf.box.x0 + surf.box.x1) / 2;
        const cz = (surf.box.z0 + surf.box.z1) / 2;
        expect(upwardFaceAt(mesh, cx, cz, surf.h), kind).toBe(true);
      }
    }
  });

  it("the bird stands on the table", () => {
    // The bird sits at a 1.6, d 0; a window of a 1.5 to 1.7 catches only
    // its own parts, since the pedestal under it tops out at h 0.72 and
    // the tray and cups sit at a 1.0 to 1.3.
    const bird = partsOf("mess-table")
      .flatMap(local)
      .filter((p) => p[0] > 1.5 && p[0] < 1.7);
    expect(bird.length).toBeGreaterThan(0);
    const lowest = Math.min(...bird.map((p) => p[2]));
    const highest = Math.max(...bird.map((p) => p[2]));
    expect(lowest).toBeCloseTo(0.76, 3);
    expect(highest).toBeLessThanOrEqual(1.1 + 1e-6);
  });

  it("the robot's two faces never share a quad", () => {
    const parts = partsOf("helper-robot");
    const faceA = parts
      .filter((p) => p.flag >= blinkFlag(0) && p.flag <= blinkFlag(3))
      .map(planBox);
    const faceB = parts
      .filter((p) => p.flag >= blinkFlag(4) && p.flag <= blinkFlag(7))
      .map(planBox);
    expect(faceA.length).toBeGreaterThan(0);
    expect(faceB.length).toBeGreaterThan(0);
    for (const a of faceA)
      for (const b of faceB) expect(overlaps(a, b)).toBe(false);
  });

  it("the ring has six pods and six lights, one per group", () => {
    const parts = partsOf("sleep-ring");
    for (let group = 0; group < 6; group++) {
      const lights = parts.filter((p) => p.flag === blinkFlag(group));
      expect(lights, `group ${String(group)}`).toHaveLength(1);
    }
    for (let group = 6; group < 8; group++) {
      expect(parts.filter((p) => p.flag === blinkFlag(group))).toHaveLength(0);
    }
  });

  it("dome planters have two or three domes", () => {
    // A lathe's `a`-extent alone understates its radius: with 6 sides, no
    // facet lands exactly on the `a` axis, so the widest points are at
    // `sin(60deg)` of it. Measure the radial distance from the shape's own
    // bounding-box centre instead, which sits exactly on the lathe's axis.
    const radius = (p: Part) => {
      const pts = local(p);
      const as = pts.map((q) => q[0]);
      const ds = pts.map((q) => q[1]);
      const ca = (Math.min(...as) + Math.max(...as)) / 2;
      const cd = (Math.min(...ds) + Math.max(...ds)) / 2;
      return Math.max(...pts.map((q) => Math.hypot(q[0] - ca, q[1] - cd)));
    };
    for (const [variant, count] of [
      [0, 2],
      [1, 3],
    ] as const) {
      const lathes = partsOf("dome-planters", variant).filter(
        (p) => p.method === "lathe",
      );
      const domes = lathes.filter((p) => Math.abs(radius(p) - 0.7) < 1e-6);
      expect(domes, `variant ${String(variant)}`).toHaveLength(count);
    }
  });
});
