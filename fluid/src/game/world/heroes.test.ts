/**
 * The hero catalogue, footprints and reservation: every kind's size,
 * footing, height band and pool, and what each hero blocks and reserves.
 * The pass that places heroes is pinned in its own tests with the goldens.
 */
import { describe, expect, it } from "vitest";

import {
  FOOTPRINTS,
  HERO_FOOTING,
  footprint,
  heroBlocker,
  heroFootprint,
} from "./footprints";
import {
  HERO_CATALOGUE,
  HERO_CLEAR,
  HERO_KINDS,
  HERO_POOLS,
  HERO_USE_OUT,
  HERO_VIEW,
  HERO_WALL_TOP,
  SLAB_TOP,
  heroCap,
  heroEdges,
  heroReserve,
  heroSurfaces,
} from "./heroes";
import heroesSource from "./heroes.ts?raw";
import { edgeKey, wallAnchor } from "./sites";
import type { Archetype, Hero, HeroKind, Side } from "./types";
import { CELL } from "./units";

const SIDES: readonly Side[] = ["n", "e", "s", "w"];
const ARCHETYPES: readonly Archetype[] = [
  "bridge",
  "council",
  "engineering",
  "archive",
  "lab",
];

/** A hero of `kind` on edge `e` (wall and backed kinds), or centred at (4.5, 3). */
function heroAt(kind: HeroKind, variant: number, side: Side = "s"): Hero {
  if (HERO_FOOTING[kind] === "free")
    return { kind, variant, x: 4.5, y: 3, turn: 0, seed: 1 };
  const a = wallAnchor({ x: 3, y: 4, side });
  return { kind, variant, x: a.x, y: a.y, turn: a.turn, seed: 1 };
}

