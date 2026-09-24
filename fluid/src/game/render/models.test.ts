import { describe, expect, it, vi } from "vitest";

import { decorFootprint, footprint, footprintOf } from "../world/footprints";
import { MACHINE_KINDS } from "../world/generate";
import type {
  Box,
  Decor,
  DecorKind,
  DoorStyle,
  Fixture,
  Rect,
  Side,
  WallSlot,
} from "../world/types";
import { CELL } from "../world/units";
import {
  FLAG,
  FLOATS_PER_VERTEX,
  createBuilder,
  type MeshData,
  type Surface,
  type V3,
} from "./geometry";
import { createKit, frameForDecor, frameForSlot, type Frame } from "./kit";
import { LOOKS } from "./looks";
import {
  BLAST_DOWN_TRAVEL,
  BLAST_UP_TRAVEL,
  BULKHEAD_TRAVEL,
  FLUSH_DEPTH,
  HEADROOM,
  HOUSING_DEPTH,
  PIPE_DROP,
  SLIDE_TRAVEL,
  buildDecor,
  buildFixture,
  pipeLength,
  type ModelContext,
  type Mover,
} from "./models";

/** One kit call, as the recording kit saw it. */
interface Part {
  /** The builder it emitted into: the room's, or a mover's own. */
  builder: object;
  method: string;
  layer: number;
  flag: number;
  points: V3[];
}

/**
 * Every kit made while a model builds, the models' own mover kits
 * included, records each primitive call with its surface and its own
 * vertices (emitted a second time into a scratch builder).
 */
const rec = vi.hoisted(() => ({ parts: [] as Part[] }));

vi.mock("./kit", async (importOriginal) => {
  // Only the kit is imported here: geometry imports the kit, so importing
  // it from this factory would wait on itself. The kit emits through
  // `builder.vertex` alone, which is all a scratch builder needs.
  const real = await importOriginal<typeof import("./kit")>();
  type Fn = (...args: unknown[]) => void;
  type KitBuilder = Parameters<typeof real.createKit>[0];
  const call = (kit: object, name: string, args: unknown[]) => {
    (kit as Record<string, Fn | undefined>)[name]?.(...args);
  };
  return {
    ...real,
    createKit: (builder: KitBuilder, f: Frame) => {
      const kit = real.createKit(builder, f);
      const wrapped: Record<string, Fn> = {};
      for (const name of Object.keys(kit)) {
        wrapped[name] = (...args: unknown[]) => {
          call(kit, name, args);
          const points: V3[] = [];
          const scratch = {
            vertex: (p: V3) => points.push([p[0], p[1], p[2]]),
          } as unknown as KitBuilder;
          call(real.createKit(scratch, f), name, args);
          const s = args.find(
            (x): x is Surface =>
              typeof x === "object" && x !== null && "flag" in x,
          );
          rec.parts.push({
            builder,
            method: name,
            layer: s?.layer ?? -1,
            flag: s?.flag ?? -1,
            points,
          });
        };
      }
      return wrapped as unknown as ReturnType<typeof real.createKit>;
    },
  };
});

/** The lowest ceiling the generator makes, and the highest. */
const CEILING = 3.0;
const HIGH_CEILING = 5.0;
const EPS = 1e-4;
const HALL: Rect = { x0: 2, y0: 0, x1: 7, y1: 6 };
/** The text layer the test hands out for one-line labels. */
const LABEL_LAYER = 12;

/** A context that records the text keys it was asked for. */
function context(ceiling = CEILING): { ctx: ModelContext; keys: string[] } {
  const keys: string[] = [];
  return {
    keys,
    ctx: {
      look: LOOKS.aperture,
      ceiling,
      hall: HALL,
      textLayer: (key) => {
        keys.push(key);
        if (
          key.startsWith("terminal") ||
          key.startsWith("poster") ||
          key === "placard"
        )
          return { layer: 9, v0: 0, v1: 1 };
        return { layer: LABEL_LAYER, v0: 2 / 6, v1: 3 / 6 };
      },
    },
  };
}

