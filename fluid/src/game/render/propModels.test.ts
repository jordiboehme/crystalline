import { describe, expect, it } from "vitest";

import { propFootprint } from "../world/footprints";
import {
  PROP_CATALOGUE,
  PROP_KINDS,
  RARE_PROP_KINDS,
  SPAN_HALF,
  TALL_MIN,
  WALL_PROP_DEPTH,
  WIDE_REACH,
} from "../world/props";
import { turnForSide, wallAnchor } from "../world/sites";
import type { PropKind, Side } from "../world/types";
import { CELL } from "../world/units";
import {
  ACCENT_MARK,
  FLAG,
  FLOATS_PER_VERTEX,
  PROP_MARK,
  accentTint,
  createBuilder,
  type MeshData,
  type V3,
} from "./geometry";
import { frameAt, frameForSlot, type Frame } from "./kit";
import { LOOKS, propLook, type Look } from "./looks";
import {
  GLOWING,
  floatingGlow,
  inBox,
  looseParts,
  placeMesh,
  placeParts,
  positions,
  reaches,
  recordingKitAt,
  shape,
  toLocal,
  worstWinding,
  type Part,
} from "./modelChecks";
import { FLUSH_DEPTH, HEADROOM } from "./models";
import { WALL_STANDING } from "./models/props/wall";
import {
  CEILING_DROP,
  CEILING_OUT,
  CEILING_SETBACK,
  FLOOR_TOP,
  PROP_BANK,
  RUN_BAND,
  RUN_REACH,
  SPAN_REACH,
  WALL_REACH,
  WALL_TOP,
} from "./models/props/common";
import { buildProp, buildPropMesh } from "./models/props";

const EPS = 1e-4;
/** The lowest ceiling the generator makes, where the ceiling anchor sits. */
const CEILING = 3.0;
/** The floor anchor, in cell units: the middle of a cell's width, on a row line. */
const FLOOR_AT = { x: 4.5, y: 3 } as const;
const SIDES: readonly Side[] = ["n", "e", "s", "w"];

/** The wall edge whose side takes turn `t`. */
function sideFor(t: number): Side {
  const side = SIDES.find((s) => turnForSide(s) === t);
  if (!side) throw new Error(`no side for turn ${String(t)}`);
  return side;
}

/**
 * A prop built once at the origin, with every kit call recorded, in look 2
 * (whose parts drawn only for the prop's own accent the envelope checks
 * then cover) unless `look` says otherwise.
 */
function buildRecorded(
  kind: PropKind,
  variant: number,
  look: Look = LOOKS.aperture,
): { mesh: MeshData; parts: Part[] } {
  const builder = createBuilder();
  const parts: Part[] = [];
  buildProp(recordingKitAt(builder, parts), kind, variant, { look });
  return { mesh: builder.build(), parts };
}

/**
 * The surface of a mesh's triangles carrying the prop's own mark, and of
 * all its triangles, in square metres.
 */
function markedArea(m: MeshData): { part: number; whole: number } {
  const at = positions(m);
  let part = 0;
  let whole = 0;
  for (let i = 0; i + 2 < m.count; i += 3) {
    const [a, b, c] = [at[i], at[i + 1], at[i + 2]];
    if (a === undefined || b === undefined || c === undefined) continue;
    const u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    const w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    const area =
      Math.hypot(
        (u[1] ?? 0) * (w[2] ?? 0) - (u[2] ?? 0) * (w[1] ?? 0),
        (u[2] ?? 0) * (w[0] ?? 0) - (u[0] ?? 0) * (w[2] ?? 0),
        (u[0] ?? 0) * (w[1] ?? 0) - (u[1] ?? 0) * (w[0] ?? 0),
      ) / 2;
    whole += area;
    if (m.vertices[i * FLOATS_PER_VERTEX + 9] === PROP_MARK) part += area;
  }
  return { part, whole };
}

