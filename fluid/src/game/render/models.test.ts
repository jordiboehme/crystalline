import { describe, expect, it } from "vitest";

import { CELL, MACHINE_KINDS } from "../world/generate";
import {
  decorFootprint,
  footprint,
  footprintOf,
  type Box,
} from "../world/move";
import type {
  Decor,
  DecorKind,
  DoorStyle,
  Fixture,
  Rect,
  Side,
  WallSlot,
} from "../world/types";
import {
  FLAG,
  FLOATS_PER_VERTEX,
  createBuilder,
  type MeshData,
  type Surface,
  type V3,
} from "./geometry";
import {
  createKit,
  frameForDecor,
  frameForSlot,
  type Frame,
  type Kit,
} from "./kit";
import { LOOKS } from "./looks";
import {
  FLUSH_DEPTH,
  HEADROOM,
  PIPE_DROP,
  buildDecor,
  buildFixture,
  pipeLength,
  type ModelContext,
  type Mover,
} from "./models";

/** The lowest ceiling the generator makes. */
const CEILING = 3.0;
const EPS = 1e-4;
const HALL: Rect = { x0: 2, y0: 0, x1: 7, y1: 6 };

const CTX: ModelContext = {
  look: LOOKS.aperture,
  ceiling: CEILING,
  hall: HALL,
  textLayer: (key) => {
    if (
      key.startsWith("terminal") ||
      key.startsWith("poster") ||
      key === "placard"
    )
      return { layer: 9, v0: 0, v1: 1 };
    return { layer: 12, v0: 2 / 6, v1: 3 / 6 };
  },
};

const SIDES: readonly Side[] = ["n", "e", "s", "w"];
const slotOn = (side: Side): WallSlot => ({ x: 3, y: 4, side });

const ADDRESS = { domain: "d", permalink: "p" };

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

/** One kit call, as the recording kit saw it. */
interface Part {
  method: string;
  flag: number;
  points: V3[];
}

/** Calls a kit primitive by name. */
function call(kit: Kit, name: string, args: unknown[]) {
  const fns = kit as unknown as Record<string, (...a: unknown[]) => void>;
  fns[name]?.(...args);
}

/**
 * A kit factory that emits into `builder` like the room mesh's, and also
 * records each primitive call with its flag and its own vertices, so the
 * glow check can look at parts rather than at the model's overall box.
 */
function recorder(builder: ReturnType<typeof createBuilder>) {
  const parts: Part[] = [];
  const kitAt = (f: Frame): Kit => {
    const real = createKit(builder, f);
    const wrapped = {} as Record<string, (...args: unknown[]) => void>;
    for (const name of Object.keys(real)) {
      wrapped[name] = (...args: unknown[]) => {
        call(real, name, args);
        const scratch = createBuilder();
        call(createKit(scratch, f), name, args);
        const s = args.find(
          (x): x is Surface =>
            typeof x === "object" && x !== null && "flag" in x,
        );
        parts.push({
          method: name,
          flag: s?.flag ?? -1,
          points: positions(scratch.build()),
        });
      };
    }
    return wrapped as unknown as Kit;
  };
  return { kitAt, parts };
}

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

const sub = (p: V3, q: V3): V3 => [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
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
  parts: Part[];
}

function buildOne(fx: Fixture, ctx: ModelContext = CTX): Built {
  const builder = createBuilder();
  const { kitAt, parts } = recorder(builder);
  const movers = buildFixture(kitAt, fx, 7, ctx);
  return { static: builder.build(), movers, parts };
}

function buildOneDecor(d: Decor, ctx: ModelContext = CTX): Built {
  const builder = createBuilder();
  const { kitAt, parts } = recorder(builder);
  buildDecor(kitAt, d, ctx);
  return { static: builder.build(), movers: [], parts };
}

const all = (b: Built): MeshData[] => [
  b.static,
  ...b.movers.map((m) => m.mesh),
];
const triangleCount = (b: Built) => all(b).reduce((n, m) => n + m.count / 3, 0);

type Bounds = [number, number, number, number, number, number];
function boundsOf(points: readonly V3[]): Bounds {
  const b: Bounds = [
    Infinity,
    -Infinity,
    Infinity,
    -Infinity,
    Infinity,
    -Infinity,
  ];
  for (const p of points) {
    b[0] = Math.min(b[0], p[0]);
    b[1] = Math.max(b[1], p[0]);
    b[2] = Math.min(b[2], p[1]);
    b[3] = Math.max(b[3], p[1]);
    b[4] = Math.min(b[4], p[2]);
    b[5] = Math.max(b[5], p[2]);
  }
  return b;
}
const TOUCH = 2e-3;
const touches = (p: Bounds, q: Bounds) =>
  p[0] <= q[1] + TOUCH &&
  q[0] <= p[1] + TOUCH &&
  p[2] <= q[3] + TOUCH &&
  q[2] <= p[3] + TOUCH &&
  p[4] <= q[5] + TOUCH &&
  q[4] <= p[5] + TOUCH;

