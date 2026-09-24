import { describe, expect, it } from "vitest";

import { CELL } from "../world/generate";
import type { Decor } from "../world/types";
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
  frameAt,
  frameForDecor,
  frameForSlot,
  type Frame,
  type Kit,
} from "./kit";

const S: Surface = { layer: 1, tint: [0.5, 0.6, 0.7], flag: FLAG.lit };

const DECOR: Decor = {
  kind: "generator",
  x: 7.5,
  y: 4,
  turn: 1,
  seed: 9,
};

/** Every frame a primitive is checked in: the four wall sides, two turned decor frames. */
const FRAMES: [string, Frame][] = [
  ["north wall", frameForSlot({ x: 3, y: 5, side: "n" })],
  ["south wall", frameForSlot({ x: 3, y: 5, side: "s" })],
  ["west wall", frameForSlot({ x: 3, y: 5, side: "w" })],
  ["east wall", frameForSlot({ x: 3, y: 5, side: "e" })],
  ["decor turned once", frameForDecor(DECOR)],
  ["decor turned three times", frameForDecor({ ...DECOR, turn: 3 })],
];

interface Vertex {
  pos: V3;
  normal: V3;
}

function vertices(m: MeshData): Vertex[] {
  return Array.from({ length: m.count }, (_, i) => {
    const o = i * FLOATS_PER_VERTEX;
    const v = (k: number) => m.vertices[o + k] ?? NaN;
    return { pos: [v(0), v(1), v(2)], normal: [v(3), v(4), v(5)] };
  });
}

function triangles(m: MeshData): [Vertex, Vertex, Vertex][] {
  const vs = vertices(m);
  const out: [Vertex, Vertex, Vertex][] = [];
  for (let t = 0; t + 2 < vs.length; t += 3) {
    const [a, b, c] = [vs[t], vs[t + 1], vs[t + 2]];
    if (!a || !b || !c) throw new Error("short triangle");
    out.push([a, b, c]);
  }
  return out;
}

const sub = (p: readonly number[], q: readonly number[]): V3 => [
  (p[0] ?? 0) - (q[0] ?? 0),
  (p[1] ?? 0) - (q[1] ?? 0),
  (p[2] ?? 0) - (q[2] ?? 0),
];
const cross = (p: V3, q: V3): V3 => [
  p[1] * q[2] - p[2] * q[1],
  p[2] * q[0] - p[0] * q[2],
  p[0] * q[1] - p[1] * q[0],
];
const dot = (p: readonly number[], q: readonly number[]) =>
  (p[0] ?? 0) * (q[0] ?? 0) +
  (p[1] ?? 0) * (q[1] ?? 0) +
  (p[2] ?? 0) * (q[2] ?? 0);

/**
 * The same check as geometry.test.ts: every triangle's geometric normal
 * (the cross product of its edges, counter-clockwise seen from the front)
 * against its stored normal. Returns the smallest dot product.
 */
function worstWinding(m: MeshData): number {
  let worst = Infinity;
  for (const [a, b, c] of triangles(m)) {
    const n = cross(sub(b.pos, a.pos), sub(c.pos, a.pos));
    worst = Math.min(worst, dot(n, a.normal) / Math.hypot(...n));
  }
  return worst;
}

/**
 * The signed volume enclosed by the triangles, measured from the frame's
 * origin so float32 rounding far from the world origin stays small. A closed
 * shape wound outward has a positive volume.
 */
function signedVolume(m: MeshData, f: Frame): number {
  let v = 0;
  for (const [a, b, c] of triangles(m)) {
    const p = sub(a.pos, f.origin);
    const q = sub(b.pos, f.origin);
    const r = sub(c.pos, f.origin);
    v += dot(p, cross(q, r)) / 6;
  }
  return v;
}

/** A world point back in the frame's local `[a, d, h]`. */
function toLocal(f: Frame, p: V3): V3 {
  const o = sub(p, f.origin);
  return [dot(o, f.along), dot(o, f.inward), o[1]];
}

/** A world direction back in the frame's local `[a, d, h]`. */
function dirToLocal(f: Frame, n: V3): V3 {
  return [dot(n, f.along), dot(n, f.inward), n[1]];
}