describe("the hero catalogue", () => {
  it("lists every kind once, with a size per variant and a footing that matches its placement", () => {
    expect(new Set(HERO_KINDS).size).toBe(HERO_KINDS.length);
    expect(HERO_KINDS).toHaveLength(16);
    const footing = {
      wall: "flush",
      backed: "backed",
      band: "free",
      corner: "free",
      centre: "free",
    } as const;
    for (const kind of HERO_KINDS) {
      const e = HERO_CATALOGUE[kind];
      expect(FOOTPRINTS.hero[kind], kind).toHaveLength(e.variants);
      expect(HERO_FOOTING[kind], kind).toBe(footing[e.placement]);
      expect(e.edges === 2, kind).toBe(kind === "core-wall");
    }
  });

  it("keeps every hero under its height band", () => {
    for (const kind of HERO_KINDS) {
      const e = HERO_CATALOGUE[kind];
      if (kind === "black-slab") expect(e.top).toBe(SLAB_TOP);
      else if (e.placement === "wall")
        expect(e.top, kind).toBeLessThanOrEqual(HERO_WALL_TOP);
      else expect(e.top, kind).toBeLessThanOrEqual(2.2);
    }
    expect(SLAB_TOP).toBeLessThanOrEqual(3.0 - 0.05);
    const slab = FOOTPRINTS.hero["black-slab"][0];
    if (slab === undefined) throw new Error("no slab size");
    expect(slab.width / slab.depth).toBeCloseTo(4);
    expect(SLAB_TOP / slab.depth).toBeCloseTo(9);
  });

  it("keeps flush heroes within the wall band and inside their edges", () => {
    for (const kind of HERO_KINDS) {
      if (HERO_FOOTING[kind] !== "flush") continue;
      for (const size of FOOTPRINTS.hero[kind]) {
        expect(size.depth, kind).toBeLessThanOrEqual(0.3);
        // WALL_REACH's rule: 0.2 m clear of whatever hangs on the next edge.
        expect(size.width, kind).toBeLessThanOrEqual(
          HERO_CATALOGUE[kind].edges * CELL - 0.2,
        );
      }
    }
  });

  it("draws pools from each archetype's own kinds, never the turret or the slab", () => {
    const pooled = new Set<HeroKind>();
    for (const a of ARCHETYPES) {
      const kinds = HERO_POOLS[a].map(([k]) => k);
      expect(new Set(kinds).size, a).toBe(kinds.length);
      expect(kinds, a).toContain("arcade-cabinet");
      expect(kinds, a).toContain("recruit-cabinet");
      for (const [k, w] of HERO_POOLS[a]) {
        expect(w, `${a} ${k}`).toBeGreaterThan(0);
        pooled.add(k);
      }
    }
    expect(pooled.has("turret")).toBe(false);
    expect(pooled.has("black-slab")).toBe(false);
    expect([...pooled].sort()).toEqual(
      HERO_KINDS.filter((k) => k !== "turret" && k !== "black-slab").sort(),
    );
  });

  it("keeps a use point only on the cabinets, in front of the box and inside the reserve", () => {
    // The reserve is the box grown by HERO_CLEAR, so a point HERO_USE_OUT in
    // front of the face lies inside it; checked against heroReserve below.
    for (const kind of HERO_KINDS) {
      const use = HERO_CATALOGUE[kind].use;
      if (kind !== "arcade-cabinet" && kind !== "recruit-cabinet") {
        expect(use, kind).toBeNull();
        continue;
      }
      if (use === null) throw new Error(kind);
      const h = heroAt(kind, 0, "s");
      const box = heroFootprint(h);
      // At turn 0 (a south wall) the local d runs north, towards smaller z.
      const x = h.x * CELL + use.a;
      const z = h.y * CELL - use.d;
      expect(x, kind).toBeGreaterThan(box.x0);
      expect(x, kind).toBeLessThan(box.x1);
      expect(box.z0 - z, kind).toBeCloseTo(HERO_USE_OUT);
      expect(HERO_USE_OUT).toBeGreaterThan(0.35);
      expect(HERO_USE_OUT).toBeLessThan(HERO_CLEAR);
      const reserve = heroReserve([h]).boxes[0];
      if (reserve === undefined) throw new Error(kind);
      expect(x, kind).toBeGreaterThan(reserve.x0);
      expect(x, kind).toBeLessThan(reserve.x1);
      expect(z, kind).toBeGreaterThan(reserve.z0);
      expect(z, kind).toBeLessThan(reserve.z1);
    }
  });

  it("allows two heroes only in halls of 16 by 16 cells or more", () => {
    expect(heroCap({ x0: 0, y0: 0, x1: 16, y1: 16 })).toBe(2);
    expect(heroCap({ x0: 3, y0: 0, x1: 27, y1: 24 })).toBe(2);
    expect(heroCap({ x0: 0, y0: 0, x1: 16, y1: 15 })).toBe(1);
    expect(heroCap({ x0: 0, y0: 0, x1: 13, y1: 12 })).toBe(1);
  });

  it("keeps heroes.ts away from move, generate, interact, malfunction and render", () => {
    expect(heroesSource).not.toMatch(
      /\b(?:from|import)\s*\(?\s*["'](?:\.\/(?:move|generate|interact|malfunction)|\.\.\/render(?:\/[^"']*)?)["']/,
    );
  });
});

describe("hero footprints", () => {
  it("puts a wall-anchored hero's box in front of its wall, like a fixture's footprint", () => {
    for (const kind of HERO_KINDS) {
      if (HERO_FOOTING[kind] === "free" || HERO_CATALOGUE[kind].edges !== 1)
        continue;
      for (const side of SIDES) {
        const h = heroAt(kind, 0, side);
        const size = FOOTPRINTS.hero[kind][0];
        if (size === undefined) throw new Error(kind);
        expect(heroFootprint(h), `${kind} ${side}`).toEqual(
          footprint(
            { x: 3, y: 4, side },
            { along: size.width, out: size.depth },
          ),
        );
        expect(heroEdges(h).map(edgeKey), `${kind} ${side}`).toEqual([
          edgeKey({ x: 3, y: 4, side }),
        ]);
      }
    }
  });

  it("spans a two-edge wall hero over both edges, anchored where they meet", () => {
    for (const side of SIDES) {
      const along = side === "n" || side === "s";
      const e0 = { x: 3, y: 4, side };
      const e1 = along ? { x: 4, y: 4, side } : { x: 3, y: 5, side };
      const a0 = wallAnchor(e0);
      const a1 = wallAnchor(e1);
      const h: Hero = {
        kind: "core-wall",
        variant: 0,
        x: (a0.x + a1.x) / 2,
        y: (a0.y + a1.y) / 2,
        turn: a0.turn,
        seed: 1,
      };
      expect(heroEdges(h).map(edgeKey).sort()).toEqual(
        [edgeKey(e0), edgeKey(e1)].sort(),
      );
      // Its box covers both edges' wall stretch less 0.2 m at each end, and
      // stands out from the wall as deep as a one-edge box would.
      const size = FOOTPRINTS.hero["core-wall"][0];
      if (size === undefined) throw new Error("no core wall size");
      const wall = { along: CELL, out: size.depth };
      const f0 = footprint(e0, wall);
      const f1 = footprint(e1, wall);
      const edges = {
        x0: Math.min(f0.x0, f1.x0),
        x1: Math.max(f0.x1, f1.x1),
        z0: Math.min(f0.z0, f1.z0),
        z1: Math.max(f0.z1, f1.z1),
      };
      const want = along
        ? { ...edges, x0: edges.x0 + 0.2, x1: edges.x1 - 0.2 }
        : { ...edges, z0: edges.z0 + 0.2, z1: edges.z1 - 0.2 };
      const box = heroFootprint(h);
      for (const k of ["x0", "x1", "z0", "z1"] as const)
        expect(box[k], `${side} ${k}`).toBeCloseTo(want[k], 9);
    }
  });

  it("centres a free hero's box on its anchor and swaps its sides at a quarter turn", () => {
    const h: Hero = {
      kind: "mess-table",
      variant: 0,
      x: 4.5,
      y: 3,
      turn: 1,
      seed: 1,
    };
    const b = heroFootprint(h);
    expect(b.x1 - b.x0).toBeCloseTo(2.6);
    expect(b.z1 - b.z0).toBeCloseTo(5.0);
    expect((b.x0 + b.x1) / 2).toBeCloseTo(4.5 * CELL);
    expect((b.z0 + b.z1) / 2).toBeCloseTo(3 * CELL);
  });

  it("blocks with every hero but the flush ones", () => {
    for (const kind of HERO_KINDS) {
      const h = heroAt(kind, 0);
      if (HERO_FOOTING[kind] === "flush")
        expect(heroBlocker(h), kind).toBeNull();
      else expect(heroBlocker(h), kind).toEqual(heroFootprint(h));
    }
  });

  it("throws on a variant the kind does not have", () => {
    expect(() =>
      heroFootprint({
        kind: "turret",
        variant: 1,
        x: 1,
        y: 1,
        turn: 0,
        seed: 0,
      }),
    ).toThrow(/no variant 1/);
  });
});

describe("what a hero reserves (H19)", () => {
  it("reserves a free or backed hero's box grown by HERO_CLEAR, and a flush hero's view box", () => {
    for (const kind of HERO_KINDS) {
      const h = heroAt(kind, 0);
      const box = heroFootprint(h);
      const r = heroReserve([h]);
      expect(r.boxes, kind).toHaveLength(1);
      const got = r.boxes[0];
      if (got === undefined) throw new Error(kind);
      if (HERO_FOOTING[kind] === "flush") {
        // At turn 0 (a south wall) the view box runs HERO_VIEW north of the wall point.
        expect(got.z1, kind).toBeCloseTo(h.y * CELL);
        expect(got.z1 - got.z0, kind).toBeCloseTo(HERO_VIEW);
        expect([got.x0, got.x1], kind).toEqual([box.x0, box.x1]);
      } else {
        expect(got, kind).toEqual({
          x0: box.x0 - HERO_CLEAR,
          x1: box.x1 + HERO_CLEAR,
          z0: box.z0 - HERO_CLEAR,
          z1: box.z1 + HERO_CLEAR,
        });
      }
      const wantEdges =
        HERO_FOOTING[kind] === "free" ? [] : heroEdges(h).map(edgeKey);
      expect([...r.edges].sort(), kind).toEqual(wantEdges.sort());
    }
  });

  it("reserves a one-edge flush hero's view box HERO_VIEW deep off its edge, on every side", () => {
    for (const kind of HERO_KINDS) {
      if (HERO_FOOTING[kind] !== "flush" || HERO_CATALOGUE[kind].edges !== 1)
        continue;
      const size = FOOTPRINTS.hero[kind][0];
      if (size === undefined) throw new Error(kind);
      for (const side of SIDES) {
        const got = heroReserve([heroAt(kind, 0, side)]).boxes;
        expect(got, `${kind} ${side}`).toEqual([
          footprint(
            { x: 3, y: 4, side },
            { along: size.width, out: HERO_VIEW },
          ),
        ]);
      }
    }
  });

  it("gives every surface inside its hero's box, under its top, at every turn", () => {
    for (const kind of HERO_KINDS) {
      const heroes =
        HERO_FOOTING[kind] === "free"
          ? [0, 1, 2, 3].map((turn) => ({ ...heroAt(kind, 0), turn }))
          : SIDES.map((side) => heroAt(kind, 0, side));
      for (const h of heroes) {
        const box = heroFootprint(h);
        const tops = heroSurfaces(h);
        expect(tops, kind).toHaveLength(HERO_CATALOGUE[kind].surfaces.length);
        for (const s of tops) {
          expect(s.box.x0, kind).toBeGreaterThanOrEqual(box.x0 - 1e-9);
          expect(s.box.x1, kind).toBeLessThanOrEqual(box.x1 + 1e-9);
          expect(s.box.z0, kind).toBeGreaterThanOrEqual(box.z0 - 1e-9);
          expect(s.box.z1, kind).toBeLessThanOrEqual(box.z1 + 1e-9);
          expect(s.h, kind).toBeLessThan(HERO_CATALOGUE[kind].top);
        }
      }
    }
  });
});