const GLOWING: readonly number[] = [FLAG.emissive, FLAG.frame, FLAG.portal];

/**
 * Every glowing part is held: it touches a part that is not glowing, or
 * (for a wall fixture) the wall itself, or a glowing part that is held
 * (the portal surface in its glowing ring). Nothing glows in mid-air.
 */
function floatingGlow(b: Built, wall: Frame | null): string[] {
  const glow = b.parts
    .map((p, i) => ({ i, p, box: boundsOf(p.points) }))
    .filter(({ p }) => GLOWING.includes(p.flag) && p.points.length > 0);
  const solid = b.parts
    .filter((p) => !GLOWING.includes(p.flag))
    .map((p) => boundsOf(p.points));
  const held = new Set<number>();
  for (const { i, p, box } of glow) {
    const onWall =
      wall !== null &&
      Math.min(...p.points.map((q) => toLocal(wall, q)[1])) < 0.035;
    if (onWall || solid.some((s) => touches(box, s))) held.add(i);
  }
  for (let grew = true; grew;) {
    grew = false;
    for (const { i, box } of glow) {
      if (held.has(i)) continue;
      if (glow.some((g) => held.has(g.i) && touches(box, g.box))) {
        held.add(i);
        grew = true;
      }
    }
  }
  return glow
    .filter(({ i }) => !held.has(i))
    .map(({ i, p }) => `${i}:${p.method}`);
}

const counts: Record<string, number> = {};

describe("fixture models", () => {
  for (const side of SIDES) {
    const slot = slotOn(side);
    const wall = frameForSlot(slot);
    // The wall band a flush part may use: the slot's cell along, FLUSH_DEPTH out.
    const band = footprint(slot, { along: CELL, out: FLUSH_DEPTH });
    for (const [name, fx] of fixtures(slot)) {
      describe(`${name} on the ${side} wall`, () => {
        const built = buildOne(fx);

        it("stays inside its footprint and the wall band, under the ceiling", () => {
          const own = footprintOf(fx);
          for (const m of all(built)) {
            for (const p of positions(m)) {
              expect(inBox(band, p) || (own !== null && inBox(own, p))).toBe(
                true,
              );
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
          const n = triangleCount(built);
          if (side === "n") counts[name] = n;
          expect(n).toBeLessThan(4000);
        });

        it("glows only on or in its body", () => {
          expect(floatingGlow(built, wall)).toEqual([]);
        });

        it("returns movers only for an open door", () => {
          const { movers } = built;
          for (const m of movers) {
            expect(m.key).toBe("door:7");
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
              expect(movers.map((m) => m.travel)).toEqual([0.8, 0.8]);
              expect(axes.map((a) => dot(a, wall.along)).sort()).toEqual([
                -1, 1,
              ]);
              break;
            case "bulkhead":
              expect(movers).toHaveLength(1);
              expect(axes[0]).toEqual([0, 1, 0]);
              expect(movers[0]?.travel).toBe(2.3);
              break;
            case "blast":
              expect(movers).toHaveLength(2);
              expect(axes.map((a) => a[1]).sort()).toEqual([-1, 1]);
              expect(movers.map((m) => m.travel)).toEqual([1.3, 1.3]);
              break;
          }
        });
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
          const n = triangleCount(built);
          if (turn === 0) counts[kind] = n;
          expect(n).toBeLessThan(4000);
        });

        it("glows only on or in its body", () => {
          expect(floatingGlow(built, null)).toEqual([]);
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
    const built = buildOneDecor(d, { ...CTX, ceiling: 5 });
    for (const p of positions(built.static)) {
      expect(p[1]).toBeGreaterThan(5 - PIPE_DROP - 0.2);
      expect(p[1]).toBeLessThanOrEqual(5 - HEADROOM + EPS);
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
      if (m.vertices[o + 8] === 12) vs.push(m.vertices[o + 7] ?? NaN);
    }
    expect(vs.length).toBe(6);
    expect(Math.min(...vs)).toBeCloseTo(2 / 6, 6);
    expect(Math.max(...vs)).toBeCloseTo(3 / 6, 6);
  });

  it("prints the per-model triangle counts", () => {
    // Collected by the budget tests above; printed for the report.
    console.log(
      Object.entries(counts)
        .map(([k, v]) => `${k}: ${v}`)
        .join("\n"),
    );
    expect(Object.keys(counts).length).toBeGreaterThan(0);
  });
});