type Bounds = [
  a0: number,
  a1: number,
  d0: number,
  d1: number,
  h0: number,
  h1: number,
];

interface Case {
  name: string;
  emit: (k: Kit) => void;
  count: number;
  bounds: Bounds;
  /** The enclosed volume when the shape is closed: a number to match, or `true` for "above zero". */
  closed?: number | true;
  /**
   * How far a triangle faces out, from its centroid and its stored normal
   * (both local `[a, d, h]`): positive for every triangle of a primitive
   * wound outward. This is the orientation check that does not lean on the
   * kit's own normals agreeing with its winding.
   */
  outward: (centroid: V3, normal: V3) => number;
}

/** Outwardness measured from an interior reference point chosen per triangle. */
const awayFrom =
  (ref: (c: V3) => V3) =>
  (c: V3, n: V3): number =>
    dot(sub(c, ref(c)), n);

/** Outwardness from the centre of a convex shape's bounds. */
const fromCentre = ([a0, a1, d0, d1, h0, h1]: Bounds) =>
  awayFrom(() => [(a0 + a1) / 2, (d0 + d1) / 2, (h0 + h1) / 2]);

/**
 * Outwardness of a torus: from the nearest point on the tube's centre
 * circle, which lies in the plane of the two axes named by `u` and `v`.
 */
const fromTube =
  (centre: V3, radius: number, u: 0 | 2, v: 1 | 2) =>
  (c: V3, n: V3): number => {
    const du = c[u] - centre[u];
    const dv = c[v] - centre[v];
    const len = Math.hypot(du, dv);
    const ref: V3 = [centre[0], centre[1], centre[2]];
    ref[u] += (du / len) * radius;
    ref[v] += (dv / len) * radius;
    return dot(sub(c, ref), n);
  };

/**
 * Outwardness of an extrusion: the front must face `+d`, the back `-d`,
 * and each side must face the same way as the outward normal of the
 * outline edge it stands on. A triangle on no edge scores -1.
 */
const fromOutline =
  (polygon: readonly (readonly [number, number])[], d0: number, d1: number) =>
  (c: V3, n: V3): number => {
    if (Math.abs(n[1]) > 0.5) return c[1] > (d0 + d1) / 2 ? n[1] : -n[1];
    let area = 0;
    polygon.forEach((p, i) => {
      const q = polygon[(i + 1) % polygon.length] ?? p;
      area += p[0] * q[1] - q[0] * p[1];
    });
    for (let i = 0; i < polygon.length; i++) {
      const p = polygon[i];
      const q = polygon[(i + 1) % polygon.length];
      if (!p || !q) continue;
      const [ea, eh] = [q[0] - p[0], q[1] - p[1]];
      const t = Math.min(
        1,
        Math.max(
          0,
          ((c[0] - p[0]) * ea + (c[2] - p[1]) * eh) / (ea * ea + eh * eh),
        ),
      );
      const off = Math.hypot(c[0] - p[0] - t * ea, c[2] - p[1] - t * eh);
      if (off > 1e-5) continue;
      const sign = area > 0 ? 1 : -1;
      return sign * (n[0] * eh - n[2] * ea);
    }
    return -1;
  };

/** The area of a regular polygon of `sides` sides inscribed in radius `r`. */
const ngon = (sides: number, r: number) =>
  (sides / 2) * r * r * Math.sin((2 * Math.PI) / sides);

const L_SHAPE: [number, number][] = [
  [0.5, 0.3],
  [0, 0.3],
  [0, 1],
  [-0.5, 1],
  [-0.5, 0],
  [0.5, 0],
];
const L_AREA = 0.65;

