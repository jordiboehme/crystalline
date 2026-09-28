import { describe, expect, it, vi } from "vitest";

import {
  PIPE_HALF,
  decorFootprint,
  footprint,
  footprintOf,
  pipeRunBox,
} from "../world/footprints";
import { FIXTURE_SURFACES } from "../world/curios";
import { MACHINE_KINDS } from "../world/generate";
import type {
  Decor,
  DecorKind,
  DoorStyle,
  Fixture,
  MachineKind,
  Rect,
  Side,
  WallSlot,
} from "../world/types";
import { CELL } from "../world/units";
import { VARIANT_COUNTS, tagAccent } from "../world/variants";
import {
  FLAG,
  FLOATS_PER_VERTEX,
  accentTint,
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
import { LAYER, TEXT_BASE, layerPlan } from "./layers";
import { LOOKS } from "./looks";
import {
  add,
  cross,
  GLOWING,
  dot,
  floatingGlow,
  inBox,
  occupancy,
  positions,
  scale,
  shape,
  silhouetteDelta,
  sub,
  toLocal,
  touching,
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
import { CASE_GLASS } from "./models/machines";
import { LIFT_OPENING, LIFT_POCKET } from "./models/lift";
import { LIFT_WORDS } from "../world/lifts";
import { liftsHallRoom } from "../world/canned";

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
            tint: s?.tint ?? null,
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
const LABEL_LAYER = 14;

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
          key.startsWith("lift") ||
          key.startsWith("screen") ||
          key === "placard"
        )
          return { layer: 11, v0: 0, v1: 1 };
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
  for (let v = 0; v < VARIANT_COUNTS.terminal; v++) {
    out.push([
      `terminal variant ${String(v)}`,
      {
        kind: "terminal",
        slot,
        heading: "H",
        lines: [],
        section: 0,
        seed: 1,
        variant: v,
      },
    ]);
  }
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
    for (let v = 0; v < VARIANT_COUNTS.machine[machine]; v++) {
      out.push([
        `machine ${machine} variant ${String(v)}`,
        {
          kind: "machine",
          slot,
          machine,
          tag: "t",
          hue: 140,
          seed: 5,
          variant: v,
        },
      ]);
    }
  }
  out.push([
    "poster",
    { kind: "poster", slot, category: "C", lines: [], seed: 6 },
  ]);
  out.push(["placard", { kind: "placard", slot, lines: [] }]);
  out.push([
    "lift",
    {
      kind: "lift",
      slot,
      stops: Array.from({ length: 14 }, (_, i) => ({
        label: `S${String(i)}`,
        to: { kind: "airlock" as const },
        key: i === 0,
        here: i === 0,
      })),
      note: LIFT_WORDS.deckError,
      seed: 8,
    },
  ]);
  out.push([
    "screen",
    { kind: "screen", slot, lines: ["A", "B"], keys: [0], seed: 9 },
  ]);
  out.push([
    "exit",
    {
      kind: "exit",
      slot,
      label: "DECK 1",
      to: { kind: "airlock" },
      seed: 10,
    },
  ]);
  return out;
}

/** The door style a way is drawn in: an exit is a sliding door (M3 C28). */
const styleOf = (fx: Extract<Fixture, { kind: "door" | "exit" }>): DoorStyle =>
  fx.kind === "door" ? fx.style : "sliding";

