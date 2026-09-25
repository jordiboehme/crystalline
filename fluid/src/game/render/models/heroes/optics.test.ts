/**
 * The optics heroes' shape tests: what `heroModels.test.ts` does not check
 * for every kind. The laser desk's catalogue top lies on an upward face of
 * its mesh and stays clear, the slab keeps its 1 : 4 : 9, the turret's eye
 * looks out of its front and a seam splits its shell, the eye panel is a
 * portrait plate with a small dot at the middle of its lens, the photo
 * console's picture leans back, and the laser's lens hangs over the
 * chair's seat.
 */

import { describe, expect, it } from "vitest";

import { heroSurfaces } from "../../../world/heroes";
import type { Hero, HeroKind } from "../../../world/types";
import { CELL } from "../../../world/units";
import {
  blinkFlag,
  createBuilder,
  FLAG,
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
import { OFFICE_CHAIR } from "./optics";

/** A free hero at turn 0, centred on the middle of a cell's width on a row line. */
function heroAt(kind: HeroKind): Hero {
  return { kind, variant: 0, x: 4.5, y: 3, turn: 0, seed: 1 };
}

/** Where a hero's mesh is placed, in world metres. */
const anchorOf = (h: Hero): V3 => [h.x * CELL, 0, h.y * CELL];

/** A hero's recorded parts, built at the origin at turn 0. */
function partsOf(kind: HeroKind): Part[] {
  const parts: Part[] = [];
  buildHero(recordingKitAt(createBuilder(), parts), kind, 0, LOOKS.aperture);
  return parts;
}

/** A part's points in the recipe's local `[a, d, h]`. */
const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));

/** The one part of a list, failing the test when there is not exactly one. */
function one(parts: readonly Part[]): Part {
  expect(parts).toHaveLength(1);
  const p = parts[0];
  if (!p) throw new Error("no part");
  return p;
}

/** The bounds of a list of local points. */
function bounds(points: readonly V3[]): { lo: V3; hi: V3 } {
  const lo = (k: 0 | 1 | 2) => Math.min(...points.map((p) => p[k]));
  const hi = (k: 0 | 1 | 2) => Math.max(...points.map((p) => p[k]));
  return { lo: [lo(0), lo(1), lo(2)], hi: [hi(0), hi(1), hi(2)] };
}

/** The middle of a list of local points' bounds. */
function centre(points: readonly V3[]): V3 {
  const mid = (k: 0 | 1 | 2) =>
    (Math.min(...points.map((p) => p[k])) +
      Math.max(...points.map((p) => p[k]))) /
    2;
  return [mid(0), mid(1), mid(2)];
}