const CASES: Case[] = [
  {
    name: "box",
    emit: (k) => {
      k.box(-0.4, 0.6, 0.1, 0.5, 0, 1.2, S);
    },
    count: 36,
    bounds: [-0.4, 0.6, 0.1, 0.5, 0, 1.2],
    closed: 1 * 0.4 * 1.2,
    outward: fromCentre([-0.4, 0.6, 0.1, 0.5, 0, 1.2]),
  },
  {
    name: "bevel box",
    emit: (k) => {
      k.bevelBox(-0.5, 0.5, 0, 0.6, 0.2, 1.1, 0.05, S);
    },
    count: 132,
    bounds: [-0.5, 0.5, 0, 0.6, 0.2, 1.1],
    closed: true,
    outward: fromCentre([-0.5, 0.5, 0, 0.6, 0.2, 1.1]),
  },
  {
    name: "capped cylinder",
    emit: (k) => {
      k.cylinder(0.2, 0.3, 0.1, 0.9, 0.25, 12, S);
    },
    count: 12 * 6 + 12 * 6,
    bounds: [-0.05, 0.45, 0.05, 0.55, 0.1, 0.9],
    closed: ngon(12, 0.25) * 0.8,
    outward: fromCentre([-0.05, 0.45, 0.05, 0.55, 0.1, 0.9]),
  },
  {
    name: "open cylinder",
    emit: (k) => {
      k.cylinder(0.2, 0.3, 0.1, 0.9, 0.25, 12, S, false);
    },
    count: 12 * 6,
    bounds: [-0.05, 0.45, 0.05, 0.55, 0.1, 0.9],
    outward: fromCentre([-0.05, 0.45, 0.05, 0.55, 0.1, 0.9]),
  },
  {
    name: "cylinder along the wall",
    emit: (k) => {
      k.cylinderAlong(-0.6, 0.4, 0.2, 1.3, 0.05, 8, S);
    },
    count: 8 * 12,
    bounds: [-0.6, 0.4, 0.15, 0.25, 1.25, 1.35],
    closed: ngon(8, 0.05) * 1,
    outward: fromCentre([-0.6, 0.4, 0.15, 0.25, 1.25, 1.35]),
  },
  {
    name: "ring facing inward",
    emit: (k) => {
      k.ring(0, 0.1, 1.5, 0.4, 0.04, 8, 16, S, "inward");
    },
    count: 16 * 8 * 6,
    bounds: [-0.44, 0.44, 0.06, 0.14, 1.06, 1.94],
    closed: true,
    outward: fromTube([0, 0.1, 1.5], 0.4, 0, 2),
  },
  {
    name: "ring facing up",
    emit: (k) => {
      k.ring(0.1, 0.3, 0.8, 0.3, 0.03, 6, 12, S, "up");
    },
    count: 12 * 6 * 6,
    bounds: [-0.23, 0.43, -0.03, 0.63, 0.77, 0.83],
    closed: true,
    outward: fromTube([0.1, 0.3, 0.8], 0.3, 0, 1),
  },
  {
    name: "closed lathe",
    emit: (k) => {
      k.lathe(
        0.1,
        0.2,
        [
          [0, 0],
          [0.3, 0],
          [0.3, 1],
          [0.2, 1.2],
          [0, 1.2],
        ],
        12,
        S,
      );
    },
    count: 12 * (3 + 6 + 6 + 3),
    bounds: [-0.2, 0.4, -0.1, 0.5, 0, 1.2],
    closed: true,
    outward: fromCentre([-0.2, 0.4, -0.1, 0.5, 0, 1.2]),
  },
  {
    name: "open lathe",
    emit: (k) => {
      k.lathe(
        0.1,
        0.2,
        [
          [0.2, 0],
          [0.3, 0.5],
          [0.1, 1],
        ],
        10,
        S,
      );
    },
    count: 10 * 2 * 6,
    bounds: [-0.2, 0.4, -0.1, 0.5, 0, 1],
    outward: awayFrom((c) => [0.1, 0.2, c[2]]),
  },
  {
    name: "concave extrusion",
    emit: (k) => {
      k.extrude(L_SHAPE, 0.05, 0.25, S);
    },
    count: 12 * L_SHAPE.length - 12,
    bounds: [-0.5, 0.5, 0.05, 0.25, 0, 1],
    closed: L_AREA * 0.2,
    outward: fromOutline(L_SHAPE, 0.05, 0.25),
  },
  {
    name: "clockwise extrusion",
    emit: (k) => {
      k.extrude([...L_SHAPE].reverse(), 0.05, 0.25, S);
    },
    count: 12 * L_SHAPE.length - 12,
    bounds: [-0.5, 0.5, 0.05, 0.25, 0, 1],
    closed: L_AREA * 0.2,
    outward: fromOutline(L_SHAPE, 0.05, 0.25),
  },
  {
    name: "pinched lathe touching the axis mid-way",
    emit: (k) => {
      k.lathe(
        0.1,
        0.2,
        [
          [0, 0],
          [0.3, 0],
          [0.3, 0.4],
          [0, 0.5],
          [0.3, 0.6],
          [0.3, 1],
          [0, 1],
        ],
        12,
        S,
      );
    },
    count: 12 * (3 + 6 + 3 + 3 + 6 + 3),
    bounds: [-0.2, 0.4, -0.1, 0.5, 0, 1],
    closed: true,
    // Each half is convex: measure from the axis at the middle of its half.
    outward: awayFrom((c) => [0.1, 0.2, c[2] < 0.5 ? 0.25 : 0.75]),
  },
  {
    name: "lathe with a segment on the axis",
    emit: (k) => {
      k.lathe(
        0.1,
        0.2,
        [
          [0, 0],
          [0.3, 0],
          [0.3, 1],
          [0, 1],
          [0, 1.5],
        ],
        12,
        S,
      );
    },
    // The last segment lies on the axis and emits nothing.
    count: 12 * (3 + 6 + 3),
    bounds: [-0.2, 0.4, -0.1, 0.5, 0, 1],
    closed: ngon(12, 0.3) * 1,
    outward: fromCentre([-0.2, 0.4, -0.1, 0.5, 0, 1]),
  },
  {
    name: "panel",
    emit: (k) => {
      k.panel(-0.5, 0.5, 0.02, 1, 1.6, S);
    },
    count: 6,
    bounds: [-0.5, 0.5, 0.02, 0.02, 1, 1.6],
    outward: awayFrom((c) => [c[0], -1, c[2]]),
  },
];