/** The one text key each kind of fixture draws. */
const KEY_OF: Record<Fixture["kind"], string> = {
  terminal: `terminal:${INDEX}`,
  door: `door:${INDEX}`,
  portal: `portal:${INDEX}`,
  hatch: `hatch:${INDEX}`,
  machine: `tag:${INDEX}`,
  poster: `poster:${INDEX}`,
  placard: "placard",
  lift: `lift:${INDEX}`,
  screen: `screen:${INDEX}`,
  exit: `exit:${INDEX}`,
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
  it("builds a tag's machine from the tag, never the room (Review Focus 2)", () => {
    // 2.7 C5. Mutation caught: `createRng(fx.seed)` left in any recipe of
    // any variant (tool lengths, bottle heights and LED picks would
    // differ), or a recipe that reads the room's hall or ceiling.
    const slot = slotOn("n");
    const f = frameForSlot(slot);
    expect(MACHINE_KINDS.length).toBe(12);
    const other: Rect = { x0: 0, y0: 1, x1: 12, y1: 20 };
    for (const machine of MACHINE_KINDS)
      for (
        let variant = 0;
        variant < VARIANT_COUNTS.machine[machine];
        variant++
      ) {
        const at = (seed: number, hall: Rect, ceiling: number) => {
          const builder = createBuilder();
          const { ctx } = context(ceiling);
          buildFixture(
            (fr) => createKit(builder, fr),
            {
              kind: "machine",
              slot,
              machine,
              tag: "reactor",
              hue: 140,
              seed,
              variant,
            },
            INDEX,
            { ...ctx, hall },
          );
          return positions(builder.build()).map((p) =>
            toLocal(f, p)
              .map((c) => c.toFixed(5))
              .join(","),
          );
        };
        expect(
          at(5, HALL, CEILING),
          `${machine} variant ${String(variant)}`,
        ).toEqual(at(987654321, other, HIGH_CEILING));
      }
  });

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
            case "lift": {
              // Two leaves under the door key, which the lift's door
              // state drives (M3 C27), and no lamp or sparks.
              expect(movers).toHaveLength(2);
              for (const m of movers) {
                expect(m.key).toBe(`door:${INDEX}`);
                expect(m.part).toBe("leaf");
                expect(m.pivot).toBeNull();
                expect(m.rest).toBe(1);
                expect(dot(m.axis, wall.along)).toBeCloseTo(1, 9);
              }
              break;
            }
            case "door":
            case "exit": {
              const leaves = of("leaf");
              expect(leaves).toHaveLength(2);
              for (const m of leaves) {
                expect(m.key).toBe(`door:${INDEX}`);
                expect(m.pivot).toBeNull();
                expect(m.rest).toBe(1);
              }
              const axes = leaves.map((m) => m.axis);
              switch (styleOf(fx)) {
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

        if (fx.kind === "door" || fx.kind === "exit") {
          const style = styleOf(fx);
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
                  if (style !== "sliding")
                    expect(d).toBeLessThan(HOUSING_DEPTH);
                  if (a > la0 && a < la1 && h > lh0 && h < lh1)
                    expect(d).toBeLessThan(labelD);
                  // Nothing is left standing in the opening.
                  const o = OPENING[style];
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

        if (fx.kind === "lift") {
          it("slides its two doors into the pier beside them, clear of the opening (M3 C27)", () => {
            // Mutation caught: a leaf that stays in the doorway at open 1,
            // one that parks outside the wall band or the pier (seen on
            // the bare wall past the frame), or in front of the pier's face.
            const leaves = opened(built);
            expect(leaves).toHaveLength(2);
            const closed = built.moverParts.flat().flatMap((p) => p.points);
            const closedA = closed.map((q) => toLocal(wall, q)[0]);
            // Shut, the two leaves span the whole opening.
            expect(Math.min(...closedA)).toBeLessThanOrEqual(LIFT_OPENING.a0);
            expect(Math.max(...closedA)).toBeGreaterThanOrEqual(
              LIFT_OPENING.a1,
            );
            for (const { points } of leaves) {
              expect(points.length).toBeGreaterThan(0);
              for (const p of points) {
                expect(inBox(band, p)).toBe(true);
                const [a, d, h] = toLocal(wall, p);
                expect(a).toBeGreaterThanOrEqual(LIFT_POCKET.a0 - EPS);
                expect(a).toBeLessThanOrEqual(LIFT_POCKET.a1 + EPS);
                expect(d).toBeLessThan(LIFT_POCKET.front);
                expect(
                  a > LIFT_OPENING.a0 + EPS &&
                    a < LIFT_OPENING.a1 - EPS &&
                    h > LIFT_OPENING.h0 + EPS &&
                    h < LIFT_OPENING.h1 - EPS,
                ).toBe(false);
              }
            }
          });
        }

        if (fx.kind === "door" || fx.kind === "exit") {
          it("draws its lamp lens as a signal light, off the room's light (H12)", () => {
            const lens = partsOf(built, "lamp");
            expect(lens.length).toBeGreaterThan(0);
            for (const p of lens) expect(p.flag).toBe(FLAG.signal);
          });

          it("sets its lamp lens on a flat face of its host", () => {
            // The lens is a FLAG.signal panel, which the glow check now
            // looks at too (it is in GLOWING), but only as touching some
            // host within 3 cm; the lens's own face is held here tighter:
            // every corner lies on a static triangle that faces into the
            // room DECAL_LIFT behind it.
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

describe("the exit's label (M3 C28)", () => {
  it("reads the exit's own text key from the room's plan, never the door's", () => {
    // Mutation caught: the exit's label drawn under `door:<i>`, which the
    // room's plan has no text for, so the lookup throws.
    const room = liftsHallRoom();
    const index = room.fixtures.findIndex((f) => f.kind === "exit");
    const exit = room.fixtures[index];
    if (exit === undefined) throw new Error("no exit");
    const plan = layerPlan(room);
    const asked: string[] = [];
    const builder = createBuilder();
    const movers = buildFixture((f) => createKit(builder, f), exit, index, {
      look: LOOKS.aperture,
      ceiling: room.ceiling,
      hall: room.hall,
      textLayer: (key) => {
        asked.push(key);
        return plan.lookup(key);
      },
    });
    expect(asked).toEqual([`exit:${String(index)}`]);
    expect(movers.filter((m) => m.part === "leaf").map((m) => m.key)).toEqual([
      `door:${String(index)}`,
      `door:${String(index)}`,
    ]);
  });
});

describe("decor models", () => {
  for (const kind of DECOR_KINDS) {
    for (const turn of [0, 1, 2, 3]) {
      for (let variant = 0; variant < VARIANT_COUNTS.decor[kind]; variant++) {
        describe(`${kind} turned ${turn} variant ${String(variant)}`, () => {
          const decor: Decor = { kind, x: 4.5, y: 3, turn, seed: 11, variant };
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

/** A machine of `machine` in variant `variant`, tag "t", on the north wall. */
function machineAt(machine: MachineKind, variant: number): Fixture {
  return {
    kind: "machine",
    slot: slotOn("n"),
    machine,
    tag: "t",
    hue: 140,
    seed: 5,
    variant,
  };
}

/** The terminal of variant `variant` on the north wall. */
const terminalAt = (variant: number): Fixture => ({
  kind: "terminal",
  slot: slotOn("n"),
  heading: "H",
  lines: [],
  section: 0,
  seed: 1,
  variant,
});

/** A piece of decor of `kind` in variant `variant`, at (4.5, 3), turn 0. */
const decorAt = (kind: DecorKind, variant: number): Decor => ({
  kind,
  x: 4.5,
  y: 3,
  turn: 0,
  seed: 11,
  variant,
});

/**
 * An FNV-1a hash over a mesh's float bits, every vertex float but its three
 * tint floats (offsets 9 to 11), so an accent that only re-tints an existing
 * part (2.7 C9, C10) leaves it as it was.
 */
function meshHash(m: MeshData): string {
  const bits = new Uint32Array(
    m.vertices.buffer,
    m.vertices.byteOffset,
    m.count * FLOATS_PER_VERTEX,
  );
  let h = 0x811c9dc5;
  for (let i = 0; i < m.count; i++) {
    for (let k = 0; k < FLOATS_PER_VERTEX; k++) {
      if (k >= 9 && k <= 11) continue;
      const w = bits[i * FLOATS_PER_VERTEX + k] ?? 0;
      for (let b = 0; b < 4; b++) {
        h ^= (w >>> (8 * b)) & 0xff;
        h = Math.imul(h, 0x01000193) >>> 0;
      }
    }
  }
  return h.toString(16).padStart(8, "0");
}

/**
 * Variant 0's hash of every machine kind, the terminal and every decor kind
 * (2.7 C1), taken from the models before any other variant existed; a new
 * variant never moves them.
 */
const VARIANT_ZERO_HASHES: Record<string, string> = {
  terminal: "5ef012b8",
  "machine:workbench": "73a071c7",
  "machine:lab-bench": "4cb12d3a",
  "machine:server-rack": "85fcdac0",
  "machine:cryo-pod": "c5dbba3b",
  "machine:fabricator": "04ca04ef",
  "machine:hydroponics": "c7291645",
  "machine:nav-table": "8e42665b",
  "machine:comms-array": "cb32155e",
  "machine:reactor-coupling": "ab9813af",
  "machine:cargo-loader": "a4a64aa1",
  "machine:med-scanner": "c6152b5e",
  "machine:containment": "b1c94d20",
  "decor:command-console": "5532e1b9",
  "decor:captain-chair": "9b1ef258",
  "decor:round-table": "b927ad5b",
  "decor:council-chair": "1ae66879",
  "decor:generator": "de41788a",
  "decor:pipe-run": "13545095",
  "decor:shelf-row": "6fd4f6d2",
  "decor:lab-island": "36ff4f55",
  "decor:specimen-tank": "918f0109",
};

/** Every kind's variant 0 hash, by `terminal`, `machine:<kind>`, `decor:<kind>`. */
function variantZeroHashes(): Record<string, string> {
  const out: Record<string, string> = {};
  out.terminal = meshHash(buildOne(terminalAt(0)).static);
  for (const m of MACHINE_KINDS)
    out[`machine:${m}`] = meshHash(buildOne(machineAt(m, 0)).static);
  for (const d of DECOR_KINDS)
    out[`decor:${d}`] = meshHash(buildOneDecor(decorAt(d, 0)).static);
  return out;
}

/** A kind with more than one variant and how to voxelise each of them. */
interface VariantKind {
  name: string;
  count: number;
  occupancy(v: number): Set<string>;
}

/** Every machine kind, the terminal and every decor kind (2.7 C2). */
function variantKinds(): VariantKind[] {
  const wall = frameForSlot(slotOn("n"));
  const out: VariantKind[] = [
    {
      name: "terminal",
      count: VARIANT_COUNTS.terminal,
      occupancy: (v) => occupancy(buildOne(terminalAt(v)).static, wall),
    },
  ];
  for (const m of MACHINE_KINDS)
    out.push({
      name: `machine:${m}`,
      count: VARIANT_COUNTS.machine[m],
      occupancy: (v) => occupancy(buildOne(machineAt(m, v)).static, wall),
    });
  for (const d of DECOR_KINDS)
    out.push({
      name: `decor:${d}`,
      count: VARIANT_COUNTS.decor[d],
      occupancy: (v) => {
        const decor = decorAt(d, v);
        return occupancy(buildOneDecor(decor).static, frameForDecor(decor));
      },
    });
  return out;
}

/** A recorded part's bounds in the north wall's local `[a, d, h]`. */
interface LocalBox {
  part: Part;
  lo: V3;
  hi: V3;
}

/** Every static part of a machine variant, with its local bounds. */
function localParts(fx: Fixture): LocalBox[] {
  const wall = frameForSlot(fx.slot);
  return buildOne(fx).parts.map((part) => {
    const s = shape(part.points.map((q) => toLocal(wall, q)));
    return { part, lo: s.lo, hi: s.hi };
  });
}

const CYLINDERS = ["cylinder", "cylinderAlong", "lathe", "ring"];
const BOXES = ["box", "bevelBox"];
const extent = (b: LocalBox, k: 0 | 1 | 2) => b.hi[k] - b.lo[k];

/**
 * Whether some chain of parts of `methods`, each touching the next, runs
 * from a part `from` accepts to a part `to` accepts.
 */
function chained(
  boxes: readonly LocalBox[],
  methods: readonly string[],
  from: (b: LocalBox) => boolean,
  to: (b: LocalBox) => boolean,
  minLength = 1,
): boolean {
  const pool = boxes.filter((b) => methods.includes(b.part.method));
  const shapes = pool.map((b) => shape(b.part.points));
  const depth: number[] = pool.map((b) => (from(b) ? 1 : 0));
  for (let changed = true; changed;) {
    changed = false;
    shapes.forEach((p, i) => {
      if (depth[i]) return;
      const j = shapes.findIndex(
        (q, k) => (depth[k] ?? 0) > 0 && touching(p, q),
      );
      if (j >= 0) {
        depth[i] = (depth[j] ?? 0) + 1;
        changed = true;
      }
    });
  }
  return pool.some((b, i) => (depth[i] ?? 0) >= minLength && to(b));
}

/** Whether a part of `ps` lies in the north wall's plane (d about 0). */
const onWall = (b: LocalBox) => b.lo[1] <= 0.01;

/** Whether two plan rectangles overlap, `a` and `d` in local terms. */
const overlapsPlan = (
  b: LocalBox,
  r: { a0: number; a1: number; d0: number; d1: number },
) => b.lo[0] < r.a1 && b.hi[0] > r.a0 && b.lo[1] < r.d1 && b.hi[1] > r.d0;

/** The machine body's tint in the test's look. */
const BODY = LOOKS.aperture.palette.machine;
const isBody = (b: LocalBox) => b.part.tint?.join() === BODY.join();
const centreD = (b: LocalBox) => (b.lo[1] + b.hi[1]) / 2;

/** Where the tag strip starts: it and its label sit at or above this height. */
const TAG_ZONE = 2.5;
/** Whether a part is the machine's own, not the tag strip or its label. */
const belowTag = (b: LocalBox) => b.lo[2] < TAG_ZONE - EPS;
const glows = (b: LocalBox) => GLOWING.includes(b.part.flag);
/** A part's points in the north wall's local `[a, d, h]`. */
const localPoints = (b: LocalBox): V3[] => {
  const wall = frameForSlot(slotOn("n"));
  return b.part.points.map((q) => toLocal(wall, q));
};
/** A part's extents, smallest first. */
const extents = (b: LocalBox) =>
  [extent(b, 0), extent(b, 1), extent(b, 2)].sort((x, y) => x - y);
/**
 * A part thin in two directions (at most 4 cm) and at least 15 cm long in
 * the third: a bar, a rod or a strut.
 */
const thinBar = (b: LocalBox) => {
  const [, e1 = 0, e2 = 0] = extents(b);
  return e1 <= 0.04 + EPS && e2 >= 0.15;
};
/**
 * Which way a flat bar in the wall's plane climbs along the wall: +1 when
 * its end furthest along is higher than its other end, -1 when lower.
 */
const climb = (b: LocalBox) => {
  const ps = localPoints(b);
  const lo = ps.reduce((x, y) => (y[0] < x[0] ? y : x));
  const hi = ps.reduce((x, y) => (y[0] > x[0] ? y : x));
  return Math.sign(hi[2] - lo[2]);
};
/** Whether two ranges overlap by more than `EPS`. */
const overlap = (lo0: number, hi0: number, lo1: number, hi1: number) =>
  Math.min(hi0, hi1) - Math.max(lo0, lo1) > EPS;
/** The height of a revolved part's widest point off its own axis. */
const widestAt = (b: LocalBox) => {
  const ca = (b.lo[0] + b.hi[0]) / 2;
  const cd = (b.lo[1] + b.hi[1]) / 2;
  let best = { r: -1, h: 0 };
  for (const [a, d, h] of localPoints(b)) {
    const r = Math.hypot(a - ca, d - cd);
    if (r > best.r + EPS) best = { r, h };
  }
  return best.h;
};
/**
 * Whether some parts of `methods`, each thin along the wall, together
 * close a ring standing across the wall (its axis along it): their points
 * seen from the side surround their common centre in all eight directions
 * and none comes within `hole` of it. Returns the ring's centre and hole,
 * or null.
 */
function uprightRing(
  ps: readonly LocalBox[],
  methods: readonly string[],
  hole: number,
): { a0: number; a1: number; d: number; h: number; r: number } | null {
  const pool = ps.filter(
    (b) =>
      methods.includes(b.part.method) &&
      extent(b, 0) <= 0.6 &&
      extent(b, 1) >= 0.5 &&
      extent(b, 2) >= 0.25,
  );
  for (const first of pool) {
    const group = pool.filter((b) =>
      overlap(b.lo[0], b.hi[0], first.lo[0], first.hi[0]),
    );
    const d =
      (Math.min(...group.map((b) => b.lo[1])) +
        Math.max(...group.map((b) => b.hi[1]))) /
      2;
    const h =
      (Math.min(...group.map((b) => b.lo[2])) +
        Math.max(...group.map((b) => b.hi[2]))) /
      2;
    const octants = new Set<number>();
    let r = Infinity;
    for (const b of group)
      for (const [, pd, ph] of localPoints(b)) {
        const angle = Math.atan2(ph - h, pd - d) + Math.PI;
        octants.add(Math.min(7, Math.floor(angle / (Math.PI / 4))));
        r = Math.min(r, Math.hypot(pd - d, ph - h));
      }
    if (octants.size === 8 && r >= hole) {
      const a0 = Math.min(...group.map((b) => b.lo[0]));
      const a1 = Math.max(...group.map((b) => b.hi[0]));
      return { a0, a1, d, h, r };
    }
  }
  return null;
}

/**
 * What makes each new machine variant that variant (2.7 Tasks 4 and 5's tables), on
 * its recorded parts in the north wall's local terms. Entry `v - 1` is
 * variant `v`'s check; each holds for its variant and fails for variant 0.
 */
const SIGNATURES: Partial<
  Record<MachineKind, ((ps: LocalBox[]) => boolean)[]>
> = {
  workbench: [
    // A closed cabinet on the wall, 0.5 to 0.9 m wide, above 1.1 m, and
    // no pegboard hooks.
    (ps) =>
      ps.some(
        (b) =>
          BOXES.includes(b.part.method) &&
          onWall(b) &&
          b.lo[2] >= 1.1 &&
          extent(b, 0) >= 0.5 - EPS &&
          extent(b, 0) <= 0.9 + EPS,
      ) &&
      !ps.some(
        (b) =>
          BOXES.includes(b.part.method) &&
          b.lo[2] >= 1.6 &&
          b.hi[2] <= 1.75 &&
          b.hi[1] <= 0.06 &&
          extent(b, 0) <= 0.08,
      ),
    // Two gas bottles standing on the floor, 0.08 to 0.12 m in radius.
    (ps) =>
      ps.filter((b) => {
        const r = Math.max(extent(b, 0), extent(b, 1)) / 2;
        return (
          b.part.method === "cylinder" &&
          b.lo[2] <= EPS &&
          r >= 0.08 - EPS &&
          r <= 0.12 + EPS &&
          extent(b, 2) > 2 * r
        );
      }).length >= 2,
  ],
  "lab-bench": [
    // The fume arm: a chain of round parts from the wall reaching out
    // over the bench top.
    (ps) =>
      chained(
        ps,
        CYLINDERS,
        (b) => onWall(b) && b.lo[2] > 0.94,
        (b) => b.hi[1] >= 0.35 && b.lo[2] > 0.94,
      ),
    // The analyser or the reagent tower: a box over 0.4 m tall standing
    // on the bench top at an end, clear of both curio tops.
    (ps) => {
      const tops = FIXTURE_SURFACES.machine["lab-bench"];
      return ps.some(
        (b) =>
          BOXES.includes(b.part.method) &&
          Math.abs(b.lo[2] - 0.94) < 0.005 &&
          extent(b, 2) > 0.4 &&
          tops.every((r) => !overlapsPlan(b, r)) &&
          tops.some((r) => b.lo[0] < r.a1 && b.hi[0] > r.a0),
      );
    },
  ],
  "server-rack": [
    // Two tall cabinets with a gap between them.
    (ps) => {
      const tall = ps.filter(
        (b) => BOXES.includes(b.part.method) && extent(b, 2) > 1.5,
      );
      return tall.some((p) =>
        tall.some((q) => q.lo[0] - p.hi[0] >= 0.05 - EPS),
      );
    },
    // An open frame: four thin corner posts and no tall side panel.
    (ps) =>
      ps.filter(
        (b) =>
          BOXES.includes(b.part.method) &&
          extent(b, 0) < 0.06 &&
          extent(b, 1) < 0.06 &&
          extent(b, 2) > 1.5,
      ).length >= 4 &&
      !ps.some(
        (b) =>
          extent(b, 2) > 1.5 && (extent(b, 0) >= 0.3 || extent(b, 1) >= 0.3),
      ),
  ],
  "cryo-pod": [
    // The capsule leans: its top's centre at least 0.2 m further from the
    // wall than its bottom's.
    (ps) => {
      const capsule = ps.filter(
        (b) => isBody(b) && CYLINDERS.includes(b.part.method),
      );
      if (capsule.length === 0) return false;
      const low = capsule.reduce((x, y) => (y.lo[2] < x.lo[2] ? y : x));
      const high = capsule.reduce((x, y) => (y.hi[2] > x.hi[2] ? y : x));
      return centreD(high) - centreD(low) >= 0.2;
    },
    // A drum: a capsule under 1.6 m tall with a round window.
    (ps) => {
      const capsule = ps.filter((b) => isBody(b) && b.part.method === "lathe");
      const disc = ps.some((b) => {
        if (b.part.method !== "extrude" || b.part.flag !== FLAG.emissive)
          return false;
        const ca = (b.lo[0] + b.hi[0]) / 2;
        const ch = (b.lo[2] + b.hi[2]) / 2;
        const wall = frameForSlot(slotOn("n"));
        const rs = b.part.points.map((q) => {
          const [a, , h] = toLocal(wall, q);
          return Math.hypot(a - ca, h - ch);
        });
        return (
          Math.min(...rs) > 0.9 * Math.max(...rs) && Math.max(...rs) > 0.05
        );
      });
      return (
        capsule.length > 0 && capsule.every((b) => extent(b, 2) < 1.6) && disc
      );
    },
  ],
  fabricator: [
    // The resin printer: a box over 1.4 m tall.
    (ps) => ps.some((b) => BOXES.includes(b.part.method) && extent(b, 2) > 1.4),
    // The arm cell: a chain of at least four round or square parts rising
    // from the top of the machine's wide table (the highest part off the
    // wall at least 1.2 m wide; the tag strip is on the wall).
    (ps) => {
      const wide = ps.filter(
        (b) =>
          BOXES.includes(b.part.method) && !onWall(b) && extent(b, 0) >= 1.2,
      );
      if (wide.length === 0) return false;
      const table = Math.max(...wide.map((b) => b.hi[2]));
      const above = ps.filter((b) => b.lo[2] >= table - 0.002);
      return chained(
        above,
        [...CYLINDERS, ...BOXES],
        (b) => Math.abs(b.lo[2] - table) <= 0.002,
        (b) => b.hi[2] >= table + 0.4,
        4,
      );
    },
  ],
  hydroponics: [
    // The tray rack: three flat trays stacked at three heights.
    (ps) => {
      const trays = ps
        .filter(
          (b) =>
            BOXES.includes(b.part.method) &&
            extent(b, 0) >= 1.2 &&
            extent(b, 1) >= 0.4 &&
            extent(b, 2) <= 0.15,
        )
        .map((b) => b.lo[2])
        .sort((x, y) => x - y);
      let levels = 0;
      let last = -Infinity;
      for (const h of trays) {
        if (h - last > 0.2) levels++;
        if (h - last > 0.2) last = h;
      }
      return levels >= 3;
    },
    // The tube garden: at least five tall upright tubes.
    (ps) =>
      ps.filter(
        (b) =>
          b.part.method === "cylinder" &&
          extent(b, 2) >= 0.6 &&
          extent(b, 2) > 2 * Math.max(extent(b, 0), extent(b, 1)),
      ).length >= 5,
  ],
  "nav-table": [
    // The round plotting table: a flat round top (a lathe or a disc) at
    // table height as wide as the footprint is deep, 0.9 m across both ways.
    (ps) =>
      ps.some(
        (b) =>
          ["lathe", "cylinder"].includes(b.part.method) &&
          b.lo[2] >= 0.5 &&
          extent(b, 2) <= 0.2 &&
          extent(b, 0) >= 0.9 - EPS &&
          extent(b, 1) >= 0.9 - EPS,
      ),
    // The chart lectern: a glowing panel at least 0.6 m wide whose top edge
    // is above 1.2 m (the tag strip aside).
    (ps) =>
      ps.some(
        (b) => glows(b) && belowTag(b) && b.hi[2] > 1.2 && extent(b, 0) >= 0.6,
      ),
  ],
  "comms-array": [
    // The lattice mast: at least twelve thin bars.
    (ps) => ps.filter((b) => belowTag(b) && thinBar(b)).length >= 12,
    // The radio rack: no part above 1.6 m (the tag strip aside).
    (ps) => !ps.some((b) => belowTag(b) && b.hi[2] > 1.6 + EPS),
  ],
  "reactor-coupling": [
    // The lying coupling: a thick cylinder whose axis runs along the wall.
    (ps) =>
      ps.some(
        (b) =>
          b.part.method === "cylinderAlong" &&
          extent(b, 0) >= 0.4 &&
          extent(b, 1) >= 0.5 &&
          extent(b, 2) >= 0.5,
      ),
    // The sphere vessel: a lathe at least 0.6 m across whose widest point
    // is above 0.8 m.
    (ps) =>
      ps.some(
        (b) =>
          b.part.method === "lathe" && extent(b, 0) >= 0.6 && widestAt(b) > 0.8,
      ),
  ],
  "cargo-loader": [
    // The scissor lift: at least four flat bars in the wall's plane, each
    // crossing another that climbs the other way on the same side.
    (ps) => {
      const bars = ps.filter(
        (b) =>
          b.part.method === "extrude" &&
          extent(b, 1) <= 0.08 &&
          extent(b, 0) >= 0.3 &&
          extent(b, 2) >= 0.1,
      );
      return (
        bars.filter((p) =>
          bars.some(
            (q) =>
              climb(q) === -climb(p) &&
              climb(p) !== 0 &&
              overlap(p.lo[0], p.hi[0], q.lo[0], q.hi[0]) &&
              overlap(p.lo[2], p.hi[2], q.lo[2], q.hi[2]) &&
              Math.abs(centreD(p) - centreD(q)) <= 0.1,
          ),
        ).length >= 4
      );
    },
    // The gantry hoist: a crate (every side at least 0.3 m) whose bottom is
    // above 0.3 m with nothing under it: it hangs.
    (ps) =>
      ps.some(
        (b) =>
          BOXES.includes(b.part.method) &&
          Math.min(extent(b, 0), extent(b, 1), extent(b, 2)) >= 0.3 &&
          b.lo[2] > 0.3 &&
          !ps.some(
            (q) =>
              q !== b &&
              q.lo[2] < b.lo[2] - EPS &&
              overlapsPlan(q, {
                a0: b.lo[0],
                a1: b.hi[0],
                d0: b.lo[1],
                d1: b.hi[1],
              }),
          ),
      ),
  ],
  "med-scanner": [
    // The ring scanner: a closed ring (lathe or extrude) standing across
    // the bed, the bed's slab passing through its hole.
    (ps) => {
      const ring = uprightRing(ps, ["lathe", "extrude"], 0.2);
      if (ring === null) return false;
      return ps.some(
        (b) =>
          BOXES.includes(b.part.method) &&
          extent(b, 0) >= 1.0 &&
          extent(b, 2) <= 0.2 &&
          overlap(b.lo[0], b.hi[0], ring.a0, ring.a1) &&
          [b.lo[1], b.hi[1]].every((d) =>
            [b.lo[2], b.hi[2]].every(
              (h) => Math.hypot(d - ring.d, h - ring.h) < ring.r,
            ),
          ),
      );
    },
    // The treatment chair: no bed slab (raised, at least 0.3 m deep and at
    // most 0.3 m thick) longer than 1.2 m along the wall.
    (ps) =>
      !ps.some(
        (b) =>
          BOXES.includes(b.part.method) &&
          belowTag(b) &&
          b.lo[2] > 0.3 &&
          extent(b, 0) > 1.2 &&
          extent(b, 1) >= 0.3 &&
          extent(b, 2) <= 0.3,
      ),
  ],
  containment: [
    // The glass case: four upright corner posts and no round shell (a
    // cylinder or lathe at least 0.5 m across and 0.5 m tall).
    (ps) =>
      ps.filter(
        (b) =>
          BOXES.includes(b.part.method) &&
          extent(b, 0) <= 0.08 &&
          extent(b, 1) <= 0.08 &&
          extent(b, 2) >= 0.8,
      ).length >= 4 &&
      !ps.some(
        (b) =>
          ["cylinder", "lathe"].includes(b.part.method) &&
          extent(b, 0) >= 0.5 &&
          extent(b, 2) >= 0.5,
      ),
    // The twin cells: glowing round parts at least 0.4 m tall on two
    // upright axes at least 0.3 m apart.
    (ps) => {
      const cores = ps.filter(
        (b) =>
          glows(b) &&
          ["cylinder", "lathe"].includes(b.part.method) &&
          extent(b, 2) >= 0.4,
      );
      const axes: [number, number][] = [];
      for (const b of cores) {
        const at: [number, number] = [
          (b.lo[0] + b.hi[0]) / 2,
          (b.lo[1] + b.hi[1]) / 2,
        ];
        if (axes.every(([a, d]) => Math.hypot(a - at[0], d - at[1]) >= 0.3))
          axes.push(at);
      }
      return axes.length >= 2;
    },
  ],
};

/**
 * How many parts of each machine variant carry the tag's second accent
 * (2.7 C10), entry `v` for variant `v`. Variant 0 re-tints parts it already
 * had: the vice's two jaws, the lab bench's three door handles, the rack's
 * five top vents, the pod's collar ring, the fabricator's print carriage,
 * the hydroponics grow light's two arms, the nav table's three metal keys,
 * the comms base's cross bar, the reactor's two collar rings, the loader's
 * mast head, the scanner's two bed rails and the containment cap's ring.
 * Variants 1 and 2 carry the parts their recipes' docs name: the reactor's
 * two collar rings and the twin cells' two cap rings among them.
 */
const ACCENT_PARTS: Record<MachineKind, readonly number[]> = {
  workbench: [2, 3, 2],
  "lab-bench": [3, 1, 1],
  "server-rack": [5, 2, 1],
  "cryo-pod": [1, 1, 1],
  fabricator: [1, 1, 1],
  hydroponics: [2, 3, 1],
  "nav-table": [3, 1, 1],
  "comms-array": [1, 1, 1],
  "reactor-coupling": [2, 2, 2],
  "cargo-loader": [1, 1, 1],
  "med-scanner": [2, 1, 1],
  containment: [1, 1, 2],
};

describe("model variants (2.7)", () => {
  it("keeps every variant 0 as it was (2.7 C1)", () => {
    // Mutation caught: a variant 1 edit that leaks into variant 0's branch.
    const hashes = variantZeroHashes();
    expect(Object.keys(hashes).length).toBe(1 + 12 + 9);
    expect(hashes).toEqual(VARIANT_ZERO_HASHES);
  });

  it("tints the glass case's panes a pale blue-grey glass, not the tag's hue darkened (2.7 Task 5 ruling)", () => {
    // Mutation caught: the panes back in `shade(hue, 0.3)`, which read as
    // a dark cabinet rather than glass.
    const [r, g, b] = CASE_GLASS;
    expect(g).toBeGreaterThanOrEqual(r);
    expect(b).toBeGreaterThan(g);
    // Grey with a little blue, pale but a low glow.
    expect(b - r).toBeLessThanOrEqual(0.15);
    expect(r).toBeGreaterThanOrEqual(0.3);
    expect(b).toBeLessThanOrEqual(0.5);
    const m = buildOne(machineAt("containment", 1)).static;
    let panes = 0;
    for (let i = 0; i < m.count; i++) {
      const o = i * FLOATS_PER_VERTEX;
      const tint = [9, 10, 11].map((k) => m.vertices[o + k]);
      if (tint.every((c, k) => c === Math.fround(CASE_GLASS[k] ?? NaN))) {
        expect(m.vertices[o + 12]).toBe(FLAG.emissive);
        panes++;
      }
    }
    // Four panes, each a box of at least four faces of six vertices.
    expect(panes).toBeGreaterThanOrEqual(4 * 4 * 6);
  });

  it("draws every variant its count names, and throws on one past it (2.7 C2)", () => {
    // Mutation caught: a recipe table shorter or longer than its
    // `VARIANT_COUNTS` entry, or an unknown variant quietly drawn as
    // variant 0 (the old `?? recipes[0]` fallback).
    expect(MACHINE_KINDS.length).toBe(12);
    for (const kind of MACHINE_KINDS) {
      const n = VARIANT_COUNTS.machine[kind];
      for (let v = 0; v < n; v++)
        expect(
          () => buildOne(machineAt(kind, v)),
          `${kind} ${String(v)}`,
        ).not.toThrow();
      expect(() => buildOne(machineAt(kind, n)), kind).toThrow(kind);
    }
    const t = VARIANT_COUNTS.terminal;
    for (let v = 0; v < t; v++)
      expect(() => buildOne(terminalAt(v))).not.toThrow();
    expect(() => buildOne(terminalAt(t))).toThrow("terminal");
    for (const kind of DECOR_KINDS) {
      const n = VARIANT_COUNTS.decor[kind];
      for (let v = 0; v < n; v++)
        expect(
          () => buildOneDecor(decorAt(kind, v)),
          `${kind} ${String(v)}`,
        ).not.toThrow();
      expect(() => buildOneDecor(decorAt(kind, n)), kind).toThrow(kind);
    }
  });

  it("makes every pair of a kind's variants differ in shape (2.7 C2)", () => {
    // Mutation caught: a variant built as another (delta 0), or one that
    // only recolours (the same voxels).
    const kinds = variantKinds().filter((k) => k.count >= 2);
    expect(kinds.length).toBeGreaterThan(0);
    for (const k of kinds)
      for (let a = 0; a < k.count; a++)
        for (let b = a + 1; b < k.count; b++)
          expect(
            silhouetteDelta(k.occupancy(a), k.occupancy(b)),
            `${k.name} ${String(a)} against ${String(b)}`,
          ).toBeGreaterThanOrEqual(0.2);
  }, 30_000);

  for (const machine of MACHINE_KINDS)
    for (const [i, check] of (SIGNATURES[machine] ?? []).entries())
      it(`builds ${machine} variant ${String(i + 1)} to its design (2.7 Tasks 4 and 5)`, () => {
        // Mutation caught: the variant built as variant 0 (the check must
        // fail there, so it cannot pass on any build of the kind).
        expect(VARIANT_COUNTS.machine[machine]).toBeGreaterThan(i + 1);
        expect(check(localParts(machineAt(machine, i + 1)))).toBe(true);
        expect(check(localParts(machineAt(machine, 0)))).toBe(false);
      });

  it("paints the tag's second accent on each variant's trim parts only (2.7 C10)", () => {
    // Mutation caught: an accent part left in its old colour, one painted
    // twice, a glowing part tinted in the accent, or the room's accent mark
    // used instead of the tag's colour.
    const accent = LOOKS.aperture.accents[tagAccent("t")];
    if (!accent) throw new Error("no accent");
    expect(Object.keys(ACCENT_PARTS).length).toBe(MACHINE_KINDS.length);
    for (const machine of MACHINE_KINDS) {
      const counts = ACCENT_PARTS[machine];
      expect(counts.length, machine).toBe(VARIANT_COUNTS.machine[machine]);
      counts.forEach((n, v) => {
        const painted = buildOne(machineAt(machine, v)).parts.filter(
          (p) => p.tint?.join() === accent.join(),
        );
        expect(painted.length, `${machine} variant ${String(v)}`).toBe(n);
        for (const p of painted) expect(GLOWING).not.toContain(p.flag);
      });
    }
  });
});

/** A terminal variant's parts in the north wall's local terms, and the keys it asked for. */
function terminalParts(variant: number): { ps: LocalBox[]; keys: string[] } {
  const fx = terminalAt(variant);
  const wall = frameForSlot(fx.slot);
  const built = buildOne(fx);
  const ps = built.parts.map((part) => {
    const s = shape(part.points.map((q) => toLocal(wall, q)));
    return { part, lo: s.lo, hi: s.hi };
  });
  return { ps, keys: built.keys };
}

/**
 * The screen of a terminal build: its text quad's rectangle (`a0..a1`,
 * `h0..h1`) and the face it stands on, `DECAL_LIFT` behind the quad.
 */
function screenOf(ps: readonly LocalBox[]) {
  const text = ps.filter((b) => b.part.layer >= TEXT_BASE);
  expect(text).toHaveLength(1);
  const [t] = text;
  if (!t) throw new Error("no screen");
  return {
    a0: t.lo[0],
    a1: t.hi[0],
    h0: t.lo[2],
    h1: t.hi[2],
    face: t.lo[1] - DECAL_LIFT,
  };
}

/** How far in front of the wall the desk top reaches (`terminal.ts`'s `DESK_DEPTH`). */
const DESK_FRONT = 0.6;

/**
 * What makes each new terminal variant that variant (2.7 Task 6), on its
 * recorded parts in the north wall's local terms. Entry `v - 1` is variant
 * `v`'s check; each holds for its variant and fails for variant 0.
 */
const TERMINAL_SIGNATURES: ((ps: LocalBox[], keys: string[]) => boolean)[] = [
  // The flat console: nothing behind the screen deeper than 0.12 m from
  // its face (no tube case), and a seat with no back: no part standing
  // wholly in front of the desk rises above 0.6 m.
  (ps) => {
    const sc = screenOf(ps);
    const behind = ps.filter(
      (b) =>
        b.part.layer < TEXT_BASE &&
        b.lo[0] < sc.a1 - EPS &&
        b.hi[0] > sc.a0 + EPS &&
        b.lo[2] < sc.h1 - EPS &&
        b.hi[2] > sc.h0 + EPS &&
        b.lo[1] < sc.face - 0.12 - EPS,
    );
    const back = ps.filter(
      (b) => b.lo[1] >= DESK_FRONT - EPS && b.hi[2] > 0.6 + EPS,
    );
    return behind.length === 0 && back.length === 0;
  },
  // The hooded twin: a part above the screen's top edge, over the screen,
  // that reaches at least 0.1 m out past its face (the hood); a second
  // glowing panel left of the screen at screen height; and no text key but
  // the terminal's own.
  (ps, keys) => {
    const sc = screenOf(ps);
    const hood = ps.some(
      (b) =>
        b.lo[2] >= sc.h1 - EPS &&
        b.hi[1] >= sc.face + 0.1 &&
        b.lo[0] < sc.a1 &&
        b.hi[0] > sc.a0,
    );
    const side = ps.some(
      (b) =>
        b.part.method === "panel" &&
        b.part.layer < TEXT_BASE &&
        glows(b) &&
        b.hi[0] <= sc.a0 + EPS &&
        b.lo[2] < sc.h1 &&
        b.hi[2] > sc.h0,
    );
    return hood && side && keys.join() === `terminal:${String(INDEX)}`;
  },
];

/**
 * How many parts of each terminal variant carry the room's accent mark
 * (2.7 C9), entry `v` for variant `v`: the chair's seat and back and the
 * three drawer pulls on variants 0 and 2 (re-tints of existing parts on
 * variant 0), the stool's seat and the three pulls on variant 1.
 */
const TERMINAL_ACCENT_PARTS: readonly number[] = [5, 4, 5];

describe("terminal variants (2.7 Task 6)", () => {
  it("keeps the screen quad where it was on every terminal variant (2.7 C3)", () => {
    // Mutation caught: a variant that moves, resizes or reshapes the screen
    // (the text would stretch and the reading would move).
    const screen = (v: number) =>
      buildOne({
        kind: "terminal",
        slot: slotOn("n"),
        heading: "H",
        lines: [],
        section: 0,
        seed: 1,
        variant: v,
      })
        .parts.filter((p) => p.layer >= TEXT_BASE)
        .map((p) =>
          p.points.map((q) => q.map((c) => c.toFixed(5)).join(",")).join(";"),
        );
    expect(VARIANT_COUNTS.terminal).toBe(3);
    expect(screen(0)).toHaveLength(1);
    for (let v = 1; v < VARIANT_COUNTS.terminal; v++)
      expect(screen(v)).toEqual(screen(0));
  });

  for (const [i, check] of TERMINAL_SIGNATURES.entries())
    it(`builds terminal variant ${String(i + 1)} to its design (2.7 Task 6)`, () => {
      // Mutation caught: the variant built as variant 0 (the check must
      // fail there, so it cannot pass on any build of the kind).
      expect(VARIANT_COUNTS.terminal).toBeGreaterThan(i + 1);
      const own = terminalParts(i + 1);
      const zero = terminalParts(0);
      expect(check(own.ps, own.keys)).toBe(true);
      expect(check(zero.ps, zero.keys)).toBe(false);
    });

  it("paints the room's accent on the seat, back and drawer pulls only (2.7 C9)", () => {
    // Mutation caught: an accent part left in its old colour, one painted
    // twice, a glowing part (the screen, the side monitor's bars) in the
    // accent, or the tag's baked colour used instead of the mark.
    const mark = accentTint(1).join();
    expect(TERMINAL_ACCENT_PARTS.length).toBe(VARIANT_COUNTS.terminal);
    TERMINAL_ACCENT_PARTS.forEach((n, v) => {
      const painted = buildOne(terminalAt(v)).parts.filter(
        (p) => p.tint?.join() === mark,
      );
      expect(painted.length, `terminal variant ${String(v)}`).toBe(n);
      for (const p of painted) expect(GLOWING).not.toContain(p.flag);
    });
  });
});

/** The frame every `decorAt` piece stands in: (4.5, 3), turn 0. */
const DECOR_FRAME = frameForDecor(decorAt("generator", 0));

/** A decor variant's static parts, with their bounds in the piece's own `[a, d, h]`. */
function decorParts(kind: DecorKind, variant: number): LocalBox[] {
  return buildOneDecor(decorAt(kind, variant)).parts.map((part) => {
    const s = shape(part.points.map((q) => toLocal(DECOR_FRAME, q)));
    return { part, lo: s.lo, hi: s.hi };
  });
}

/** Every triangle normal of a part, in the piece's own `[a, d, h]`. */
function decorNormals(b: LocalBox): V3[] {
  const ps = b.part.points.map((q) => toLocal(DECOR_FRAME, q));
  const out: V3[] = [];
  for (let t = 0; t + 2 < ps.length; t += 3) {
    const [p0, p1, p2] = [ps[t], ps[t + 1], ps[t + 2]];
    if (!p0 || !p1 || !p2) continue;
    const g = cross(sub(p1, p0), sub(p2, p0));
    if (Math.hypot(...g) < 1e-9) continue;
    out.push(unit(g));
  }
  return out;
}

/** Whether a part's recorded tint is the room's accent mark (2.7 C8). */
const accented = (b: LocalBox) => b.part.tint?.join() === accentTint(1).join();
/** A part's centre along `a`, `d` or `h`. */
const mid = (b: LocalBox, k: 0 | 1 | 2) => (b.lo[k] + b.hi[k]) / 2;
/** A revolved part: a cylinder, a lathe or a ring. */
const upright = (b: LocalBox) =>
  b.part.method === "cylinder" || b.part.method === "lathe";

/**
 * What makes each new decor variant that variant (2.7 Task 7's table), on
 * its recorded parts in its own terms. Entry `v - 1` is variant `v`'s
 * check; each holds for its variant and fails for variant 0. Where the
 * table's words alone hold on variant 0 too, the check adds what the
 * design says on top (named on each).
 */
const DECOR_SIGNATURES: Partial<
  Record<DecorKind, ((ps: LocalBox[]) => boolean)[]>
> = {
  "command-console": [
    // v1, the straight console: no yawed frame. Every flat part's faces
    // stand square to the piece: each normal runs along it or not at all.
    (ps) =>
      ps
        .filter((b) =>
          ["box", "bevelBox", "extrude", "panel"].includes(b.part.method),
        )
        .every((b) =>
          decorNormals(b).every(
            (n) => Math.abs(n[0]) < 1e-3 || Math.abs(n[0]) > 1 - 1e-3,
          ),
        ),
    // v2, the horseshoe: at least three screens, and (beyond the table's
    // words, which variant 0's three screens meet) at least three of them
    // turned off the piece's front, the wings curving round the chair.
    (ps) => {
      const screens = ps.filter((b) => b.part.method === "panel" && glows(b));
      const turned = screens.filter((b) =>
        decorNormals(b).every((n) => Math.abs(n[0]) > 0.2),
      );
      return screens.length >= 3 && turned.length >= 3;
    },
  ],
  "captain-chair": [
    // v1: two glowing panels at arm height, one on each arm.
    (ps) => {
      const lit = ps.filter(
        (b) => glows(b) && b.lo[2] >= 0.55 && b.hi[2] <= 0.9,
      );
      return lit.some((b) => mid(b, 0) < 0) && lit.some((b) => mid(b, 0) > 0);
    },
  ],
  "round-table": [
    // v1: no lathe column under the top: no revolved part stands on the
    // floor under the table's middle and reaches up towards the top.
    (ps) =>
      !ps.some(
        (b) =>
          upright(b) &&
          Math.hypot(mid(b, 0), mid(b, 1)) < 0.2 &&
          b.lo[2] < 0.3 &&
          b.hi[2] > 0.5,
      ),
  ],
  "council-chair": [
    // v1: a back wider than the seat (the seat is the accent part).
    (ps) => {
      const seat = ps.find(accented);
      if (!seat) return false;
      return ps.some(
        (b) =>
          b.lo[2] >= seat.hi[2] - EPS && extent(b, 0) >= extent(seat, 0) + 0.1,
      );
    },
    // v2: a lathe shell at least 0.5 m across.
    (ps) =>
      ps.some(
        (b) =>
          b.part.method === "lathe" &&
          extent(b, 0) >= 0.5 &&
          extent(b, 1) >= 0.5,
      ),
  ],
  generator: [
    // v1: a cylinder whose axis is horizontal, a drum at least 1 m long
    // and 0.6 m across.
    (ps) =>
      ps.some(
        (b) =>
          b.part.method === "cylinderAlong" &&
          extent(b, 0) >= 1 &&
          extent(b, 2) >= 0.6,
      ),
    // v2: two cylinders taller than 1.2 m.
    (ps) => ps.filter((b) => upright(b) && extent(b, 2) > 1.2).length >= 2,
  ],
  "pipe-run": [
    // v1: three pipe axes, and (beyond the table's words, which variant 0's
    // three flat pipes meet) bundled, not all at one height, with at least
    // one valve wheel.
    (ps) => {
      const pipes = ps.filter(
        (b) => b.part.method === "cylinderAlong" && extent(b, 0) >= 1,
      );
      const axes = new Set(
        pipes.map((b) => `${mid(b, 1).toFixed(2)},${mid(b, 2).toFixed(2)}`),
      );
      const hs = pipes.map((b) => mid(b, 2));
      return (
        axes.size >= 3 &&
        Math.max(...hs) - Math.min(...hs) > 0.05 &&
        ps.some((b) => b.part.method === "ring")
      );
    },
  ],
  "shelf-row": [
    // v1: at least 24 drawer fronts: flat boxes 0.1 to 0.5 m wide and 0.1
    // to 0.35 m tall, at most 3 cm deep.
    (ps) =>
      ps.filter(
        (b) =>
          b.part.method === "box" &&
          extent(b, 1) <= 0.03 + EPS &&
          extent(b, 0) >= 0.1 &&
          extent(b, 0) <= 0.5 &&
          extent(b, 2) >= 0.1 &&
          extent(b, 2) <= 0.35,
      ).length >= 24,
    // v2: at least four horizontal cylinders (the tube rolls).
    (ps) => ps.filter((b) => b.part.method === "cylinderAlong").length >= 4,
  ],
  "lab-island": [
    // v1: a part taller than 0.35 m standing on the top, wholly past the
    // bench surface's far end (a 0.5, `DECOR_SURFACES`): variant 0's fume
    // hood stands outside the surface too, at the other end, so the far
    // end is what tells them apart.
    (ps) =>
      ps.some(
        (b) => b.lo[0] >= 0.5 && b.lo[2] >= 0.96 - EPS && b.hi[2] - 0.96 > 0.35,
      ),
  ],
  "specimen-tank": [
    // v1, the square tank: no cylinder taller than 0.5 m, and (beyond the
    // table's words, which variant 0 meets, its tall glass being a lathe)
    // no lathe either.
    (ps) => !ps.some((b) => upright(b) && extent(b, 2) > 0.5),
    // v2: three glowing cores, each at least 0.8 m tall on its own axis.
    (ps) => {
      const cores = ps.filter(
        (b) => upright(b) && glows(b) && extent(b, 2) >= 0.8,
      );
      const axes = new Set(
        cores.map((b) => `${mid(b, 0).toFixed(1)},${mid(b, 1).toFixed(1)}`),
      );
      return cores.length >= 3 && axes.size >= 3;
    },
  ],
};

/**
 * How many parts of each decor variant carry the room's accent mark (2.7
 * C9), entry `v` for variant `v`. Variant 0 re-tints parts it already had:
 * the console's three kick strips (it has no trim under its screens), the
 * captain's and the council chair's seat, the round table's rim ring, the
 * generator's ring round its core, the pipe run's brackets (cross bar and
 * two side posts each, five brackets in `HALL`, so this count follows the
 * hall), the shelf row's two end panels, the lab island's sink rim (it has
 * no edge band) and the specimen tank's collar ring (it has no ring on its
 * cap). Variants 1 and 2 carry the parts their recipes' docs name.
 */
const DECOR_ACCENT_PARTS: Record<DecorKind, readonly number[]> = {
  "command-console": [3, 3, 5],
  "captain-chair": [1, 1],
  "round-table": [1, 1],
  "council-chair": [1, 1, 1],
  generator: [1, 1, 2],
  "pipe-run": [15, 2],
  "shelf-row": [2, 2, 2],
  "lab-island": [1, 2],
  "specimen-tank": [1, 1, 3],
};

describe("decor variants (2.7 Task 7)", () => {
  it("raises every decor kind to its count (2.7 C2)", () => {
    // Mutation caught: a count left at 1, so no new variant is ever built.
    expect(VARIANT_COUNTS.decor).toEqual({
      "command-console": 3,
      "captain-chair": 2,
      "round-table": 2,
      "council-chair": 3,
      generator: 3,
      "pipe-run": 2,
      "shelf-row": 3,
      "lab-island": 2,
      "specimen-tank": 3,
    });
  });

  for (const kind of DECOR_KINDS)
    for (const [i, check] of (DECOR_SIGNATURES[kind] ?? []).entries())
      it(`builds ${kind} variant ${String(i + 1)} to its design (2.7 Task 7)`, () => {
        // Mutation caught: the variant built as variant 0 (the check must
        // fail there, so it cannot pass on any build of the kind).
        expect(VARIANT_COUNTS.decor[kind]).toBeGreaterThan(i + 1);
        expect(check(decorParts(kind, i + 1))).toBe(true);
        expect(check(decorParts(kind, 0))).toBe(false);
      });

  it("names a design for every new decor variant", () => {
    // Mutation caught: a count raised with no signature to hold it.
    for (const kind of DECOR_KINDS)
      expect((DECOR_SIGNATURES[kind] ?? []).length, kind).toBe(
        VARIANT_COUNTS.decor[kind] - 1,
      );
  });

  it("paints the room's accent on each variant's trim parts only (2.7 C9)", () => {
    // Mutation caught: an accent part left in its old colour, one painted
    // twice, a glowing part (a screen, a core, a lit panel) in the accent,
    // or a baked colour used instead of the mark.
    expect(Object.keys(DECOR_ACCENT_PARTS).length).toBe(DECOR_KINDS.length);
    for (const kind of DECOR_KINDS) {
      const counts = DECOR_ACCENT_PARTS[kind];
      expect(counts.length, kind).toBe(VARIANT_COUNTS.decor[kind]);
      counts.forEach((n, v) => {
        const painted = decorParts(kind, v).filter(accented);
        expect(painted.length, `${kind} variant ${String(v)}`).toBe(n);
        for (const b of painted) expect(GLOWING).not.toContain(b.part.flag);
      });
    }
  });
});