/**
 * How many groups the parts fall into, two parts in one group when a
 * corner of one lies within a millimetre of a corner of the other,
 * directly or through others: the strips of a rim or a band share their
 * corners, two separate stripes or bands share none.
 */
function touchingGroups(parts: readonly (readonly V3[])[]): number {
  const parent = parts.map((_, i) => i);
  const root = (i: number): number => {
    while (parent[i] !== i) i = parent[i] ?? i;
    return i;
  };
  const touch = (a: readonly V3[], b: readonly V3[]) =>
    a.some((p) =>
      b.some((q) => Math.hypot(p[0] - q[0], p[1] - q[1], p[2] - q[2]) < 1e-3),
    );
  parts.forEach((a, i) =>
    parts.forEach((b, j) => {
      if (j > i && touch(a, b)) parent[root(j)] = root(i);
    }),
  );
  return new Set(parts.map((_, i) => root(i))).size;
}

/** Where an instance goes: its world offset, in metres. */
function anchorFor(kind: PropKind, t: number): V3 {
  const entry = PROP_CATALOGUE[kind];
  if (entry.span) return [FLOOR_AT.x * CELL, CEILING, FLOOR_AT.y * CELL];
  if (entry.anchor === "floor")
    return [FLOOR_AT.x * CELL, 0, FLOOR_AT.y * CELL];
  const a = wallAnchor({ x: 3, y: 4, side: sideFor(t) });
  return [a.x * CELL, entry.anchor === "ceiling" ? CEILING : 0, a.y * CELL];
}