function emitIn(f: Frame, emit: (k: Kit) => void): MeshData {
  const b = createBuilder();
  emit(createKit(b, f));
  return b.build();
}

describe("the modelling kit", () => {
  for (const c of CASES) {
    describe(c.name, () => {
      for (const [where, f] of FRAMES) {
        const m = emitIn(f, c.emit);

        it(`emits the promised vertex count (${where})`, () => {
          expect(m.count).toBe(c.count);
          expect(m.vertices.length).toBe(c.count * FLOATS_PER_VERTEX);
        });

        it(`winds every triangle with its unit normal (${where})`, () => {
          for (const v of vertices(m))
            expect(Math.hypot(...v.normal)).toBeCloseTo(1, 5);
          expect(worstWinding(m)).toBeGreaterThan(0.999);
        });

        it(`stays inside its declared bounds (${where})`, () => {
          const [a0, a1, d0, d1, h0, h1] = c.bounds;
          const eps = 1e-5;
          for (const v of vertices(m)) {
            const [a, d, h] = toLocal(f, v.pos);
            expect(a).toBeGreaterThanOrEqual(a0 - eps);
            expect(a).toBeLessThanOrEqual(a1 + eps);
            expect(d).toBeGreaterThanOrEqual(d0 - eps);
            expect(d).toBeLessThanOrEqual(d1 + eps);
            expect(h).toBeGreaterThanOrEqual(h0 - eps);
            expect(h).toBeLessThanOrEqual(h1 + eps);
          }
        });

        it(`points every triangle outward (${where})`, () => {
          let worst = Infinity;
          for (const [a, b, t] of triangles(m)) {
            const [pa, pb, pc] = [a, b, t].map((v) => toLocal(f, v.pos));
            if (!pa || !pb || !pc) throw new Error("short triangle");
            const centroid: V3 = [
              (pa[0] + pb[0] + pc[0]) / 3,
              (pa[1] + pb[1] + pc[1]) / 3,
              (pa[2] + pb[2] + pc[2]) / 3,
            ];
            worst = Math.min(
              worst,
              c.outward(centroid, dirToLocal(f, a.normal)),
            );
          }
          expect(worst).toBeGreaterThan(0);
        });

        if (c.closed !== undefined) {
          const closed = c.closed;
          it(`encloses a positive volume (${where})`, () => {
            const v = signedVolume(m, f);
            expect(v).toBeGreaterThan(0);
            if (closed !== true) expect(v).toBeCloseTo(closed, 4);
          });
        }
      }
    });
  }

  it("faces a panel straight into the room", () => {
    for (const [, f] of FRAMES) {
      const m = emitIn(f, (k) => {
        k.panel(-0.5, 0.5, 0.02, 1, 1.6, S);
      });
      for (const v of vertices(m)) {
        v.normal.forEach((n, i) =>
          expect(n).toBeCloseTo(f.inward[i] ?? NaN, 6),
        );
      }
    }
  });

  it("turns an open cylinder's and an open lathe's faces away from the axis", () => {
    for (const [, f] of FRAMES) {
      const m = emitIn(f, (k) => {
        k.cylinder(0.2, 0.3, 0.1, 0.9, 0.25, 12, S, false);
        k.lathe(
          0.2,
          0.3,
          [
            [0.2, 0],
            [0.3, 0.5],
            [0.1, 1],
          ],
          10,
          S,
        );
      });
      for (const v of vertices(m)) {
        const [a, d] = toLocal(f, v.pos);
        const [na, nd] = dirToLocal(f, v.normal);
        expect(na * (a - 0.2) + nd * (d - 0.3)).toBeGreaterThan(0);
      }
    }
  });

  it("triangulates a concave outline without spilling outside it", () => {
    const f = FRAMES[0]?.[1];
    if (!f) throw new Error("no frame");
    const m = emitIn(f, (k) => {
      k.extrude(L_SHAPE, 0.05, 0.25, S);
    });
    let front = 0;
    for (const [a, b, c] of triangles(m)) {
      const n = dirToLocal(f, a.normal);
      if (n[1] < 0.999) continue;
      const pa = toLocal(f, a.pos);
      const pb = toLocal(f, b.pos);
      const pc = toLocal(f, c.pos);
      front +=
        Math.abs(
          (pb[0] - pa[0]) * (pc[2] - pa[2]) - (pc[0] - pa[0]) * (pb[2] - pa[2]),
        ) / 2;
      // No triangle's centroid falls in the L's notch.
      const ca = (pa[0] + pb[0] + pc[0]) / 3;
      const ch = (pa[2] + pb[2] + pc[2]) / 3;
      expect(ca > 0 && ch > 0.3).toBe(false);
    }
    expect(front).toBeCloseTo(L_AREA, 5);
  });

  it("rejects an outline whose edges cross or that encloses nothing", () => {
    const f = FRAMES[0]?.[1];
    if (!f) throw new Error("no frame");
    const bowtie: [number, number][] = [
      [0, 0],
      [1, 1],
      [1, 0],
      [0, 1],
    ];
    expect(() =>
      emitIn(f, (k) => {
        k.extrude(bowtie, 0, 0.1, S);
      }),
    ).toThrow(/simple/);
    expect(() =>
      emitIn(f, (k) => {
        k.extrude(
          [
            [0, 0],
            [1, 0],
            [2, 0],
          ],
          0,
          0.1,
          S,
        );
      }),
    ).toThrow(/area/);
  });

  it("keeps a bevel box whole when the bevel is as large as the box allows", () => {
    for (const [, f] of FRAMES) {
      const m = emitIn(f, (k) => {
        k.bevelBox(-0.1, 0.1, 0, 0.2, 0, 0.2, 0.5, S);
      });
      expect(worstWinding(m)).toBeGreaterThan(0.999);
      expect(signedVolume(m, f)).toBeGreaterThan(0);
    }
  });

  it("turns a decor frame clockwise seen from above, facing north at turn 0", () => {
    const facing = [0, 1, 2, 3].map((turn) => frameAt([0, 0, 0], turn).inward);
    expect(facing).toEqual([
      [0, 0, -1],
      [1, 0, 0],
      [0, 0, 1],
      [-1, 0, 0],
    ]);
    for (const turn of [0, 1, 2, 3]) {
      const f = frameAt([0, 0, 0], turn);
      // along x up = inward, the handedness every recipe relies on.
      expect(cross(f.along, [0, 1, 0]).map((n) => n + 0)).toEqual(
        f.inward.map((n) => n + 0),
      );
    }
    const f = frameForDecor(DECOR);
    expect(f.origin).toEqual([DECOR.x * CELL, 0, DECOR.y * CELL]);
  });
});