const SIDES: readonly Side[] = ["n", "e", "s", "w"];
const slotOn = (side: Side): WallSlot => ({ x: 3, y: 4, side });

const ADDRESS = { domain: "d", permalink: "p" };

/** Each door style's clear opening: half width and height range. */
const OPENING: Record<DoorStyle, { half: number; h0: number; h1: number }> = {
  sliding: { half: 0.5, h0: 0.02, h1: 2.4 },
  bulkhead: { half: 0.5, h0: 0.18, h1: 2.2 },
  blast: { half: 0.8, h0: 0, h1: 2.2 },
};

/** The index every fixture is built at, which names its keys. */
const INDEX = 7;

/** Every fixture the models draw, by a readable name, on a given wall. */
function fixtures(slot: WallSlot): [string, Fixture][] {
  const out: [string, Fixture][] = [];
  out.push([
    "terminal",
    { kind: "terminal", slot, heading: "H", lines: [], section: 0, seed: 1 },
  ]);
  for (const style of ["sliding", "bulkhead", "blast"] as DoorStyle[]) {
    const door = {
      kind: "door" as const,
      slot,
      style,
      relType: "r",
      label: "L",
      seed: 2,
    };
    out.push([
      `door ${style}`,
      { ...door, address: ADDRESS, sealedLabel: null },
    ]);
    out.push([
      `door ${style} sealed`,
      { ...door, address: null, sealedLabel: "NO ROUTE" },
    ]);
  }
  for (const crossDomain of [false, true]) {
    const portal = {
      kind: "portal" as const,
      slot,
      label: "L",
      crossDomain,
      seed: 3,
    };
    out.push([
      `portal${crossDomain ? " cross" : ""}`,
      { ...portal, address: ADDRESS, sealedLabel: null },
    ]);
    out.push([
      `portal${crossDomain ? " cross" : ""} sealed`,
      { ...portal, address: null, sealedLabel: "?FILE NOT FOUND" },
    ]);
  }
  out.push([
    "hatch",
    { kind: "hatch", slot, label: "L", address: ADDRESS, seed: 4 },
  ]);
  for (const machine of MACHINE_KINDS) {
    out.push([
      `machine ${machine}`,
      { kind: "machine", slot, machine, tag: "t", hue: 140, seed: 5 },
    ]);
  }
  out.push([
    "poster",
    { kind: "poster", slot, category: "C", lines: [], seed: 6 },
  ]);
  out.push(["placard", { kind: "placard", slot, lines: [] }]);
  return out;
}

/** The one text key each kind of fixture draws. */
const KEY_OF: Record<Fixture["kind"], string> = {
  terminal: `terminal:${INDEX}`,
  door: `door:${INDEX}`,
  portal: `portal:${INDEX}`,
  hatch: `hatch:${INDEX}`,
  machine: `tag:${INDEX}`,
  poster: `poster:${INDEX}`,
  placard: "placard",
};

const DECOR_KINDS: readonly DecorKind[] = [
  "command-console",
  "captain-chair",
  "round-table",
  "council-chair",
  "generator",
  "pipe-run",
  "shelf-row",
  "lab-island",
  "specimen-tank",
];

function positions(m: MeshData): V3[] {
  return Array.from({ length: m.count }, (_, i) => {
    const o = i * FLOATS_PER_VERTEX;
    const v = (k: number) => m.vertices[o + k] ?? NaN;
    return [v(0), v(1), v(2)];
  });
}

function normals(m: MeshData): V3[] {
  return Array.from({ length: m.count }, (_, i) => {
    const o = i * FLOATS_PER_VERTEX + 3;
    const v = (k: number) => m.vertices[o + k] ?? NaN;
    return [v(0), v(1), v(2)];
  });
}