/**
 * Whether some triangle of `mesh` faces straight up (normal within 1e-3
 * of `(0, 1, 0)`), has all three corners at height `h` within 0.005, and
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

describe("optics hero models", () => {
  it("puts every surface on an upward face of the mesh", () => {
    for (const kind of ["laser-desk"] as const) {
      const h = heroAt(kind);
      const mesh = placeMesh(
        buildHeroMesh(kind, 0, LOOKS.aperture),
        0,
        anchorOf(h),
      );
      const tops = heroSurfaces(h);
      expect(tops.length, kind).toBeGreaterThan(0);
      for (const s of tops) {
        const cx = (s.box.x0 + s.box.x1) / 2;
        const cz = (s.box.z0 + s.box.z1) / 2;
        expect(upwardFaceAt(mesh, cx, cz, s.h), kind).toBe(true);
      }
    }
  });

  it("keeps the laser desk's catalogue top clear in the recipe's own terms", () => {
    // The catalogue's top: a -1.1 to -0.35, d -0.2 to 0.4, at h 0.74. The
    // arm passes high over it (from h 1.92); nothing may stand on it below
    // that, neither a vertex nor a part whose bounds span the area (a long
    // box whose corners all lie outside it).
    const [a0, a1, d0, d1, h0, h1] = [-1.1, -0.35, -0.2, 0.4, 0.74, 1.9];
    const parts = partsOf("laser-desk");
    const inside = (p: V3) =>
      p[0] > a0 &&
      p[0] < a1 &&
      p[1] > d0 &&
      p[1] < d1 &&
      p[2] > h0 + 1e-6 &&
      p[2] < h1;
    expect(parts.flatMap(local).filter(inside)).toEqual([]);
    const over = parts
      .map((p, i) => ({ i, method: p.method, b: bounds(local(p)) }))
      .filter(
        ({ b }) =>
          b.lo[0] < a1 &&
          b.hi[0] > a0 &&
          b.lo[1] < d1 &&
          b.hi[1] > d0 &&
          b.lo[2] < h1 &&
          b.hi[2] > h0 + 1e-6,
      )
      .map(({ i, method }) => `${String(i)}:${method}`);
    expect(over).toEqual([]);
  });

  it("the slab is 1 : 4 : 9", () => {
    const ps = positions(
      placeMesh(buildHeroMesh("black-slab", 0, LOOKS.aperture), 0, [0, 0, 0]),
    );
    const extent = (k: 0 | 1 | 2) =>
      Math.max(...ps.map((p) => p[k])) - Math.min(...ps.map((p) => p[k]));
    expect(extent(0)).toBeCloseTo(1.2, 6);
    expect(extent(2)).toBeCloseTo(0.3, 6);
    expect(extent(1)).toBeCloseTo(2.7, 6);
  });

  it("the turret's eye faces front", () => {
    const eye = one(partsOf("turret").filter((p) => p.flag === blinkFlag(0)));
    const [, d, h] = centre(local(eye));
    expect(d).toBeGreaterThan(0.2);
    expect(h).toBeGreaterThan(0.95);
    expect(h).toBeLessThan(1.1);
  });

  it("the laser points at the chair", () => {
    const parts = partsOf("laser-desk");
    const lens = one(
      parts.filter((p) => p.method === "cylinder" && p.flag === blinkFlag(0)),
    );
    const [s0, s1] = OFFICE_CHAIR.seat;
    const seat = one(
      parts.filter((p) => {
        if (p.method !== "bevelBox") return false;
        const hs = local(p).map((q) => q[2]);
        return (
          Math.abs(Math.min(...hs) - s0) < 1e-6 &&
          Math.abs(Math.max(...hs) - s1) < 1e-6
        );
      }),
    );
    const [la, ld, lh] = centre(local(lens));
    const [sa, sd] = centre(local(seat));
    expect(Math.hypot(la - sa, ld - sd)).toBeLessThan(0.25);
    expect(lh).toBeGreaterThan(s1);
  });

  it("splits the turret's shell with a seam down its front", () => {
    const seams = partsOf("turret").filter((p) => {
      const ps = local(p);
      const b = bounds(ps);
      return (
        p.flag === FLAG.lit &&
        Math.max(Math.abs(b.lo[0]), Math.abs(b.hi[0])) < 0.006 &&
        b.hi[1] > 0.27 &&
        b.lo[2] < 0.45
      );
    });
    expect(seams.length).toBeGreaterThan(0);
  });

  it("the eye panel is a portrait plate with a small dot in the middle of its lens", () => {
    const parts = partsOf("eye-panel");
    const plate = one(parts.filter((p) => p.method === "bevelBox"));
    const b = bounds(local(plate));
    const ratio = (b.hi[2] - b.lo[2]) / (b.hi[0] - b.lo[0]);
    expect(ratio).toBeGreaterThan(2.8);
    expect(ratio).toBeLessThan(3.2);
    const eye = parts.filter((p) => p.flag === blinkFlag(0)).map(local);
    const mids = eye.map(centre);
    for (const m of mids) {
      expect(m[0]).toBeCloseTo(mids[0]?.[0] ?? NaN, 6);
      expect(m[2]).toBeCloseTo(mids[0]?.[2] ?? NaN, 6);
    }
    const front = eye.reduce((f, p) =>
      bounds(p).hi[1] > bounds(f).hi[1] ? p : f,
    );
    const fb = bounds(front);
    expect(fb.hi[0] - fb.lo[0]).toBeLessThan(0.05);
  });

  it("the photo console's picture leans back", () => {
    const glows = partsOf("photo-console")
      .filter((p) => p.flag === FLAG.emissive)
      .map(local);
    const picture = glows.reduce((f, p) => {
      const area = (q: V3[]) => {
        const b = bounds(q);
        return (b.hi[0] - b.lo[0]) * (b.hi[2] - b.lo[2]);
      };
      return area(p) > area(f) ? p : f;
    });
    const b = bounds(picture);
    const dAt = (h: number) =>
      Math.max(
        ...picture.filter((q) => Math.abs(q[2] - h) < 0.01).map((q) => q[1]),
      );
    expect(dAt(b.lo[2]) - dAt(b.hi[2])).toBeGreaterThan(0.05);
  });
});
