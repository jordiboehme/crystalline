/**
 * The workshop heroes' shape tests: what `heroModels.test.ts` does not
 * check for every kind. The gun bench's and the tube bench's catalogue
 * tops lie on an upward face of their mesh; the core wall's light cells
 * cover all eight twinkle groups and never overlap; the big gun is
 * exactly the same part sizes on the rack and the bench, only placed
 * differently; and the tube bench's three tubes all reach the one hub
 * they meet at.
 */

import { describe, expect, it } from "vitest";

import { heroSurfaces } from "../../../world/heroes";
import type { Hero, HeroKind } from "../../../world/types";
import { CELL } from "../../../world/units";
import { FLAG, createBuilder, type MeshData, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { LOOKS } from "../../looks";
import {
  positions,
  placeMesh,
  reaches,
  recordingKitAt,
  shape,
  toLocal,
  type Part,
} from "../../modelChecks";
import { surfaces } from "../common";
import { buildHero, buildHeroMesh } from ".";
import { bigGun } from "./workshop";

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

  it("the core wall's light cells cover all eight groups", () => {
    const cells = partsOf("core-wall").filter(
      (p) => p.flag >= FLAG.blink && p.flag < FLAG.blink + 8,
    );
    const groups = new Set(cells.map((p) => p.flag - FLAG.blink));
    expect(groups).toEqual(new Set([0, 1, 2, 3, 4, 5, 6, 7]));
    // BAY_COUNT bays of BAY_ROWS rows each, one glowing strip per row.
    expect(cells).toHaveLength(6 * 10);
  });

  it("no cell overlaps another", () => {
    // Every row strip is a flat panel at its bay's own depth, so the
    // overlap that matters is in the wall's own face, (a, h).
    const f = frameAt([0, 0, 0], 0);
    const cells = partsOf("core-wall")
      .filter((p) => p.flag >= FLAG.blink && p.flag < FLAG.blink + 8)
      .map((p) => p.points.map((q) => toLocal(f, q)));
    const bounds = (pts: V3[]) =>
      ([0, 2] as const).map((k) => ({
        lo: Math.min(...pts.map((p) => p[k])),
        hi: Math.max(...pts.map((p) => p[k])),
      }));
    const overlaps = (p: V3[], q: V3[]) => {
      const [bp, bq] = [bounds(p), bounds(q)];
      return bp.every((b, k) => {
        const c = bq[k];
        if (!c) return false;
        return b.lo < c.hi - 1e-6 && c.lo < b.hi - 1e-6;
      });
    };
    for (let i = 0; i < cells.length; i++)
      for (let j = i + 1; j < cells.length; j++) {
        const [ci, cj] = [cells[i], cells[j]];
        if (!ci || !cj) continue;
        expect(overlaps(ci, cj), `${String(i)} vs ${String(j)}`).toBe(false);
      }
  });

  it("the big gun is the same on the rack and the bench", () => {
    const gunParts = (
      a0: number,
      a1: number,
      d0: number,
      d1: number,
      h0: number,
    ): Part[] => {
      const builder = createBuilder();
      const parts: Part[] = [];
      const kit = recordingKitAt(builder, parts)(frameAt([0, 0, 0], 0));
      bigGun(kit, surfaces(LOOKS.aperture), a0, a1, d0, d1, h0);
      return parts;
    };
    const rack = gunParts(-0.8, 0.85, 0.06, 0.28, 1.15);
    const bench = gunParts(-0.5, 0.9, 0.32, 0.54, 0.96);
    expect(rack.length).toBeGreaterThan(0);
    expect(rack.length).toBe(bench.length);
    const extent = (pts: readonly V3[]) =>
      ([0, 1, 2] as const).map(
        (k) =>
          Math.max(...pts.map((p) => p[k])) - Math.min(...pts.map((p) => p[k])),
      );
    rack.forEach((p, i) => {
      const q = bench[i];
      if (!q) throw new Error(`no matching bench part ${String(i)}`);
      expect(p.method, `part ${String(i)}`).toBe(q.method);
      expect(p.flag, `part ${String(i)}`).toBe(q.flag);
      const [ep, eq] = [extent(p.points), extent(q.points)];
      ep.forEach((v, k) => {
        expect(v, `part ${String(i)} axis ${String(k)}`).toBeCloseTo(
          eq[k] ?? NaN,
          5,
        );
      });
    });
  });

  it("the tube bench's three tubes meet at one hub", () => {
    const parts = partsOf("tube-bench");
    const hub = parts.find(
      (p) => p.method === "cylinder" && p.flag === FLAG.lit,
    );
    if (!hub) throw new Error("no hub part");
    const hubShape = shape(hub.points);
    const tubes = parts.filter(
      (p) => p.flag >= FLAG.blink && p.flag < FLAG.blink + 3,
    );
    expect(tubes).toHaveLength(3);
    for (const tube of tubes) {
      const tubeShape = shape(tube.points);
      expect(reaches(tubeShape, hubShape) || reaches(hubShape, tubeShape)).toBe(
        true,
      );
    }
  });
});
