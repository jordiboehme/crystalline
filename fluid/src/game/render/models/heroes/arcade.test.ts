/**
 * The arcade heroes' shape tests: what `heroModels.test.ts` does not check
 * for every kind. Three games with their own titles and colours, attract
 * screens whose title card and demo never share a quad, a recruitment
 * cabinet that is bigger than the arcade cabinet and glows along its
 * sides, nothing that reaches out into the use point in front, and no
 * two faces stacked closer than the decal spacing.
 */

import { describe, expect, it } from "vitest";

import { HERO_CATALOGUE } from "../../../world/heroes";
import type { HeroKind } from "../../../world/types";
import {
  FLAG,
  blinkFlag,
  createBuilder,
  type MeshData,
  type V3,
} from "../../geometry";
import { DECAL_LIFT, frameAt } from "../../kit";
import { LOOKS } from "../../looks";
import {
  cross,
  dot,
  positions,
  recordingKitAt,
  sub,
  toLocal,
  type Part,
} from "../../modelChecks";
import { buildHero, buildHeroMesh } from ".";
import { ARCADE_GAMES, RECRUIT_DEMO, RECRUIT_TITLE } from "./arcade";
import { heroHalf } from "./common";
import { textRows } from "./pixels";

const CABINETS = ["arcade-cabinet", "recruit-cabinet"] as const;

