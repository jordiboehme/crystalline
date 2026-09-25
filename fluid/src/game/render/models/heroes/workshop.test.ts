/**
 * The workshop heroes' shape tests: what `heroModels.test.ts` does not
 * check for every kind. The gun bench's and the tube bench's catalogue
 * tops lie on an upward face of their mesh; the core wall's lamps cover
 * all eight twinkle groups and never overlap; the big gun, taken from the
 * built rack and bench, is the same parts only moved; the tube bench's
 * three tubes meet at one round hub, two arms up and the stem down; the
 * field pack's chase climbs its cell one light per group and runs round
 * its cyclotron in ring order; and no part of any workshop hero floats:
 * each stands on the floor, on its wall or on another part.
 */

import { describe, expect, it } from "vitest";

import { HERO_FOOTING } from "../../../world/footprints";
import { HERO_CATALOGUE, heroSurfaces } from "../../../world/heroes";
import type { Hero, HeroKind } from "../../../world/types";
import { CELL } from "../../../world/units";
import { FLAG, createBuilder, type MeshData, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { LOOKS } from "../../looks";
import {
  closestOnTriangle,
  positions,
  placeMesh,
  reaches,
  recordingKitAt,
  shape,
  sub,
  toLocal,
  type Part,
  type Shape,
} from "../../modelChecks";
import { buildHero, buildHeroMesh } from ".";

/** A free hero at turn 0, centred on the middle of a cell's width on a row line. */
function heroAt(kind: HeroKind): Hero {
  return { kind, variant: 0, x: 4.5, y: 3, turn: 0, seed: 1 };
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

/** Whether a part is a light in one of the eight blink groups. */
const blinks = (p: Part) => p.flag >= FLAG.blink && p.flag < FLAG.blink + 8;

/** How far two points lie apart. */
const gap = (p: V3, q: V3) => Math.hypot(...sub(p, q));

/** The offset that moves part `p` onto part `q` (their first points). */
const offsetOf = (p: Part, q: Part): V3 =>
  sub(q.points[0] ?? [0, 0, 0], p.points[0] ?? [0, 0, 0]);

/**
 * Whether part `q` is part `p` moved by one vector: the same primitive,
 * the same flag and every point shifted by the same offset (to 1e-6).
 */
function sameMoved(p: Part, q: Part): boolean {
  if (p.method !== q.method || p.flag !== q.flag) return false;
  if (p.points.length === 0 || p.points.length !== q.points.length)
    return false;
  const shift = offsetOf(p, q);
  return p.points.every((a, i) => {
    const b = q.points[i];
    return b !== undefined && gap(sub(b, a), shift) < 1e-6;
  });
}

/**
 * Whether some triangle of `mesh` faces straight up (normal within 1e-3
 * of `(0, 1, 0)`), has all three corners at height `h` within 0.005, and
 * contains `(x, z)` in plan.
 */
function upwardFaceAt(mesh: MeshData, x: number, z: number, h: number) {
  const ps = positions(mesh);
  for (let t = 0; t + 2 < ps.length; t += 3) {
    const [a, b, c] = [ps[t], ps[t + 1], ps[t + 2]];
    if (!a || !b || !c) continue;
    const ab: V3 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    const ac: V3 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    const n: V3 = [
      ab[1] * ac[2] - ab[2] * ac[1],
      ab[2] * ac[0] - ab[0] * ac[2],
      ab[0] * ac[1] - ab[1] * ac[0],
    ];
    const len = Math.hypot(n[0], n[1], n[2]);
    if (len < 1e-9) continue;
    const nn: V3 = [n[0] / len, n[1] / len, n[2] / len];
    if (Math.hypot(nn[0], nn[1] - 1, nn[2]) > 1e-3) continue;
    if ([a, b, c].some((p) => Math.abs(p[1] - h) > 0.005)) continue;
    const side = (p: V3, q: V3) =>
      (q[0] - p[0]) * (z - p[2]) - (q[2] - p[2]) * (x - p[0]);
    const s = [side(a, b), side(b, c), side(c, a)];
    if (s.every((v) => v >= -1e-9) || s.every((v) => v <= 1e-9)) return true;
  }
  return false;
}

/**
 * How close two parts must come to count as touching in the float test:
 * a little over `DECAL_LIFT`, so a light or decal on its face counts, and
 * well under `reaches`' 3 cm, so a 1.5 cm gap under a shelf or a pack
 * does not.
 */
const TOUCH = 0.012;

/**
 * Whether two parts touch: their bounds overlap with some volume (a sleeve
 * round a barrel, a part sunk into another), or a vertex of one lies
 * within `TOUCH` of a triangle of the other.
 */
function touching(p: Shape, q: Shape): boolean {
  const axes = [0, 1, 2] as const;
  if (axes.some((k) => p.lo[k] > q.hi[k] + TOUCH || q.lo[k] > p.hi[k] + TOUCH))
    return false;
  if (axes.every((k) => p.lo[k] < q.hi[k] - 1e-6 && q.lo[k] < p.hi[k] - 1e-6))
    return true;
  const near = (from: Shape, to: Shape) => {
    for (let t = 0; t + 2 < to.points.length; t += 3) {
      const [a, b, c] = [to.points[t], to.points[t + 1], to.points[t + 2]];
      if (!a || !b || !c) continue;
      for (const v of from.points)
        if (gap(v, closestOnTriangle(v, a, b, c)) <= TOUCH) return true;
    }
    return false;
  };
  return near(p, q) || near(q, p);
}

/** The workshop kinds, each tested in every variant. */
const WORKSHOP_KINDS = [
  "core-wall",
  "gun-rack",
  "gun-bench",
  "tube-bench",
  "field-pack",
] as const satisfies readonly HeroKind[];

describe("workshop hero models", () => {
  it("puts every surface on an upward face of the mesh", () => {
    for (const kind of ["gun-bench", "tube-bench"] as const) {
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

  it("the core wall's lamps cover all eight groups, many small ones to a cabinet", () => {
    const lamps = partsOf("core-wall").filter(blinks);
    const groups = new Set(lamps.map((p) => p.flag - FLAG.blink));
    expect(groups).toEqual(new Set([0, 1, 2, 3, 4, 5, 6, 7]));
    // Six cabinets, each a grid of six columns by thirteen rows.
    expect(lamps).toHaveLength(6 * 6 * 13);
    for (const lamp of lamps) {
      const b = shape(lamp.points);
      expect(b.hi[0] - b.lo[0]).toBeLessThan(0.06);
      expect(b.hi[1] - b.lo[1]).toBeLessThan(0.06);
    }
  });

  it("no lamp overlaps another", () => {
    // Every lamp is a flat quad on its cabinet's plate, so the overlap that
    // matters is in the wall's own face, (a, h).
    const f = frameAt([0, 0, 0], 0);
    const boxes = partsOf("core-wall")
      .filter(blinks)
      .map((p) => {
        const pts = p.points.map((q) => toLocal(f, q));
        return ([0, 2] as const).map((k) => ({
          lo: Math.min(...pts.map((q) => q[k])),
          hi: Math.max(...pts.map((q) => q[k])),
        }));
      });
    const overlaps = (
      p: { lo: number; hi: number }[],
      q: { lo: number; hi: number }[],
    ) =>
      p.every((b, k) => {
        const c = q[k];
        return c !== undefined && b.lo < c.hi - 1e-6 && c.lo < b.hi - 1e-6;
      });
    for (let i = 0; i < boxes.length; i++)
      for (let j = i + 1; j < boxes.length; j++) {
        const [bi, bj] = [boxes[i], boxes[j]];
        if (!bi || !bj) continue;
        expect(overlaps(bi, bj), `${String(i)} vs ${String(j)}`).toBe(false);
      }
  });

  it("the big gun is the same on the rack and the bench", () => {
    // The longest run of parts, in build order, that the bench repeats from
    // the rack, each moved by one and the same offset: that run is the gun.
    const rack = partsOf("gun-rack");
    const bench = partsOf("gun-bench");
    let best = { i: 0, j: 0, n: 0 };
    for (let i = 0; i < rack.length; i++)
      for (let j = 0; j < bench.length; j++) {
        const [r0, b0] = [rack[i], bench[j]];
        if (!r0 || !b0 || !sameMoved(r0, b0)) continue;
        const shift = offsetOf(r0, b0);
        let n = 0;
        for (;;) {
          const [r, b] = [rack[i + n], bench[j + n]];
          if (!r || !b || !sameMoved(r, b)) break;
          if (gap(offsetOf(r, b), shift) > 1e-6) break;
          n++;
        }
        if (n > best.n) best = { i, j, n };
      }
    // Stock, body, spine, bezel, window, barrel, three collars, four vent
    // slots, the muzzle and the grip.
    expect(best.n).toBe(15);
    // Every light of either model is the gun's core, all inside the run.
    const lights = rack.filter(blinks);
    expect(lights.length).toBeGreaterThan(0);
    expect(rack.slice(best.i, best.i + best.n).filter(blinks)).toEqual(lights);
    expect(bench.slice(best.j, best.j + best.n).filter(blinks)).toEqual(
      bench.filter(blinks),
    );
  });

  it("the tube bench's three tubes meet at one round hub, two arms up and the stem down", () => {
    const parts = partsOf("tube-bench");
    const tubes = parts
      .filter(
        (p) =>
          p.method === "extrude" &&
          p.flag >= FLAG.blink &&
          p.flag < FLAG.blink + 3,
      )
      .map((p) => shape(p.points));
    expect(tubes).toHaveLength(3);
    const hub = parts.find((p) => {
      if (p.method !== "extrude" || p.flag !== FLAG.lit) return false;
      const h = shape(p.points);
      return tubes.every((t) => reaches(t, h) || reaches(h, t));
    });
    if (!hub) throw new Error("no hub that all three tubes reach");
    const hb = shape(hub.points);
    // Round in the wall's plane: as wide along the wall as it is tall.
    expect(hb.hi[0] - hb.lo[0]).toBeCloseTo(hb.hi[1] - hb.lo[1], 2);
    const hubH = (hb.lo[1] + hb.hi[1]) / 2;
    expect(tubes.filter((t) => t.hi[1] > hubH + 0.05)).toHaveLength(2);
    expect(tubes.filter((t) => t.lo[1] < hubH - 0.05)).toHaveLength(1);
  });

  it("runs the field pack's chase up the cell and round the cyclotron", () => {
    const centre = (p: Part) => {
      const b = shape(p.points);
      return [(b.lo[0] + b.hi[0]) / 2, (b.lo[1] + b.hi[1]) / 2] as const;
    };
    const group = (p: Part) => p.flag - FLAG.blink;
    for (let v = 0; v < HERO_CATALOGUE["field-pack"].variants; v++) {
      const lights = partsOf("field-pack", v).filter(blinks);
      // The cell: one light per group, 0 at the bottom to 7 at the top.
      const cell = lights
        .filter((p) => p.method === "panel")
        .sort((p, q) => centre(p)[1] - centre(q)[1]);
      expect(cell.map(group), `v${String(v)}`).toEqual([
        0, 1, 2, 3, 4, 5, 6, 7,
      ]);
      // The cyclotron: four lenses on groups 0, 2, 4 and 6, each the next
      // quarter round the ring in the same direction.
      const ring = lights
        .filter((p) => p.method === "extrude")
        .sort((p, q) => group(p) - group(q));
      expect(ring.map(group), `v${String(v)}`).toEqual([0, 2, 4, 6]);
      const cs = ring.map(centre);
      const mid = [0, 1].map(
        (k) => cs.reduce((n, c) => n + (c[k] ?? 0), 0) / cs.length,
      );
      const angles = cs.map((c) =>
        Math.atan2(c[1] - (mid[1] ?? 0), c[0] - (mid[0] ?? 0)),
      );
      const steps = angles.map((a, i) => {
        const next = angles[(i + 1) % angles.length] ?? a;
        const d = (((next - a) % (2 * Math.PI)) + 3 * Math.PI) % (2 * Math.PI);
        return d - Math.PI;
      });
      const first = steps[0] ?? 0;
      expect(Math.abs(Math.abs(first) - Math.PI / 2)).toBeLessThan(1e-6);
      for (const step of steps) expect(step).toBeCloseTo(first, 6);
    }
  });

  it("stands every part on the floor, its wall or another part", () => {
    const f = frameAt([0, 0, 0], 0);
    const loose: string[] = [];
    for (const kind of WORKSHOP_KINDS)
      for (let v = 0; v < HERO_CATALOGUE[kind].variants; v++) {
        const parts = partsOf(kind, v).filter((p) => p.points.length > 0);
        const shapes: Shape[] = parts.map((p) => shape(p.points));
        const onWall = HERO_FOOTING[kind] !== "free";
        const held = shapes.map(
          (s) =>
            s.lo[1] <= 1e-4 ||
            (onWall && s.points.some((q) => toLocal(f, q)[1] <= 1e-4)),
        );
        for (let changed = true; changed;) {
          changed = false;
          shapes.forEach((s, i) => {
            if (held[i]) return;
            if (shapes.some((o, j) => held[j] && touching(s, o))) {
              held[i] = true;
              changed = true;
            }
          });
        }
        parts.forEach((p, i) => {
          if (!held[i])
            loose.push(`${kind} ${String(v)} ${String(i)}:${p.method}`);
        });
      }
    expect(loose).toEqual([]);
  });
});
