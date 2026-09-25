import { describe, expect, it, vi } from "vitest";

import {
  PIPE_HALF,
  decorFootprint,
  footprint,
  footprintOf,
  pipeRunBox,
} from "../world/footprints";
import { MACHINE_KINDS } from "../world/generate";
import type {
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
  FLOATS_PER_VERTEX,
  createBuilder,
  type MeshData,
  type Surface,
  type V3,
} from "./geometry";
import {
  DECAL_LIFT,
  createKit,
  frameForDecor,
  frameForSlot,
  type Frame,
} from "./kit";
import { LAYER } from "./layers";
import { LOOKS } from "./looks";
import {
  add,
  cross,
  dot,
  floatingGlow,
  inBox,
  positions,
  scale,
  sub,
  toLocal,
  worstWinding,
  type Part,
} from "./modelChecks";
import {
  BLAST_DOWN_TRAVEL,
  BLAST_SPLIT,
  BLAST_UP_TRAVEL,
  BULKHEAD_TRAVEL,
  DISC_SEALED_GAIN,
  FLUSH_DEPTH,
  HEADROOM,
  HOUSING_DEPTH,
  LAMP_IDLE,
  LID_CRACK,
  OPENING,
  PIPE_DROP,
  SLIDE_TRAVEL,
  buildDecor,
  buildFixture,
  pipeLength,
  type ModelContext,
  type Mover,
} from "./models";

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

interface Built {
  static: MeshData;
  movers: Mover[];
  /** The static parts, and each mover's parts, in mover order. */
  parts: Part[];
  moverParts: Part[][];
  keys: string[];
}

/**
 * Splits the recorded parts into the room's and each mover's. A mover's
 * builder is known by the order its kit was first used, which is the
 * order the recipes return their movers in (leaves, lamp, sparks for a
 * door); the vertex count of each checks the pairing.
 */
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

/** Every mover's parts moved to where it is fully open. */
function opened(b: Built): { mover: Mover; points: V3[] }[] {
  return b.movers.map((mover, i) => ({
    mover,
    points: (b.moverParts[i] ?? []).flatMap((p) =>
      p.points.map((q) => add(q, scale(mover.axis, mover.travel))),
    ),
  }));
}

/** The parts of the movers of one kind. */
const partsOf = (b: Built, part: Mover["part"]): Part[] =>
  b.movers.flatMap((m, i) => (m.part === part ? (b.moverParts[i] ?? []) : []));

/** Every part of a built model: the static ones, then each mover's. */
const allParts = (b: Built): Part[] => [...b.parts, ...b.moverParts.flat()];

/** A unit vector along `p`. */
const unit = (p: V3): V3 => scale(p, 1 / Math.hypot(...p));

/** How much two ranges share; negative when they are apart. */
const shared = (lo0: number, hi0: number, lo1: number, hi1: number) =>
  Math.min(hi0, hi1) - Math.max(lo0, lo1);

/** The least two faces must share each way to count as overlapping. */
const OVERLAP = 1e-4;

/**
 * Every decal (a `panel` part: text, pictogram, screen or hazard stripe)
 * that sits closer than `DECAL_LIFT` to a face it covers. A face counts
 * when it faces the same way as the decal, lies within `DECAL_LIFT` of the
 * decal's plane on either side and shares a patch of the decal's extent
 * with it; for a wall fixture the wall plane counts too. Such a pair is
 * what z-fights at a distance, so the list must be empty. Mover parts are
 * measured where the door is closed.
 */