const add = (p: V3, q: V3): V3 => [p[0] + q[0], p[1] + q[1], p[2] + q[2]];
const sub = (p: V3, q: V3): V3 => [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
const scale = (p: V3, k: number): V3 => [p[0] * k, p[1] * k, p[2] * k];
const cross = (p: V3, q: V3): V3 => [
  p[1] * q[2] - p[2] * q[1],
  p[2] * q[0] - p[0] * q[2],
  p[0] * q[1] - p[1] * q[0],
];
const dot = (p: V3, q: V3) => p[0] * q[0] + p[1] * q[1] + p[2] * q[2];

/** The smallest agreement between a triangle's winding and its stored normal. */
function worstWinding(m: MeshData): number {
  const ps = positions(m);
  const ns = normals(m);
  let worst = Infinity;
  for (let t = 0; t + 2 < ps.length; t += 3) {
    const [a, b, c] = [ps[t], ps[t + 1], ps[t + 2]];
    const n = ns[t];
    if (!a || !b || !c || !n) throw new Error("short triangle");
    const g = cross(sub(b, a), sub(c, a));
    const len = Math.hypot(...g);
    if (len < 1e-9) continue;
    worst = Math.min(worst, dot(g, n) / len);
  }
  return worst;
}

/** A world point in a frame's local `[a, d, h]`. */
function toLocal(f: Frame, p: V3): V3 {
  const o = sub(p, f.origin);
  return [dot(o, f.along), dot(o, f.inward), o[1]];
}

const inBox = (b: Box, p: V3) =>
  p[0] >= b.x0 - EPS &&
  p[0] <= b.x1 + EPS &&
  p[2] >= b.z0 - EPS &&
  p[2] <= b.z1 + EPS;

interface Built {
  static: MeshData;
  movers: Mover[];
  /** The static parts, and each mover's parts, in mover order. */
  parts: Part[];
  moverParts: Part[][];
  keys: string[];
}

/** Splits the recorded parts into the room's and each mover's. */
function collect(builder: object, movers: readonly Mover[]) {
  const parts = rec.parts.filter((p) => p.builder === builder);
  const others: object[] = [];
  for (const p of rec.parts) {
    if (p.builder !== builder && !others.includes(p.builder))
      others.push(p.builder);
  }
  const moverParts = others.map((b) =>
    rec.parts.filter((p) => p.builder === b),
  );
  expect(moverParts.length).toBe(movers.length);
  moverParts.forEach((ps, i) => {
    const n = ps.reduce((sum, p) => sum + p.points.length, 0);
    expect(n).toBe(movers[i]?.mesh.count);
  });
  return { parts, moverParts };
}

function buildOne(fx: Fixture, ceiling = CEILING): Built {
  rec.parts = [];
  const builder = createBuilder();
  const { ctx, keys } = context(ceiling);
  const movers = buildFixture((f) => createKit(builder, f), fx, INDEX, ctx);
  return {
    static: builder.build(),
    movers,
    ...collect(builder, movers),
    keys,
  };
}

function buildOneDecor(d: Decor, ceiling = CEILING): Built {
  rec.parts = [];
  const builder = createBuilder();
  const { ctx, keys } = context(ceiling);
  buildDecor((f) => createKit(builder, f), d, ctx);
  return {
    static: builder.build(),
    movers: [],
    ...collect(builder, []),
    keys,
  };
}

const all = (b: Built): MeshData[] => [
  b.static,
  ...b.movers.map((m) => m.mesh),
];
const triangleCount = (b: Built) => all(b).reduce((n, m) => n + m.count / 3, 0);

/** Every mover's parts moved to where the door is fully open. */
function opened(b: Built): { mover: Mover; points: V3[] }[] {
  return b.movers.map((mover, i) => ({
    mover,
    points: (b.moverParts[i] ?? []).flatMap((p) =>
      p.points.map((q) => add(q, scale(mover.axis, mover.travel))),
    ),
  }));
}

/** The closest point on triangle `a b c` to `p` (Ericson's method). */
function closestOnTriangle(p: V3, a: V3, b: V3, c: V3): V3 {
  const ab = sub(b, a);
  const ac = sub(c, a);
  const ap = sub(p, a);
  const d1 = dot(ab, ap);
  const d2 = dot(ac, ap);
  if (d1 <= 0 && d2 <= 0) return a;
  const bp = sub(p, b);
  const d3 = dot(ab, bp);
  const d4 = dot(ac, bp);
  if (d3 >= 0 && d4 <= d3) return b;
  const vc = d1 * d4 - d3 * d2;
  if (vc <= 0 && d1 >= 0 && d3 <= 0) return add(a, scale(ab, d1 / (d1 - d3)));
  const cp = sub(p, c);
  const d5 = dot(ab, cp);
  const d6 = dot(ac, cp);
  if (d6 >= 0 && d5 <= d6) return c;
  const vb = d5 * d2 - d1 * d6;
  if (vb <= 0 && d2 >= 0 && d6 <= 0) return add(a, scale(ac, d2 / (d2 - d6)));
  const va = d3 * d6 - d5 * d4;
  if (va <= 0 && d4 - d3 >= 0 && d5 - d6 >= 0) {
    return add(b, scale(sub(c, b), (d4 - d3) / (d4 - d3 + (d5 - d6))));
  }
  const denom = 1 / (va + vb + vc);
  return add(a, add(scale(ab, vb * denom), scale(ac, vc * denom)));
}

/** How close two parts must come to count as touching, in metres. */
const CONTACT = 0.03;

/** A part's points with their bounds, measured once. */
interface Shape {
  points: readonly V3[];
  lo: V3;
  hi: V3;
}

function shape(points: readonly V3[]): Shape {
  const lo: V3 = [Infinity, Infinity, Infinity];
  const hi: V3 = [-Infinity, -Infinity, -Infinity];
  for (const p of points) {
    for (const k of [0, 1, 2] as const) {
      lo[k] = Math.min(lo[k], p[k]);
      hi[k] = Math.max(hi[k], p[k]);
    }
  }
  return { points, lo, hi };
}

/** Whether a point lies within `CONTACT` of a shape's bounds. */
const nearBounds = (p: V3, s: Shape) =>
  ([0, 1, 2] as const).every(
    (k) => p[k] >= s.lo[k] - CONTACT && p[k] <= s.hi[k] + CONTACT,
  );

/** Whether any vertex of `from` lies within `CONTACT` of a triangle of `to`. */
function reaches(from: Shape, to: Shape): boolean {
  const near = from.points.filter((p) => nearBounds(p, to));
  if (near.length === 0) return false;
  const pts = to.points;
  for (let t = 0; t + 2 < pts.length; t += 3) {
    const [a, b, c] = [pts[t], pts[t + 1], pts[t + 2]];
    if (!a || !b || !c) continue;
    for (const p of near) {
      const q = closestOnTriangle(p, a, b, c);
      if (Math.hypot(...sub(p, q)) <= CONTACT) return true;
    }
  }
  return false;
}

const GLOWING: readonly number[] = [FLAG.emissive, FLAG.frame, FLAG.portal];

/**
 * Every glowing part is in contact with a lit host part (a vertex of one
 * within `CONTACT` of a triangle of the other, either way round) or, for a
 * wall fixture, with the wall plane. Mover parts count as hosts. Nothing
 * glows in mid-air.
 */
function floatingGlow(b: Built, wall: Frame | null): string[] {
  const every = [...b.parts, ...b.moverParts.flat()];
  const hosts = every
    .filter((p) => !GLOWING.includes(p.flag) && p.points.length > 0)
    .map((p) => shape(p.points));
  return every
    .map((p, i) => ({ p, i }))
    .filter(({ p }) => GLOWING.includes(p.flag) && p.points.length > 0)
    .filter(({ p }) => {
      if (wall && p.points.some((q) => toLocal(wall, q)[1] <= CONTACT))
        return false;
      const glow = shape(p.points);
      return !hosts.some((h) => reaches(glow, h) || reaches(h, glow));
    })
    .map(({ p, i }) => `${i}:${p.method}`);
}

describe("fixture models", () => {
  for (const side of SIDES) {
    const slot = slotOn(side);
    const wall = frameForSlot(slot);
    // The wall band a flush part may use: the slot's cell along, FLUSH_DEPTH out.
    const band = footprint(slot, { along: CELL, out: FLUSH_DEPTH });
    for (const [name, fx] of fixtures(slot)) {
      describe(`${name} on the ${side} wall`, () => {
        const built = buildOne(fx);
        const own = footprintOf(fx);
        const inside = (p: V3) =>
          inBox(band, p) || (own !== null && inBox(own, p));

        it("stays inside its footprint and the wall band, under the ceiling", () => {
          for (const m of all(built)) {
            for (const p of positions(m)) {
              expect(inside(p)).toBe(true);
              expect(p[1]).toBeGreaterThanOrEqual(-EPS);
              expect(p[1]).toBeLessThanOrEqual(CEILING - HEADROOM + EPS);
            }
          }
        });

        it("winds every triangle with its normal", () => {
          for (const m of all(built)) {
            expect(m.count % 3).toBe(0);
            expect(m.count).toBeGreaterThan(0);
            expect(worstWinding(m)).toBeGreaterThan(0.999);
          }
        });

        it("stays under the triangle budget", () => {
          expect(triangleCount(built)).toBeLessThan(4000);
        });

        it("glows only on or in its body", () => {
          expect(floatingGlow(built, wall)).toEqual([]);
        });

        it("asks for its own text key", () => {
          expect(built.keys).toEqual([KEY_OF[fx.kind]]);
        });

        it("returns movers only for an open door", () => {
          const { movers } = built;
          for (const m of movers) {
            expect(m.key).toBe(`door:${INDEX}`);
            expect(Math.hypot(...m.axis)).toBeCloseTo(1, 9);
          }
          if (fx.kind !== "door" || fx.address === null) {
            expect(movers).toEqual([]);
            return;
          }
          const axes = movers.map((m) => m.axis);
          switch (fx.style) {
            case "sliding":
              expect(movers).toHaveLength(2);
              expect(movers.map((m) => m.travel)).toEqual([
                SLIDE_TRAVEL,
                SLIDE_TRAVEL,
              ]);
              expect(axes.map((a) => dot(a, wall.along)).sort()).toEqual([
                -1, 1,
              ]);
              break;
            case "bulkhead":
              expect(movers).toHaveLength(2);
              expect(movers.map((m) => m.travel)).toEqual([
                BULKHEAD_TRAVEL,
                BULKHEAD_TRAVEL,
              ]);
              expect(axes.map((a) => dot(a, wall.along)).sort()).toEqual([
                -1, 1,
              ]);
              break;
            case "blast":
              expect(movers).toHaveLength(2);
              expect(axes.map((a) => a[1])).toEqual([1, -1]);
              expect(movers.map((m) => m.travel)).toEqual([
                BLAST_UP_TRAVEL,
                BLAST_DOWN_TRAVEL,
              ]);
              break;
          }
        });

        if (fx.kind === "door" && fx.address !== null) {
          for (const ceiling of [CEILING, HIGH_CEILING]) {
            it(`opens fully and cleanly under a ${ceiling} m ceiling`, () => {
              const b = ceiling === CEILING ? built : buildOne(fx, ceiling);
              const label = b.parts.find((p) => p.layer === LABEL_LAYER);
              if (!label) throw new Error("no label");
              const lab = label.points.map((q) => toLocal(wall, q));
              const [la0, la1] = [
                Math.min(...lab.map((q) => q[0])),
                Math.max(...lab.map((q) => q[0])),
              ];
              const [lh0, lh1] = [
                Math.min(...lab.map((q) => q[2])),
                Math.max(...lab.map((q) => q[2])),
              ];
              const labelD = Math.min(...lab.map((q) => q[1]));
              for (const { mover, points } of opened(b)) {
                for (const p of points) {
                  expect(inside(p)).toBe(true);
                  expect(p[1]).toBeLessThanOrEqual(ceiling - HEADROOM + EPS);
                  // Only a leaf sinking into the floor goes below it.
                  if (mover.axis[1] >= 0)
                    expect(p[1]).toBeGreaterThanOrEqual(-EPS);
                  const [a, d, h] = toLocal(wall, p);
                  // Behind the housing front, and never over the label.
                  if (fx.style !== "sliding")
                    expect(d).toBeLessThan(HOUSING_DEPTH);
                  if (a > la0 && a < la1 && h > lh0 && h < lh1)
                    expect(d).toBeLessThan(labelD);
                  // Nothing is left standing in the opening.
                  const o = OPENING[fx.style];
                  expect(
                    Math.abs(a) < o.half - EPS &&
                      h > o.h0 + EPS &&
                      h < o.h1 - EPS,
                  ).toBe(false);
                }
              }
            });
          }
        }
      });
    }
  }
});

describe("decor models", () => {
  for (const kind of DECOR_KINDS) {
    for (const turn of [0, 1, 2, 3]) {
      describe(`${kind} turned ${turn}`, () => {
        const decor: Decor = { kind, x: 4.5, y: 3, turn, seed: 11 };
        const built = buildOneDecor(decor);
        const f = frameForDecor(decor);

        it("stays inside its footprint, under the ceiling", () => {
          const own = decorFootprint(decor);
          const half = pipeLength(decor, HALL) / 2;
          for (const p of positions(built.static)) {
            if (own) {
              expect(inBox(own, p)).toBe(true);
              expect(p[1]).toBeGreaterThanOrEqual(-EPS);
            } else {
              const [a, d, h] = toLocal(f, p);
              expect(Math.abs(a)).toBeLessThanOrEqual(half + EPS);
              expect(Math.abs(d)).toBeLessThanOrEqual(0.4);
              expect(h).toBeGreaterThanOrEqual(CEILING - PIPE_DROP - 0.2);
            }
            expect(p[1]).toBeLessThanOrEqual(CEILING - HEADROOM + EPS);
          }
        });

        it("winds every triangle with its normal", () => {
          expect(built.static.count).toBeGreaterThan(0);
          expect(worstWinding(built.static)).toBeGreaterThan(0.999);
        });

        it("stays under the triangle budget", () => {
          expect(triangleCount(built)).toBeLessThan(4000);
        });

        it("glows only on or in its body", () => {
          expect(floatingGlow(built, null)).toEqual([]);
        });

        it("asks for no text", () => {
          expect(built.keys).toEqual([]);
        });
      });
    }
  }

  it("sizes a pipe run to the hall: at most 6 m, 1 m short of the hall", () => {
    const d: Decor = { kind: "pipe-run", x: 4.5, y: 3, turn: 0, seed: 1 };
    expect(pipeLength(d, HALL)).toBe(6);
    expect(pipeLength(d, { x0: 0, y0: 0, x1: 3, y1: 9 })).toBe(5);
    expect(pipeLength({ ...d, turn: 1 }, { x0: 0, y0: 0, x1: 9, y1: 3 })).toBe(
      5,
    );
  });

  it("hangs pipe runs under a higher ceiling too", () => {
    const d: Decor = { kind: "pipe-run", x: 4.5, y: 3, turn: 0, seed: 1 };
    const built = buildOneDecor(d, HIGH_CEILING);
    for (const p of positions(built.static)) {
      expect(p[1]).toBeGreaterThan(HIGH_CEILING - PIPE_DROP - 0.2);
      expect(p[1]).toBeLessThanOrEqual(HIGH_CEILING - HEADROOM + EPS);
    }
  });
});

describe("text rows", () => {
  it("maps a label onto its row of the layer", () => {
    const slot = slotOn("n");
    const fx = fixtures(slot).find(([n]) => n === "door sliding")?.[1];
    if (!fx) throw new Error("no door");
    const { static: m } = buildOne(fx);
    const vs: number[] = [];
    for (let i = 0; i < m.count; i++) {
      const o = i * FLOATS_PER_VERTEX;
      if (m.vertices[o + 8] === LABEL_LAYER) vs.push(m.vertices[o + 7] ?? NaN);
    }
    expect(vs.length).toBe(6);
    expect(Math.min(...vs)).toBeCloseTo(2 / 6, 6);
    expect(Math.max(...vs)).toBeCloseTo(3 / 6, 6);
  });
});