/** A hero's recorded parts, built at the origin at turn 0. */
function partsOf(kind: HeroKind, variant: number): Part[] {
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

/** A part's bounds in `(a, h)`: `[a0, a1, h0, h1]`. */
function boundsAH(p: Part): [number, number, number, number] {
  const ps = local(p);
  const as = ps.map((q) => q[0]);
  const hs = ps.map((q) => q[2]);
  return [Math.min(...as), Math.max(...as), Math.min(...hs), Math.max(...hs)];
}

/** Whether two `(a, h)` bounds share any area (touching edges do not count). */
function overlap(
  p: readonly [number, number, number, number],
  q: readonly [number, number, number, number],
): boolean {
  const e = 1e-9;
  return (
    p[0] < q[1] - e && q[0] < p[1] - e && p[2] < q[3] - e && q[2] < p[3] - e
  );
}

/** A mesh's triangles: corners, unit normal and the plane's offset along it. */
function triangles(m: MeshData): { pts: V3[]; n: V3; off: number }[] {
  const ps = positions(m);
  const out: { pts: V3[]; n: V3; off: number }[] = [];
  for (let i = 0; i + 2 < ps.length; i += 3) {
    const [a, b, c] = [ps[i], ps[i + 1], ps[i + 2]];
    if (!a || !b || !c) continue;
    const x = cross(sub(b, a), sub(c, a));
    const l = Math.hypot(...x);
    if (l < 1e-10) continue;
    const n: V3 = [x[0] / l, x[1] / l, x[2] / l];
    out.push({ pts: [a, b, c], n, off: dot(n, a) });
  }
  return out;
}

/**
 * Whether two triangles in parallel planes of normal `n` overlap when seen
 * along `n` (a separating-axis test on their projections; sharing only an
 * edge or a corner does not count).
 */
function overlapAlong(n: V3, p: readonly V3[], q: readonly V3[]): boolean {
  const t: V3 = Math.abs(n[0]) < 0.9 ? [1, 0, 0] : [0, 1, 0];
  const u = cross(n, t);
  const lu = Math.hypot(...u);
  const e1: V3 = [u[0] / lu, u[1] / lu, u[2] / lu];
  const e2 = cross(n, e1);
  const flat = (xs: readonly V3[]) => xs.map((x) => [dot(x, e1), dot(x, e2)]);
  const [a, b] = [flat(p), flat(q)];
  for (const poly of [a, b])
    for (let i = 0; i < 3; i++) {
      const [p0, p1] = [poly[i], poly[(i + 1) % 3]];
      if (!p0 || !p1) continue;
      const ax = [(p1[1] ?? 0) - (p0[1] ?? 0), (p0[0] ?? 0) - (p1[0] ?? 0)];
      const pr = (xs: number[][]) =>
        xs.map((x) => (x[0] ?? 0) * (ax[0] ?? 0) + (x[1] ?? 0) * (ax[1] ?? 0));
      const [pa, pb] = [pr(a), pr(b)];
      if (
        Math.max(...pa) <= Math.min(...pb) + 1e-6 ||
        Math.max(...pb) <= Math.min(...pa) + 1e-6
      )
        return false;
    }
  return true;
}

describe("arcade hero models", () => {
  it("three games with distinct titles and colours", () => {
    expect(ARCADE_GAMES).toHaveLength(3);
    expect(ARCADE_GAMES).toHaveLength(
      HERO_CATALOGUE["arcade-cabinet"].variants,
    );
    const titles = new Set<string>(ARCADE_GAMES.map((g) => g.title));
    expect(titles.size).toBe(3);
    expect(titles.has(RECRUIT_TITLE)).toBe(false);
    const colours = ARCADE_GAMES.flatMap((g) => [g.body, g.side, g.accent]);
    expect(new Set(colours.map((c) => c.join(","))).size).toBe(colours.length);
    for (const g of [...ARCADE_GAMES.map((x) => x.title), RECRUIT_TITLE])
      expect(() => textRows(g), g).not.toThrow();
    for (const g of [
      ...ARCADE_GAMES,
      { title: RECRUIT_TITLE, demo: RECRUIT_DEMO },
    ]) {
      expect(g.demo.length, g.title).toBeGreaterThan(0);
      const width = g.demo[0]?.length ?? 0;
      expect(width, g.title).toBeGreaterThan(0);
      for (const row of g.demo) {
        expect(row, g.title).toHaveLength(width);
        expect(row, g.title).toMatch(/^[.asw]+$/);
      }
    }
  });

  it("every cabinet variant has title pixels in groups 0-3 and demo pixels in groups 4-7, and no title quad overlaps a demo quad in (a, h)", () => {
    for (const kind of CABINETS)
      for (let v = 0; v < HERO_CATALOGUE[kind].variants; v++) {
        const parts = partsOf(kind, v);
        const inGroups = (g0: number) =>
          parts.filter(
            (p) =>
              p.flag >= blinkFlag(g0) &&
              p.flag <= blinkFlag(g0 + 3) &&
              p.method === "panel",
          );
        const title = inGroups(0);
        const demo = inGroups(4);
        for (let g = 0; g < 8; g++)
          expect(
            parts.some((p) => p.flag === blinkFlag(g)),
            `${kind} ${String(v)} group ${String(g)}`,
          ).toBe(true);
        expect(
          parts.filter((p) => p.flag >= FLAG.blink).length,
          `${kind} ${String(v)}: every blink part is a pixel`,
        ).toBe(title.length + demo.length);
        const tb = title.map(boundsAH);
        for (const d of demo.map(boundsAH))
          for (const t of tb)
            expect(overlap(t, d), `${kind} ${String(v)}`).toBe(false);
        // The title sits above the demo.
        const lowestTitle = Math.min(...tb.map((b) => b[2]));
        const highestDemo = Math.max(...demo.map(boundsAH).map((b) => b[3]));
        expect(lowestTitle).toBeGreaterThan(highestDemo);
      }
  });

  it("the recruitment cabinet is wider and deeper than the arcade cabinet and its side panels glow", () => {
    const extents = (kind: HeroKind) => {
      const ps = positions(buildHeroMesh(kind, 0, LOOKS.aperture));
      const span = (i: 0 | 2) =>
        Math.max(...ps.map((p) => p[i])) - Math.min(...ps.map((p) => p[i]));
      return { width: span(0), depth: span(2) };
    };
    const arcade = extents("arcade-cabinet");
    const recruit = extents("recruit-cabinet");
    expect(recruit.width).toBeGreaterThan(arcade.width + 0.1);
    expect(recruit.depth).toBeGreaterThan(arcade.depth + 0.1);
    const parts = partsOf("recruit-cabinet", 0);
    for (const side of [1, -1]) {
      const panels = parts.filter((p) => {
        if (p.flag !== FLAG.signal) return false;
        const ps = local(p);
        const out = ps.map((q) => side * q[0]);
        const hs = ps.map((q) => q[2]);
        return (
          Math.min(...out) >= 0.46 - 1e-6 &&
          Math.max(...hs) - Math.min(...hs) > 1.5
        );
      });
      expect(panels.length, `side ${String(side)}`).toBeGreaterThan(0);
    }
  });

  it("no part reaches the use point", () => {
    for (const kind of CABINETS)
      for (let v = 0; v < HERO_CATALOGUE[kind].variants; v++) {
        const { d1 } = heroHalf(kind, v);
        const use = HERO_CATALOGUE[kind].use;
        expect(use).not.toBeNull();
        expect(use?.d ?? 0).toBeGreaterThan(d1);
        const deepest = Math.max(
          ...partsOf(kind, v)
            .flatMap(local)
            .map((q) => q[1]),
        );
        expect(deepest, `${kind} ${String(v)}`).toBeLessThanOrEqual(d1 + 1e-9);
      }
  });

  it("stacks no two same-facing overlapping faces closer than the decal spacing", () => {
    for (const kind of CABINETS)
      for (let v = 0; v < HERO_CATALOGUE[kind].variants; v++) {
        const ts = triangles(buildHeroMesh(kind, v, LOOKS.aperture));
        const close: string[] = [];
        for (let i = 0; i < ts.length; i++)
          for (let j = i + 1; j < ts.length; j++) {
            const [p, q] = [ts[i], ts[j]];
            if (!p || !q || dot(p.n, q.n) < 1 - 1e-6) continue;
            const gap = Math.abs(p.off - q.off);
            if (gap >= DECAL_LIFT - 1e-6) continue;
            if (overlapAlong(p.n, p.pts, q.pts))
              close.push(`${gap.toFixed(4)} at ${p.pts[0]?.join(",") ?? ""}`);
          }
        expect(close, `${kind} ${String(v)}`).toEqual([]);
      }
  });
});