function sunkDecals(b: Built, wall: Frame | null): string[] {
  const every = allParts(b);
  const out: string[] = [];
  every.forEach((decal, i) => {
    if (decal.method !== "panel") return;
    const [p0, p1, p2] = decal.points;
    if (!p0 || !p1 || !p2) return;
    const n = unit(cross(sub(p1, p0), sub(p2, p0)));
    const u = unit(
      Math.abs(n[1]) < 0.9 ? cross(n, [0, 1, 0]) : cross(n, [1, 0, 0]),
    );
    const v = cross(n, u);
    const plane = dot(n, p0);
    const span = (ps: readonly V3[], axis: V3) => {
      const ds = ps.map((p) => dot(p, axis));
      return [Math.min(...ds), Math.max(...ds)] as const;
    };
    const [u0, u1] = span(decal.points, u);
    const [v0, v1] = span(decal.points, v);
    if (wall && dot(n, wall.inward) > 0.999) {
      const lift = Math.min(...decal.points.map((q) => toLocal(wall, q)[1]));
      if (lift < DECAL_LIFT - 1e-6) out.push(`${i}:panel on the wall`);
    }
    every.forEach((face, j) => {
      if (j === i) return;
      const pts = face.points;
      for (let t = 0; t + 2 < pts.length; t += 3) {
        const [a, bb, c] = [pts[t], pts[t + 1], pts[t + 2]];
        if (!a || !bb || !c) continue;
        const g = cross(sub(bb, a), sub(c, a));
        if (Math.hypot(...g) < 1e-9) continue;
        if (dot(unit(g), n) < 0.999) continue;
        const gap = plane - dot(n, a);
        if (Math.abs(gap) >= DECAL_LIFT - 1e-6) continue;
        const [fu0, fu1] = span([a, bb, c], u);
        const [fv0, fv1] = span([a, bb, c], v);
        if (shared(u0, u1, fu0, fu1) <= OVERLAP) continue;
        if (shared(v0, v1, fv0, fv1) <= OVERLAP) continue;
        out.push(`${i}:panel ${gap.toFixed(4)} m from ${j}:${face.method}`);
        return;
      }
    });
  });
  return out;
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
          expect(floatingGlow(allParts(built), wall)).toEqual([]);
        });

        it("lifts every decal DECAL_LIFT off what it covers", () => {
          expect(sunkDecals(built, wall)).toEqual([]);
        });

        it("asks for its own text key", () => {
          expect(built.keys).toEqual([KEY_OF[fx.kind]]);
        });

        it("returns the movers of its kind", () => {
          const { movers } = built;
          for (const m of movers) {
            expect(m.fixture).toBe(INDEX);
            expect(Math.hypot(...m.axis)).toBeCloseTo(1, 9);
          }
          const of = (part: Mover["part"]) =>
            movers.filter((m) => m.part === part);
          switch (fx.kind) {
            case "door": {
              const leaves = of("leaf");
              expect(leaves).toHaveLength(2);
              for (const m of leaves) {
                expect(m.key).toBe(`door:${INDEX}`);
                expect(m.pivot).toBeNull();
                expect(m.rest).toBe(1);
              }
              const axes = leaves.map((m) => m.axis);
              switch (fx.style) {
                case "sliding":
                  expect(leaves.map((m) => m.travel)).toEqual([
                    SLIDE_TRAVEL,
                    SLIDE_TRAVEL,
                  ]);
                  expect(axes.map((a) => dot(a, wall.along)).sort()).toEqual([
                    -1, 1,
                  ]);
                  break;
                case "bulkhead":
                  expect(leaves.map((m) => m.travel)).toEqual([
                    BULKHEAD_TRAVEL,
                    BULKHEAD_TRAVEL,
                  ]);
                  expect(axes.map((a) => dot(a, wall.along)).sort()).toEqual([
                    -1, 1,
                  ]);
                  break;
                case "blast":
                  expect(axes.map((a) => a[1])).toEqual([1, -1]);
                  expect(leaves.map((m) => m.travel)).toEqual([
                    BLAST_UP_TRAVEL,
                    BLAST_DOWN_TRAVEL,
                  ]);
                  break;
              }
              const [lamp, spark] = [of("lamp"), of("spark")];
              expect(lamp.map((m) => [m.key, m.travel, m.rest])).toEqual([
                [`lamp:${INDEX}`, 0, LAMP_IDLE],
              ]);
              expect(spark.map((m) => [m.key, m.travel, m.rest])).toEqual([
                [`spark:${INDEX}`, 0, 0],
              ]);
              expect(movers).toHaveLength(4);
              break;
            }
            case "hatch":
              expect(movers).toHaveLength(1);
              expect(movers[0]?.key).toBe(`lid:${INDEX}`);
              expect(movers[0]?.part).toBe("lid");
              expect(movers[0]?.travel).toBe(LID_CRACK);
              expect(
                dot(movers[0]?.axis ?? [0, 0, 0], wall.inward),
              ).toBeCloseTo(1, 9);
              expect(movers[0]?.rest).toBe(1);
              break;
            case "portal": {
              expect(movers).toHaveLength(1);
              const disc = movers[0];
              if (!disc) throw new Error("no disc");
              expect(disc.key).toBe(`disc:${INDEX}`);
              expect(disc.part).toBe("disc");
              expect(disc.travel).toBe(0);
              const pivot = toLocal(wall, disc.pivot ?? [NaN, NaN, NaN]);
              expect(pivot[0]).toBeCloseTo(0, 9);
              expect(pivot[1]).toBeCloseTo(0.15, 9);
              expect(pivot[2]).toBeCloseTo(1.35, 9);
              expect(disc.rest).toBe(
                fx.sealedLabel === null ? 1 : DISC_SEALED_GAIN,
              );
              break;
            }
            default:
              expect(movers).toEqual([]);
          }
        });

        if (fx.kind === "door") {
          for (const ceiling of [CEILING, HIGH_CEILING]) {
            // The leaves only: the lamp and the sparks never move. The
            // envelope is convex and a leaf travels in a straight line, so
            // the fully open pose bounds every fraction a fault opens to.
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
              const leaves = opened(b).filter((o) => o.mover.part === "leaf");
              expect(leaves).toHaveLength(2);
              for (const { mover, points } of leaves) {
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

        if (fx.kind === "door" && fx.address === null) {
          it("rides a sealed door's hazard marks on its leaves", () => {
            const o = OPENING[fx.style];
            const inOpening = (p: Part) =>
              p.points.every((q) => {
                const [a, , h] = toLocal(wall, q);
                return (
                  Math.abs(a) <= o.half + EPS &&
                  h >= o.h0 - EPS &&
                  h <= o.h1 + EPS
                );
              });
            const leafParts = partsOf(built, "leaf");
            if (fx.style === "blast") {
              // The locking bar belongs to the frame: a bevelled box across
              // the split, left static.
              const bar = built.parts.filter((p) => {
                if (p.method !== "bevelBox") return false;
                const as = p.points.map((q) => toLocal(wall, q)[0]);
                const hs = p.points.map((q) => toLocal(wall, q)[2]);
                return (
                  Math.min(...as) < -o.half &&
                  Math.max(...as) > o.half &&
                  Math.min(...hs) < BLAST_SPLIT &&
                  Math.max(...hs) > BLAST_SPLIT
                );
              });
              expect(bar).toHaveLength(1);
              return;
            }
            const plates = (ps: Part[]) =>
              ps.filter(
                (p) =>
                  p.method === "panel" &&
                  p.layer === LAYER.hazard &&
                  inOpening(p),
              );
            expect(plates(leafParts)).toHaveLength(2);
            expect(plates(built.parts)).toEqual([]);
          });
        }

        if (fx.kind === "door") {
          it("sets its lamp lens on a flat face of its host", () => {
            // The lens is a FLAG.lamp panel, which the glow check does not
            // look at, so it is held here: every corner lies on a static
            // triangle that faces into the room DECAL_LIFT behind it.
            const lens = partsOf(built, "lamp").flatMap((p) => p.points);
            expect(lens.length).toBe(6);
            const local = lens.map((q) => toLocal(wall, q));
            const faceD = (local[0]?.[1] ?? NaN) - DECAL_LIFT;
            const faces: V3[][] = [];
            for (const p of built.parts) {
              for (let t = 0; t + 2 < p.points.length; t += 3) {
                const tri = p.points.slice(t, t + 3);
                const [a, b, c] = tri;
                if (!a || !b || !c) continue;
                const n = cross(sub(b, a), sub(c, a));
                if (Math.hypot(...n) < 1e-12) continue;
                if (dot(unit(n), wall.inward) < 0.999) continue;
                const l = tri.map((q) => toLocal(wall, q));
                if (l.every((q) => Math.abs(q[1] - faceD) < 1e-6))
                  faces.push(l);
              }
            }
            const onTri = (p: V3, [a, b, c]: V3[]) => {
              if (!a || !b || !c) return false;
              const side = (u: V3, v: V3) =>
                (v[0] - u[0]) * (p[2] - u[2]) - (v[2] - u[2]) * (p[0] - u[0]);
              const s = [side(a, b), side(b, c), side(c, a)];
              return s.every((x) => x >= -1e-9) || s.every((x) => x <= 1e-9);
            };
            for (const q of local) {
              expect(faces.some((f) => onTri(q, f))).toBe(true);
            }
          });

          it("stands its sparks on the recess, back faces in its plane", () => {
            const sparks = partsOf(built, "spark").flatMap((p) => p.points);
            const recess = built.parts.filter(
              (p) =>
                p.method === "panel" &&
                p.layer === LAYER.panel &&
                p.points.every(
                  (q) => Math.abs(toLocal(wall, q)[1] - DECAL_LIFT) < 1e-6,
                ),
            );
            expect(recess).toHaveLength(1);
            const r = (recess[0]?.points ?? []).map((q) => toLocal(wall, q));
            const [a0, a1] = [
              Math.min(...r.map((q) => q[0])),
              Math.max(...r.map((q) => q[0])),
            ];
            const [h0, h1] = [
              Math.min(...r.map((q) => q[2])),
              Math.max(...r.map((q) => q[2])),
            ];
            const s = sparks.map((q) => toLocal(wall, q));
            expect(Math.min(...s.map((q) => q[1]))).toBeCloseTo(DECAL_LIFT, 9);
            for (const [a, , h] of s) {
              expect(a).toBeGreaterThanOrEqual(a0 - EPS);
              expect(a).toBeLessThanOrEqual(a1 + EPS);
              expect(h).toBeGreaterThanOrEqual(h0 - EPS);
              expect(h).toBeLessThanOrEqual(h1 + EPS);
            }
          });
        }

        if (fx.kind === "hatch") {
          it("pops the lid clear of the frame, inside the wall band", () => {
            // The lid's back clears the frame's front (0.08 m) by 2 cm at
            // full crack, so the crack shows past the frame.
            const back = Math.min(
              ...partsOf(built, "lid").flatMap((p) =>
                p.points.map((q) => toLocal(wall, q)[1]),
              ),
            );
            expect(back + LID_CRACK).toBeCloseTo(0.08 + 0.02, 9);
          });

          it("pops the hatch lid inside the wall band", () => {
            const lid = built.movers[0];
            if (!lid) throw new Error("no lid");
            const points = partsOf(built, "lid").flatMap((p) => p.points);
            expect(points.length).toBeGreaterThan(0);
            for (const q of points) {
              expect(inBox(band, add(q, scale(lid.axis, LID_CRACK)))).toBe(
                true,
              );
            }
          });
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
          const run = pipeRunBox(decor, HALL);
          if (own === null) expect(run).not.toBeNull();
          for (const p of positions(built.static)) {
            if (own) {
              expect(inBox(own, p)).toBe(true);
              expect(p[1]).toBeGreaterThanOrEqual(-EPS);
            } else {
              const [a, d, h] = toLocal(f, p);
              expect(Math.abs(a)).toBeLessThanOrEqual(half + EPS);
              expect(Math.abs(d)).toBeLessThanOrEqual(PIPE_HALF + EPS);
              // The plan box the span lines keep clear of (E3).
              expect(inBox(run!, p, EPS)).toBe(true);
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
          expect(floatingGlow(allParts(built), null)).toEqual([]);
        });

        it("lifts every decal DECAL_LIFT off what it covers", () => {
          expect(sunkDecals(built, null)).toEqual([]);
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