describe("prop models", () => {
  for (const kind of PROP_KINDS) {
    const entry = PROP_CATALOGUE[kind];
    for (let v = 0; v < entry.variants; v++) {
      const { mesh, parts } = buildRecorded(kind, v);

      it(`${kind} variant ${String(v)} builds the same floats twice`, () => {
        const again = buildPropMesh(kind, v, LOOKS.aperture);
        expect(again.count).toBe(mesh.count);
        expect(Array.from(again.vertices)).toEqual(Array.from(mesh.vertices));
      });

      for (let t = 0; t < 4; t++) {
        describe(`${kind} variant ${String(v)} turned ${String(t)}`, () => {
          const at = anchorFor(kind, t);
          const placed = placeMesh(mesh, t, at);
          const wall: Frame | null =
            entry.anchor === "floor"
              ? null
              : frameForSlot({ x: 3, y: 4, side: sideFor(t) });

          it("stays inside its anchor's envelope", () => {
            const points = positions(placed);
            expect(points.length).toBeGreaterThan(0);
            if (entry.anchor === "floor") {
              const box = propFootprint({
                kind,
                variant: v,
                anchor: "floor",
                x: FLOOR_AT.x,
                y: FLOOR_AT.y,
                turn: t,
                seed: 0,
              });
              if (!box) throw new Error("no footprint");
              for (const p of points) {
                expect(inBox(box, p, EPS)).toBe(true);
                expect(p[1]).toBeGreaterThanOrEqual(-EPS);
                expect(p[1]).toBeLessThanOrEqual(FLOOR_TOP + EPS);
              }
              return;
            }
            if (entry.span) {
              const f = frameAt([FLOOR_AT.x * CELL, 0, FLOOR_AT.y * CELL], t);
              for (const p of points) {
                const [a, d, h] = toLocal(f, p);
                expect(Math.abs(a)).toBeLessThanOrEqual(SPAN_REACH + EPS);
                expect(Math.abs(d)).toBeLessThanOrEqual(SPAN_HALF + EPS);
                expect(h).toBeGreaterThanOrEqual(CEILING - CEILING_DROP - EPS);
                expect(h).toBeLessThanOrEqual(CEILING - HEADROOM + EPS);
              }
              return;
            }
            if (!wall) throw new Error("no wall frame");
            const reach = entry.run ? RUN_REACH : WALL_REACH;
            for (const p of points) {
              const [a, d, h] = toLocal(wall, p);
              expect(Math.abs(a)).toBeLessThanOrEqual(reach + EPS);
              if (entry.anchor === "wall") {
                expect(d).toBeGreaterThanOrEqual(-EPS);
                expect(d).toBeLessThanOrEqual(FLUSH_DEPTH + EPS);
                const [h0, h1] = entry.run
                  ? [RUN_BAND.h0, RUN_BAND.h1]
                  : [0, WALL_TOP];
                expect(h).toBeGreaterThanOrEqual(h0 - EPS);
                expect(h).toBeLessThanOrEqual(h1 + EPS);
              } else {
                expect(d).toBeGreaterThanOrEqual(CEILING_SETBACK - EPS);
                expect(d).toBeLessThanOrEqual(CEILING_OUT + EPS);
                expect(h).toBeGreaterThanOrEqual(CEILING - CEILING_DROP - EPS);
                expect(h).toBeLessThanOrEqual(CEILING - HEADROOM + EPS);
              }
            }
          });

          it("winds every triangle with its normal", () => {
            expect(placed.count).toBeGreaterThan(0);
            expect(placed.count % 3).toBe(0);
            expect(worstWinding(placed)).toBeGreaterThan(0.999);
          });

          it("stays under the triangle budget", () => {
            expect(placed.count / 3).toBeLessThan(4000);
          });

          it("glows only on or in its body", () => {
            const glowWall = entry.anchor === "wall" ? wall : null;
            expect(floatingGlow(placeParts(parts, t, at), glowWall)).toEqual(
              [],
            );
          });
        });
      }
    }
  }

  it("refuses a variant the catalogue does not have", () => {
    expect(() => buildPropMesh("crate", 3, LOOKS.aperture)).toThrow(
      /no variant 3/,
    );
    expect(() => buildPropMesh("duct", -1, LOOKS.aperture)).toThrow(
      /no variant -1/,
    );
  });

  it("gives the world the wall band's depth", () => {
    expect(WALL_PROP_DEPTH).toBe(FLUSH_DEPTH);
  });

  it("keeps every floor prop under the lowest ceiling prop of the lowest ceiling", () => {
    // generate.ts: ceiling = 3 + salience * 0.2, so never under 3.0 m. Spans
    // now hang over mid-hall clusters, so this gap is what keeps them apart.
    const LOWEST_CEILING = 3.0;
    expect(FLOOR_TOP).toBeLessThan(LOWEST_CEILING - CEILING_DROP);
  });

  it("makes every wide kind reach WIDE_REACH on both sides of its anchor", () => {
    const wall = frameForSlot({ x: 3, y: 4, side: "s" });
    for (const kind of PROP_KINDS) {
      if (!PROP_CATALOGUE[kind].wide) continue;
      for (let v = 0; v < PROP_CATALOGUE[kind].variants; v++) {
        const placed = placeMesh(
          buildRecorded(kind, v).mesh,
          0,
          anchorFor(kind, 0),
        );
        const along = positions(placed).map((p) => toLocal(wall, p)[0]);
        expect(Math.min(...along), `${kind} ${String(v)}`).toBeLessThanOrEqual(
          -WIDE_REACH + EPS,
        );
        expect(
          Math.max(...along),
          `${kind} ${String(v)}`,
        ).toBeGreaterThanOrEqual(WIDE_REACH - EPS);
      }
    }
  });

  it("keeps the span tray's rungs symmetric about a = 0", () => {
    // A rung is a short box across the tray (small along `a`, wide across
    // `d`); a rail is wide along `a` and a hanger rod is narrow across `d`,
    // so this shape tells rungs apart from both without naming positions.
    const f = frameAt([0, 0, 0], 0);
    for (
      let variant = 0;
      variant < PROP_CATALOGUE["span-tray"].variants;
      variant++
    ) {
      const { parts } = buildRecorded("span-tray", variant);
      const rungCenters = parts
        .filter((p) => p.method === "box")
        .map((p) => {
          const as = p.points.map((q) => toLocal(f, q)[0]);
          const ds = p.points.map((q) => toLocal(f, q)[1]);
          return {
            aWidth: Math.max(...as) - Math.min(...as),
            dWidth: Math.max(...ds) - Math.min(...ds),
            centerA: (Math.max(...as) + Math.min(...as)) / 2,
          };
        })
        .filter((p) => p.aWidth < 0.1 && p.dWidth > 0.3)
        .map((p) => p.centerA);
      expect(rungCenters.length, `variant ${String(variant)}`).toBeGreaterThan(
        0,
      );
      for (const a of rungCenters) {
        expect(
          rungCenters.some((b) => Math.abs(b + a) < EPS),
          `a = ${String(a)} has no mirror, variant ${String(variant)}`,
        ).toBe(true);
      }
    }
  });

  it("makes every tall kind stand at least TALL_MIN in every variant, under FLOOR_TOP", () => {
    for (const kind of PROP_KINDS) {
      if (!PROP_CATALOGUE[kind].tall) continue;
      for (let v = 0; v < PROP_CATALOGUE[kind].variants; v++) {
        const placed = placeMesh(
          buildRecorded(kind, v).mesh,
          0,
          anchorFor(kind, 0),
        );
        const top = Math.max(...positions(placed).map((p) => p[1]));
        expect(top, `${kind} ${String(v)}`).toBeGreaterThanOrEqual(
          TALL_MIN - EPS,
        );
        expect(top, `${kind} ${String(v)}`).toBeLessThanOrEqual(
          FLOOR_TOP + EPS,
        );
      }
    }
  });

  it("lays every ladder rung across both rails, on both variants", () => {
    // The rails lean in the (d, h) side view and the rungs follow the same
    // line; a side frame with the wrong sign mirrors the rails in depth, so
    // they cross the rungs like an X and only the middle rung meets them.
    for (let variant = 0; variant < PROP_CATALOGUE.ladder.variants; variant++) {
      const { parts } = buildRecorded("ladder", variant);
      const rungs = parts.filter((p) => p.method === "cylinderAlong");
      const rails = parts.filter((p) => p.method === "extrude");
      expect(rungs.length, `variant ${String(variant)}`).toBeGreaterThan(0);
      for (const rung of rungs) {
        const r = shape(rung.points);
        const touching = rails.filter((rail) => reaches(r, shape(rail.points)));
        expect(
          touching.length,
          `variant ${String(variant)}`,
        ).toBeGreaterThanOrEqual(2);
      }
    }
  });

  it("puts blink-flagged parts exactly in the kinds whose bank blinks (2.6d C15)", () => {
    // Mutation caught: the ooze drawn steady, or a blink part on a steady kind.
    for (const kind of PROP_KINDS)
      for (let v = 0; v < PROP_CATALOGUE[kind].variants; v++) {
        const { parts } = buildRecorded(kind, v);
        expect(
          parts.some((p) => p.flag >= FLAG.blink),
          `${kind} ${String(v)}`,
        ).toBe(PROP_BANK[kind] !== "steady");
      }
  });

  it("paints exactly the six kinds' small parts with the room's accent, and none of them glows, in a look without the props' own accents (2.7 C9)", () => {
    // Mutation caught: a part painted twice, the wrong part painted (a
    // handle's upright, a drawer pull, a middle vent slit, a rib the drum
    // rack shares), or an accent part that glows.
    const mark = accentTint(1).join();
    const PROP_ACCENT_PARTS: Partial<Record<PropKind, readonly number[]>> = {
      stool: [1, 1],
      bench: [1, 1],
      trolley: [1, 1],
      "tool-cart": [2, 3],
      barrel: [2, 6],
      "locker-bank": [3, 4],
    };
    for (const [kind, counts] of Object.entries(PROP_ACCENT_PARTS) as [
      PropKind,
      readonly number[],
    ][]) {
      counts.forEach((n, v) => {
        const { parts } = buildRecorded(kind, v, LOOKS.day);
        const painted = parts.filter((p) => p.tint?.join() === mark);
        expect(painted.length, `${kind} variant ${String(v)}`).toBe(n);
        for (const p of painted) expect(GLOWING).not.toContain(p.flag);
      });
    }
  });

  it("paints one part of each prop in its own accent in look 2, big enough to read but never a whole door, lid or body, and nothing in the room's", () => {
    // Mutation caught: two parts marked (a stripe on two locker doors, a
    // band on every barrel of the cluster), a whole door or lid marked
    // (its surface passes the cap), a whole body marked (it fills the
    // prop), a room accent part left in (the barrel's ribs beside its own
    // band), a kind that loses its coloured part, or a part that glows.
    // One part may be drawn in several strips that meet at their corners
    // (a rim, a band); strips that share no corner are two parts.
    const look = propLook(LOOKS.aperture);
    const OWN = [
      "barrel",
      "bench",
      "breaker-box",
      "conduit-cabinet",
      "crate",
      "crate-stack",
      "drum-rack",
      "filing-cabinet",
      "fume-cabinet",
      "locker-bank",
      "stool",
      "storage-shelf",
      "tool-cart",
      "trolley",
    ];
    const volume = (lo: V3, hi: V3) =>
      Math.max(hi[0] - lo[0], 0.01) *
      Math.max(hi[1] - lo[1], 0.01) *
      Math.max(hi[2] - lo[2], 0.01);
    for (const kind of PROP_KINDS)
      for (let v = 0; v < PROP_CATALOGUE[kind].variants; v++) {
        const at = `${kind} variant ${String(v)}`;
        const { mesh, parts } = buildRecorded(kind, v, look);
        const own = parts.filter((p) => p.tint?.[0] === PROP_MARK);
        expect(
          parts.filter((p) => p.tint?.[0] === ACCENT_MARK),
          at,
        ).toEqual([]);
        for (const p of own) expect(GLOWING, at).not.toContain(p.flag);
        expect(touchingGroups(own.map((p) => p.points)), at).toBe(
          OWN.includes(kind) ? 1 : 0,
        );
        if (own.length === 0) continue;
        // A door is about 2 m² of surface and a big crate's lid about 3; a
        // band all round a big crate, both its faces counted, about 1.
        expect(markedArea(mesh).part, at).toBeLessThan(1.2);
        const whole = shape(parts.flatMap((q) => q.points));
        const part = shape(own.flatMap((q) => q.points));
        expect(
          volume(part.lo, part.hi) / volume(whole.lo, whole.hi),
          at,
        ).toBeLessThan(0.25);
      }
  });

  it("gives the contact shadows the wall-standing props' bodies exactly as the recipes build them", () => {
    // Mutation caught: a shadow extent typed apart from the recipe's (the
    // two drift), or a recipe body moved without its shadow.
    const f0 = frameAt([0, 0, 0], 0);
    for (const [kind, sizes] of Object.entries(WALL_STANDING) as [
      PropKind,
      readonly { a0: number; a1: number; depth: number }[],
    ][])
      sizes.forEach((size, v) => {
        const bodies = buildRecorded(kind, v, LOOKS.day).parts.filter(
          (p) => p.method === "bevelBox",
        );
        const local = bodies.flatMap((p) =>
          p.points.map((q) => toLocal(f0, q)),
        );
        const at = `${kind} variant ${String(v)}`;
        expect(Math.min(...local.map((q) => q[0])), at).toBeCloseTo(size.a0, 5);
        expect(Math.max(...local.map((q) => q[0])), at).toBeCloseTo(size.a1, 5);
        expect(Math.max(...local.map((q) => q[1])), at).toBeCloseTo(
          size.depth,
          5,
        );
      });
  });

  it("keeps the tool cart's drawer fronts visibly proud of its own red body, not buried inside it (2.7 C9)", () => {
    // Mutation caught: the drawer front's depth moved back flush with, or
    // behind, the body's own face, which would bury the accent in the same
    // surface as the body and hide it whatever its tint (found by hand: the
    // brief's own first depths did exactly this).
    const mark = accentTint(1).join();
    for (let v = 0; v < PROP_CATALOGUE["tool-cart"].variants; v++) {
      const { parts } = buildRecorded("tool-cart", v, LOOKS.day);
      const fronts = parts.filter((p) => p.tint?.join() === mark);
      const body = parts.find(
        (p) => p.method === "bevelBox" && p.tint?.join() !== mark,
      );
      expect(fronts.length, `tool-cart variant ${String(v)}`).toBeGreaterThan(
        0,
      );
      if (!body) throw new Error("no body part");
      const f0 = frameAt([0, 0, 0], 0);
      const localD = (points: readonly V3[]) =>
        Math.max(...points.map((p) => toLocal(f0, p)[1]));
      const bodyFace = localD(body.points);
      for (const front of fronts) {
        expect(
          localD(front.points),
          `tool-cart variant ${String(v)}`,
        ).toBeGreaterThan(bodyFace);
      }
    }
  });

  it("keeps the barrel's ribs visibly proud of its own wall, not buried inside it (2.7 C9)", () => {
    // Mutation caught: a rib's inset widened back to today's (drum rack's
    // own), which would put the rib's outer edge exactly on the barrel's
    // own wall, buried in the same surface, invisible whatever its tint.
    for (let v = 0; v < PROP_CATALOGUE.barrel.variants; v++) {
      const { parts } = buildRecorded("barrel", v);
      const bodies = parts.filter((p) => {
        if (p.method !== "cylinder") return false;
        const s = shape(p.points);
        return s.hi[1] - s.lo[1] > 0.5; // the tall body, not the thin lid
      });
      const rings = parts.filter((p) => p.method === "ring");
      expect(bodies.length, `barrel variant ${String(v)}`).toBeGreaterThan(0);
      expect(rings.length, `barrel variant ${String(v)}`).toBeGreaterThan(0);
      for (const ring of rings) {
        const rs = shape(ring.points);
        const centre: [number, number] = [
          (rs.lo[0] + rs.hi[0]) / 2,
          (rs.lo[2] + rs.hi[2]) / 2,
        ];
        let nearest: { dist: number; s: ReturnType<typeof shape> | null } = {
          dist: Infinity,
          s: null,
        };
        for (const b of bodies) {
          const bs = shape(b.points);
          const c: [number, number] = [
            (bs.lo[0] + bs.hi[0]) / 2,
            (bs.lo[2] + bs.hi[2]) / 2,
          ];
          const dist = Math.hypot(c[0] - centre[0], c[1] - centre[1]);
          if (dist < nearest.dist) nearest = { dist, s: bs };
        }
        if (!nearest.s) throw new Error("no matching body");
        const ringRadius = (rs.hi[0] - rs.lo[0]) / 2;
        const bodyRadius = (nearest.s.hi[0] - nearest.s.lo[0]) / 2;
        expect(ringRadius, `barrel variant ${String(v)}`).toBeGreaterThan(
          bodyRadius,
        );
      }
    }
  });

  it("rests every part of every rare kind on the floor, its wall or another part (2.6d C14)", () => {
    // Mutation caught: the puddle's glow film built clear of its stain, or
    // a crack floating off its canister.
    for (const kind of RARE_PROP_KINDS)
      for (let v = 0; v < PROP_CATALOGUE[kind].variants; v++) {
        const { parts } = buildRecorded(kind, v);
        const wall =
          PROP_CATALOGUE[kind].anchor === "wall" ? frameAt([0, 0, 0], 0) : null;
        expect(looseParts(parts, wall), `${kind} ${String(v)}`).toEqual([]);
      }
  });
});
