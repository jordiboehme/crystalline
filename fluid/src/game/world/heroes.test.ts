/**
 * The hero catalogue, footprints and reservation: every kind's size,
 * footing, height band and pool, and what each hero blocks and reserves.
 * The pass that places heroes (`heroDraws`, `placeHeroes`) is held to its
 * rates, its rules and the flood fill in "the hero pass" below.
 */
import { describe, expect, it } from "vitest";

import { createRng, seedFor } from "../core/seed";
import { roomWithForcedHero } from "../dev/demo";
import { frameAt, turnPoint } from "../render/kit";
import { CANNED_BRIDGE, CANNED_HUB, CANNED_WORKSHOP } from "./canned";
import { dressCandidates, dressRoom } from "./dress";
import {
  FOOTPRINTS,
  HERO_FOOTING,
  HERO_LIFT,
  HERO_WALK_UNDER,
  footprint,
  heroBlocker,
  heroFootprint,
  heroLift,
  pipeRunBox,
  propFootprint,
} from "./footprints";
import { generateRoom } from "./generate";
import {
  ANY_POOL,
  ANY_SHARE,
  BLOCK_SHARE,
  HERO_CATALOGUE,
  HERO_CEILING_GAP,
  HERO_CLEAR,
  HERO_KINDS,
  HERO_MIN_CEILING,
  HERO_POOLS,
  HERO_SHARE,
  HERO_USE_OUT,
  HERO_VIEW,
  HERO_WALL_TOP,
  LOWEST_CEILING,
  SLAB_SHARE,
  SLAB_TOP,
  TURRET_SHARE,
  heroCap,
  heroDraws,
  heroEdges,
  heroMinCeiling,
  heroPoint,
  heroReserve,
  heroSurfaces,
  heroUsePoint,
  isTallHero,
  placeHeroes,
  type HeroDraws,
} from "./heroes";
import heroesSource from "./heroes.ts?raw";
import { PLAYER_RADIUS, blockersFor } from "./move";
import {
  DEGENERATE_PLACES,
  OVER_CAP,
  arrivalPoint,
  distanceTo,
  reachedTargets,
} from "./reachChecks";
import {
  dressingSites,
  edgeKey,
  edgeOf,
  fitsFloor,
  grow,
  interiorBand,
  overlaps,
  wallAnchor,
  type DressingSites,
  type RoomBase,
  type SiteBase,
} from "./sites";
import type {
  Archetype,
  Hero,
  HeroKind,
  PlaceInput,
  RoomSpec,
  Side,
} from "./types";
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
    expect(HERO_KINDS).toHaveLength(28);
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

  it("keeps every hero under its lowest ceiling less the gap, and names the tall ones", () => {
    // Mutation caught: a tall kind added without a ceiling entry, or the
    // robot's entry dropped, or a wall hero over the wall band.
    for (const kind of HERO_KINDS) {
      const e = HERO_CATALOGUE[kind];
      expect(e.top + HERO_CEILING_GAP, kind).toBeLessThanOrEqual(
        heroMinCeiling(kind) + 1e-9,
      );
      if (e.placement === "wall")
        expect(e.top, kind).toBeLessThanOrEqual(HERO_WALL_TOP);
    }
    expect(HERO_KINDS.filter(isTallHero).sort()).toEqual([
      "black-slab",
      "garden-robot",
      "moon-rocket",
      "police-box",
      "question-block",
      "spider-tank",
    ]);
    expect(HERO_MIN_CEILING).toEqual({ "garden-robot": 3.7 });
    expect(generateRoom({ ...CANNED_WORKSHOP, salience: 0 }).ceiling).toBe(
      LOWEST_CEILING,
    );
    expect(HERO_CATALOGUE["black-slab"].top).toBe(SLAB_TOP);
    const slab = FOOTPRINTS.hero["black-slab"][0];
    if (slab === undefined) throw new Error("no slab size");
    expect(slab.width / slab.depth).toBeCloseTo(4);
    expect(SLAB_TOP / slab.depth).toBeCloseTo(9);
  });

  it("stands a tall hero only in the band, at the centre or backed against a wall (C6)", () => {
    // Mutation caught: the police box made a corner hero, where ceiling
    // props hang through its roof.
    for (const kind of HERO_KINDS.filter(isTallHero))
      expect(["band", "centre", "backed"], kind).toContain(
        HERO_CATALOGUE[kind].placement,
      );
  });

  it("names exactly three floating heroes and their lifts (C4)", () => {
    expect(HERO_LIFT).toEqual({
      "question-block": 2.3,
      hoverboard: 0.25,
      "flying-cloud": 0.4,
    });
    for (const kind of HERO_KINDS)
      expect(heroLift(kind), kind).toBe(
        (HERO_LIFT as Partial<Record<HeroKind, number>>)[kind] ?? 0,
      );
    // Only the block floats over a walking player's head.
    expect(HERO_KINDS.filter((k) => heroLift(k) >= HERO_WALK_UNDER)).toEqual([
      "question-block",
    ]);
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

  it("draws pools from each archetype's own kinds; the slab, turret, block and any-archetype kinds apart", () => {
    const apart = new Set<HeroKind>([
      "turret",
      "black-slab",
      "question-block",
      ...ANY_POOL.map(([k]) => k),
    ]);
    const pooled = new Set<HeroKind>();
    for (const a of ARCHETYPES) {
      const kinds = HERO_POOLS[a].map(([k]) => k);
      expect(new Set(kinds).size, a).toBe(kinds.length);
      expect(kinds, a).toContain("arcade-cabinet");
      expect(kinds, a).toContain("recruit-cabinet");
      // Catalogue order is part of the seeded result.
      expect(kinds, a).toEqual(
        [...kinds].sort(
          (x, y) => HERO_KINDS.indexOf(x) - HERO_KINDS.indexOf(y),
        ),
      );
      for (const [k, w] of HERO_POOLS[a]) {
        expect(w, `${a} ${k}`).toBeGreaterThan(0);
        expect(apart.has(k), `${a} ${k}`).toBe(false);
        pooled.add(k);
      }
    }
    expect([...pooled, ...apart].sort()).toEqual([...HERO_KINDS].sort());
    expect(ANY_POOL.map(([k]) => k)).toEqual([
      "hoverboard",
      "flying-cloud",
      "moon-rocket",
      "thunder-hammer",
      "police-box",
    ]);
    // 2.6d C9, C17: the slab walker appended at the end of the engineering
    // pool, after the spider tank.
    expect(HERO_POOLS.engineering.slice(-4)).toEqual([
      ["mech-head", 2],
      ["red-bike", 2],
      ["spider-tank", 2],
      ["slab-walker", 2],
    ]);
    expect(HERO_POOLS.archive.at(-1)).toEqual(["stone-hand", 2]);
    expect(HERO_POOLS.lab.slice(-2)).toEqual([
      ["stone-hand", 2],
      ["garden-robot", 2],
    ]);
  });

  it("draws the slab walker from the bridge and engineering pools at weight 2 (2.6d C9, 2.6f C3)", () => {
    // Mutation caught: the walker missing from a pool, or put in another
    // archetype's.
    for (const [a, pool] of Object.entries(HERO_POOLS)) {
      const w = pool.find(([k]) => k === "slab-walker")?.[1] ?? 0;
      expect(w, a).toBe(a === "bridge" || a === "engineering" ? 2 : 0);
    }
    expect(HERO_CATALOGUE["slab-walker"]).toMatchObject({
      placement: "band",
      top: 1.8,
    });
    expect(isTallHero("slab-walker")).toBe(false);
  });

  it("keeps a use point only where a later spec needs one, inside the reserve", () => {
    // In front of the face for the cabinets, the hammer and the police box;
    // under the middle for the block (C15). Mutation caught: a use point
    // dropped, moved into the box, or added to another kind.
    const FRONT = [
      "arcade-cabinet",
      "recruit-cabinet",
      "thunder-hammer",
      "police-box",
    ];
    for (const kind of HERO_KINDS) {
      const use = HERO_CATALOGUE[kind].use;
      if (!FRONT.includes(kind) && kind !== "question-block") {
        expect(use, kind).toBeNull();
        continue;
      }
      if (use === null) throw new Error(kind);
      const h = heroAt(kind, 0, "s");
      const box = heroFootprint(h);
      const p = heroUsePoint(h);
      if (p === null) throw new Error(kind);
      if (kind === "question-block") {
        expect(p.x).toBeCloseTo((box.x0 + box.x1) / 2);
        expect(p.z).toBeCloseTo((box.z0 + box.z1) / 2);
      } else {
        // At turn 0 the local d runs north, towards smaller z.
        expect(p.x, kind).toBeGreaterThan(box.x0);
        expect(p.x, kind).toBeLessThan(box.x1);
        expect(box.z0 - p.z, kind).toBeCloseTo(HERO_USE_OUT);
      }
      const reserve = heroReserve([h]).boxes[0];
      if (reserve === undefined) throw new Error(kind);
      expect(p.x > reserve.x0 && p.x < reserve.x1, kind).toBe(true);
      expect(p.z > reserve.z0 && p.z < reserve.z1, kind).toBe(true);
    }
    expect(HERO_USE_OUT).toBeGreaterThan(0.35);
    expect(HERO_USE_OUT).toBeLessThan(HERO_CLEAR);
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

/** A point in the kit's terms: x, height, z. */
type V3 = [number, number, number];

describe("hero local terms against the kit's frame", () => {
  /**
   * Where the kit puts local point `(a, d)` of a hero: `a` along the
   * frame's `along` and `d` along its `inward`, at the hero's anchor and
   * turn, worked out twice, by `frameAt` and by `turnPoint` of the point
   * as built at turn 0 (the way instancing places a mesh).
   */
  function kitPoints(h: Hero, a: number, d: number) {
    const origin: V3 = [h.x * CELL, 0, h.y * CELL];
    const f = frameAt(origin, h.turn);
    const byFrame = [
      origin[0] + f.along[0] * a + f.inward[0] * d,
      origin[2] + f.along[2] * a + f.inward[2] * d,
    ];
    const base = frameAt([0, 0, 0], 0);
    const local: V3 = [
      base.along[0] * a + base.inward[0] * d,
      0,
      base.along[2] * a + base.inward[2] * d,
    ];
    const turned = turnPoint(local, h.turn);
    const byTurn = [origin[0] + turned[0], origin[2] + turned[2]];
    return [byFrame, byTurn];
  }

  /** A hero of `kind` at each of the four turns: on each wall, or turned in place. */
  const atEveryTurn = (kind: HeroKind): Hero[] =>
    HERO_FOOTING[kind] === "free"
      ? [0, 1, 2, 3].map((turn) => ({ ...heroAt(kind, 0), turn }))
      : SIDES.map((side) => heroAt(kind, 0, side));

  it("puts every surface where the kit's frame puts the catalogue's local top, at every turn", () => {
    // The laser desk, mess table and both benches have tops that are not
    // centred on their `a` axis, so a mirrored along axis shows.
    let checked = 0;
    for (const kind of HERO_KINDS)
      for (const h of atEveryTurn(kind)) {
        const specs = HERO_CATALOGUE[kind].surfaces;
        const got = heroSurfaces(h);
        for (const [i, s] of specs.entries()) {
          for (const pts of [0, 1]) {
            const xs: number[] = [];
            const zs: number[] = [];
            for (const a of [s.a0, s.a1])
              for (const d of [s.d0, s.d1]) {
                const p = kitPoints(h, a, d)[pts];
                if (p === undefined) throw new Error("no point");
                xs.push(p[0] ?? NaN);
                zs.push(p[1] ?? NaN);
              }
            const box = got[i]?.box;
            if (box === undefined)
              throw new Error(`${kind} surface ${String(i)}`);
            const label = `${kind} turn ${String(h.turn)} surface ${String(i)}`;
            expect(box.x0, label).toBeCloseTo(Math.min(...xs), 9);
            expect(box.x1, label).toBeCloseTo(Math.max(...xs), 9);
            expect(box.z0, label).toBeCloseTo(Math.min(...zs), 9);
            expect(box.z1, label).toBeCloseTo(Math.max(...zs), 9);
          }
          checked++;
        }
      }
    // Four kinds with one top each, at four turns.
    expect(checked).toBe(16);
  });

  it("puts every use point where the kit's frame puts the catalogue's local one, at every turn", () => {
    let checked = 0;
    for (const kind of HERO_KINDS) {
      const use = HERO_CATALOGUE[kind].use;
      if (use === null) continue;
      for (const h of atEveryTurn(kind)) {
        const p = heroUsePoint(h);
        if (p === null) throw new Error(kind);
        for (const q of kitPoints(h, use.a, use.d)) {
          expect(p.x, `${kind} ${String(h.turn)}`).toBeCloseTo(q[0] ?? NaN, 9);
          expect(p.z, `${kind} ${String(h.turn)}`).toBeCloseTo(q[1] ?? NaN, 9);
        }
        // An off-centre point at the same depth, so a mirrored axis shows
        // even though every cabinet's own point is on its centre line.
        const moved = heroPoint(h, 0.3, use.d);
        const [q] = kitPoints(h, 0.3, use.d);
        expect(moved.x).toBeCloseTo(q?.[0] ?? NaN, 9);
        expect(moved.z).toBeCloseTo(q?.[1] ?? NaN, 9);
        checked++;
      }
    }
    // Five kinds with a use point (the two cabinets, the block, the
    // hammer and the police box), at four turns.
    expect(checked).toBe(20);
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

  it("blocks with every hero but the flush ones and the block over the player's head", () => {
    for (const kind of HERO_KINDS) {
      const h = heroAt(kind, 0);
      const blocks = heroBlocker(h) !== null;
      expect(blocks, kind).toBe(
        HERO_FOOTING[kind] !== "flush" && kind !== "question-block",
      );
      if (blocks) expect(heroBlocker(h)).toEqual(heroFootprint(h));
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

const TYPES = {
  bridge: "manifest",
  council: "decision",
  engineering: "runbook",
  archive: "reference",
  lab: "guide",
} as const satisfies Record<Archetype, string>;

/** A room less its props and heroes: what the hero pass and the sites read. */
function siteBase(room: RoomSpec): SiteBase {
  const base: SiteBase & Partial<Pick<RoomSpec, "props" | "heroes">> = {
    ...room,
  };
  delete base.props;
  delete base.heroes;
  return base;
}

/** A generated room's site base: everything but its props and heroes. */
function baseOf(place: PlaceInput, type: string, status = "stable"): SiteBase {
  return siteBase(generateRoom({ ...place, type, status }));
}

interface Layout {
  archetype: Archetype;
  base: SiteBase;
}

const layouts = (place: PlaceInput): Layout[] =>
  Object.entries(TYPES).map(([a, t]) => ({
    archetype: a as Archetype,
    base: baseOf(place, t),
  }));

const WORKSHOP_BASES = layouts(CANNED_WORKSHOP);
const HUB_BASES = layouts(CANNED_HUB);
const BRIDGE_BASES = layouts(CANNED_BRIDGE);
const EVERY_BASE = [...WORKSHOP_BASES, ...HUB_BASES, ...BRIDGE_BASES];

/**
 * The same layout under many room seeds: only the draws change. The pass
 * takes the layout's sites precomputed (`sitesOf`), so a reseed costs the
 * draws and the candidates, not the sites.
 */
const reseeded = (base: SiteBase, n = 1000): SiteBase[] =>
  Array.from({ length: n }, (_, i) => ({
    ...base,
    seed: seedFor("hero-rate", base.permalink, base.archetype, i),
  }));

/** A room with these heroes, dressed around them. */
function withHeroes(base: SiteBase, heroes: Hero[]): RoomSpec {
  const room: RoomSpec = { ...base, heroes, props: [], curios: [] };
  return { ...room, props: dressRoom(room) };
}

const SITES = new Map<SiteBase, DressingSites>();
/** The sites of a layout, worked out once (`dressingSites` does not read the seed). */
const sitesOf = (base: SiteBase) => {
  const known = SITES.get(base);
  if (known !== undefined) return known;
  const sites = dressingSites(base);
  SITES.set(base, sites);
  return sites;
};

/** The heroes of reseed `r` of layout `base`, on the layout's own sites. */
const place = (base: SiteBase, r: SiteBase, draws: HeroDraws = heroDraws(r)) =>
  placeHeroes(r, draws, sitesOf(base));

/** The binomial's mean less and plus four standard deviations. */
const binomial = (n: number, p: number) => {
  const sd = Math.sqrt(n * p * (1 - p));
  return [n * p - 4 * sd, n * p + 4 * sd] as const;
};

/**
 * Every rule a placed hero keeps (H8 to H10, H20): out of every lane; a
 * blocking hero on the hall's floor with its moat clear of every taken box
 * and every other hero; its box's centre inside the hall; a band hero
 * inside the interior band; at most `heroCap` heroes, no two of a kind.
 */
function expectHeroInvariants(
  name: string,
  base: SiteBase,
  heroes: readonly Hero[],
  sites: DressingSites = dressingSites(base),
) {
  expect(heroes.length, name).toBeLessThanOrEqual(heroCap(base.hall));
  expect(new Set(heroes.map((h) => h.kind)).size, name).toBe(heroes.length);
  for (const h of heroes) {
    const label = `${name} ${h.kind}`;
    const box = heroFootprint(h);
    const moat = {
      x0: box.x0 - HERO_CLEAR,
      x1: box.x1 + HERO_CLEAR,
      z0: box.z0 - HERO_CLEAR,
      z1: box.z1 + HERO_CLEAR,
    };
    expect(
      sites.lanes.some((l) => overlaps(box, l)),
      label,
    ).toBe(false);
    if (HERO_FOOTING[h.kind] !== "flush") {
      expect(fitsFloor(base, box), label).toBe(true);
      expect(
        sites.taken.some((t) => overlaps(moat, t)),
        label,
      ).toBe(false);
      for (const o of heroes)
        if (o !== h)
          expect(overlaps(moat, heroFootprint(o)), `${label} ${o.kind}`).toBe(
            false,
          );
    }
    const cx = (box.x0 + box.x1) / 2 / CELL;
    const cy = (box.z0 + box.z1) / 2 / CELL;
    expect(
      cx >= base.hall.x0 &&
        cx <= base.hall.x1 &&
        cy >= base.hall.y0 &&
        cy <= base.hall.y1,
      label,
    ).toBe(true);
    if (HERO_CATALOGUE[h.kind].placement === "band") {
      const band = interiorBand(base.hall);
      if (band === null)
        throw new Error(`${label}: a band hero in a hall with no band`);
      expect(
        box.x0 >= band.x0 * CELL - 1e-9 &&
          box.x1 <= band.x1 * CELL + 1e-9 &&
          box.z0 >= band.y0 * CELL - 1e-9 &&
          box.z1 <= band.y1 * CELL + 1e-9,
        label,
      ).toBe(true);
    }
  }
}

describe("the hero pass", () => {
  /** The roll at the middle of `kind`'s weight in `pool`. */
  const middleRoll = (
    pool: readonly (readonly [HeroKind, number])[],
    kind: HeroKind,
  ): number => {
    const i = pool.findIndex(([k]) => k === kind);
    const w = pool[i]?.[1];
    if (w === undefined) throw new Error(`${kind} is not in the pool`);
    const before = pool.slice(0, i).reduce((s, [, x]) => s + x, 0);
    const total = pool.reduce((s, [, x]) => s + x, 0);
    return (before + w / 2) / total;
  };
  /** The any-archetype draw's roll that picks `kind` out of the full `ANY_POOL`. */
  const forcedAnyRoll = (kind: HeroKind) => middleRoll(ANY_POOL, kind);
  /** A pool slot's roll that picks `kind` out of `archetype`'s full pool. */
  const forcedPoolRoll = (archetype: Archetype, kind: HeroKind) =>
    middleRoll(HERO_POOLS[archetype], kind);

  it("holds a hero in about nine halls in ten (2.6f C2)", () => {
    // Measured by the planner: 1821 of 2000 (0.910) at 3/5 and 3/4; 1577
    // (0.788) at the old 1/2 and 1/2. Mutation caught: ANY_SHARE back to
    // 1/2 (about 0.85), both shares back (0.788), or the any draw not
    // filling empty slots. The shares are pinned too, since HERO_SHARE
    // alone back at 1/2 stays inside the band (about 0.905).
    expect(HERO_SHARE).toBe(3 / 5);
    expect(ANY_SHARE).toBe(3 / 4);
    const bases = [...WORKSHOP_BASES, ...BRIDGE_BASES];
    expect(bases).toHaveLength(10);
    let halls = 0;
    let held = 0;
    for (const { base } of bases)
      for (const r of reseeded(base, 200)) {
        halls++;
        if (place(base, r).length > 0) held++;
      }
    expect(halls).toBe(2000);
    expect(held).toBeGreaterThanOrEqual(1760);
    expect(held).toBeLessThanOrEqual(1880);
  }, 30_000);

  it("doubles the archetype-only heroes' weights and leaves every other weight (2.6f C3)", () => {
    // The whole pools, as C3 states them. Mutation caught: a weight left
    // at 1, a cabinet or a 2.6a member doubled, or an entry moved (order
    // is part of the seeded result).
    expect(HERO_POOLS).toEqual({
      bridge: [
        ["eye-panel", 4],
        ["photo-console", 4],
        ["laser-desk", 1],
        ["arcade-cabinet", 1],
        ["recruit-cabinet", 1],
        ["slab-walker", 2],
      ],
      council: [
        ["mess-table", 6],
        ["arcade-cabinet", 1],
        ["recruit-cabinet", 1],
      ],
      engineering: [
        ["helper-robot", 3],
        ["sleep-ring", 3],
        ["gun-rack", 2],
        ["gun-bench", 1],
        ["tube-bench", 2],
        ["field-pack", 1],
        ["arcade-cabinet", 1],
        ["recruit-cabinet", 1],
        ["mech-head", 2],
        ["red-bike", 2],
        ["spider-tank", 2],
        ["slab-walker", 2],
      ],
      archive: [
        ["core-wall", 6],
        ["arcade-cabinet", 1],
        ["recruit-cabinet", 1],
        ["stone-hand", 2],
      ],
      lab: [
        ["photo-console", 3],
        ["laser-desk", 2],
        ["dome-planters", 3],
        ["tube-bench", 2],
        ["field-pack", 1],
        ["arcade-cabinet", 1],
        ["recruit-cabinet", 1],
        ["stone-hand", 2],
        ["garden-robot", 2],
      ],
    });
  });

  it("places at most heroCap heroes, two only in halls of 16 by 16 or more", () => {
    let two = 0;
    for (const { base } of EVERY_BASE)
      for (const r of reseeded(base, 200)) {
        const heroes = place(base, r);
        expect(heroes.length).toBeLessThanOrEqual(heroCap(r.hall));
        expect(new Set(heroes.map((h) => h.kind)).size).toBe(heroes.length);
        if (heroes.length === 2) two++;
      }
    expect(two).toBeGreaterThan(0);
  }, 30_000);

  it("hangs a turret in about 1 room in 6", () => {
    const rooms = WORKSHOP_BASES.flatMap(({ base }) => {
      // Every workshop layout can take a turret: a forced one stands. If one
      // archetype's layout cannot, report it; do not weaken this assert.
      expect(
        placeHeroes(base, { slab: false, turret: true, picks: [] }).map(
          (h) => h.kind,
        ),
      ).toEqual(["turret"]);
      return reseeded(base, 200).map((r) => ({ base, r }));
    });
    const n = rooms.filter(({ base, r }) =>
      place(base, r).some((h) => h.kind === "turret"),
    ).length;
    const [lo, hi] = binomial(rooms.length, TURRET_SHARE);
    expect(n).toBeGreaterThanOrEqual(lo);
    expect(n).toBeLessThanOrEqual(hi);
  }, 30_000);

  it("stands the slab in about 1 room in 40 where the centre is free, and never where it is not", () => {
    const forced = (base: SiteBase) =>
      placeHeroes(base, { slab: true, turret: false, picks: [] }).length === 1;
    const eligible = EVERY_BASE.filter(({ base }) => forced(base));
    // The planner's probe: the canned bridge place in every archetype, the hub's bridge and archive.
    expect(
      eligible
        .map(({ base, archetype }) => `${base.permalink} ${archetype}`)
        .sort(),
    ).toEqual(
      [
        ...BRIDGE_BASES.map(
          ({ base, archetype }) => `${base.permalink} ${archetype}`,
        ),
        `${CANNED_HUB.permalink} archive`,
        `${CANNED_HUB.permalink} bridge`,
      ].sort(),
    );
    const rooms = eligible.flatMap(({ base }) =>
      reseeded(base, Math.ceil(1000 / eligible.length)).map((r) => ({
        base,
        r,
      })),
    );
    const n = rooms.filter(({ base, r }) =>
      place(base, r).some((h) => h.kind === "black-slab"),
    ).length;
    const [lo, hi] = binomial(rooms.length, SLAB_SHARE);
    expect(n).toBeGreaterThanOrEqual(Math.max(1, lo));
    expect(n).toBeLessThanOrEqual(hi);
    for (const { base } of EVERY_BASE.filter((b) => !eligible.includes(b)))
      for (const r of reseeded(base, 200))
        expect(place(base, r).some((h) => h.kind === "black-slab")).toBe(false);
  }, 30_000);

  it("draws pool heroes from the archetype's pool only, at about HERO_SHARE per slot", () => {
    for (const { archetype, base } of WORKSHOP_BASES) {
      // Besides the pool: the slab, the turret, the block and the
      // any-archetype kinds, each drawn apart.
      const pool = new Set<HeroKind>([
        ...HERO_POOLS[archetype].map(([k]) => k),
        "turret",
        "black-slab",
        "question-block",
        ...ANY_POOL.map(([k]) => k),
      ]);
      for (const r of reseeded(base, 200))
        for (const h of place(base, r))
          expect(pool.has(h.kind), `${archetype} ${h.kind}`).toBe(true);
    }
    const first = WORKSHOP_BASES[0];
    if (first === undefined) throw new Error("no workshop base");
    const draws = reseeded(first.base, 1000).map((r) => heroDraws(r));
    const [lo, hi] = binomial(1000, HERO_SHARE);
    const taken = draws.filter((d) => d.picks[0]?.take === true).length;
    expect(taken).toBeGreaterThanOrEqual(lo);
    expect(taken).toBeLessThanOrEqual(hi);
  }, 30_000);

  it("draws the block in about 1 hall in 20 where the slab and turret left a slot", () => {
    // Mutation caught: BLOCK_SHARE wrong, the block drawn on another
    // stream, or tried after the pools. The spec's rate is pinned here
    // too, since the binomial below reads the constant it checks.
    expect(BLOCK_SHARE).toBe(1 / 20);
    let eligible = 0;
    let blocks = 0;
    for (const { base } of WORKSHOP_BASES)
      for (const r of reseeded(base, 200)) {
        const d = heroDraws(r);
        const before = place(base, r, {
          slab: d.slab,
          turret: d.turret,
          picks: [],
        });
        if (before.length >= heroCap(r.hall)) continue;
        eligible++;
        if (place(base, r).some((h) => h.kind === "question-block")) blocks++;
      }
    const [lo, hi] = binomial(eligible, BLOCK_SHARE);
    expect(blocks).toBeGreaterThanOrEqual(Math.max(1, lo));
    expect(blocks).toBeLessThanOrEqual(hi);
    // Tried before the pool slots: in a one-hero hall a drawn block takes
    // the slot a taken pool slot would have had.
    for (const { archetype, base } of WORKSHOP_BASES) {
      expect(heroCap(base.hall), archetype).toBe(1);
      expect(
        placeHeroes(base, {
          slab: false,
          turret: false,
          block: true,
          picks: [{ take: true, roll: 0.5 }],
        }).map((h) => h.kind),
        archetype,
      ).toEqual(["question-block"]);
    }
  }, 30_000);

  it("keeps every slab and turret outcome as it was before the block's draw", () => {
    // The block is the third value on the "draw" stream (C9). Mutation
    // caught: the block drawn first or second, or on a stream of its own.
    for (const { base } of WORKSHOP_BASES)
      for (const r of reseeded(base, 200)) {
        const rng = createRng(seedFor(r.seed, "hero", "draw"));
        const d = heroDraws(r);
        expect(d.slab).toBe(rng.chance(SLAB_SHARE));
        expect(d.turret).toBe(rng.chance(TURRET_SHARE));
        // The block is the third value on the same stream.
        expect(d.block).toBe(rng.chance(BLOCK_SHARE));
      }
  });

  it("fills only a slot the pools left empty with the any-archetype draw (Review Focus 3)", () => {
    // Mutation caught: the any draw run before the pools, or ignoring the cap.
    let added = 0;
    for (const { base } of EVERY_BASE)
      for (const r of reseeded(base, 200)) {
        const d = heroDraws(r);
        // The same draws less the any-archetype one.
        const rest: HeroDraws = {
          slab: d.slab,
          turret: d.turret,
          picks: d.picks,
          ...(d.block === undefined ? {} : { block: d.block }),
        };
        const without = place(base, r, rest);
        const withAny = place(base, r);
        for (const h of without) expect(withAny).toContainEqual(h);
        expect(withAny.length - without.length).toBeLessThanOrEqual(1);
        expect(withAny.length).toBeLessThanOrEqual(heroCap(r.hall));
        const extra = withAny.filter(
          (h) => !without.some((o) => o.kind === h.kind),
        );
        for (const h of extra) {
          expect(
            ANY_POOL.map(([k]) => k),
            h.kind,
          ).toContain(h.kind);
          added++;
        }
      }
    expect(added).toBeGreaterThan(0);
  }, 60_000);

  it("lands each any-archetype kind in about 1 hall in 20 in the workshop", () => {
    // The planner measured 39 to 64 per 1000 (thunder hammer 64, cloud 56,
    // rocket 50, hoverboard 41, police box 39). Mutation caught: a kind
    // that never fits, or the draw taking twice as often or never.
    // Doubling is impossible above 1, so the constant pin carries the
    // share.
    expect(ANY_SHARE).toBe(3 / 4);
    const counts = new Map<HeroKind, number>();
    for (const { base } of WORKSHOP_BASES)
      for (const r of reseeded(base, 200))
        for (const h of place(base, r))
          counts.set(h.kind, (counts.get(h.kind) ?? 0) + 1);
    for (const [kind] of ANY_POOL) {
      const n = counts.get(kind) ?? 0;
      expect(n, kind).toBeGreaterThanOrEqual(25);
      expect(n, kind).toBeLessThanOrEqual(85);
    }
    console.info(
      `2.6c kinds per 1000 workshop rooms: ${JSON.stringify([...counts])}`,
    );
  }, 30_000);

  it("stands the garden robot only under a ceiling of 3.7 m or more (Review Focus 1)", () => {
    // Mutation caught: the gate dropped (the robot under 3.6), or the
    // comparison turned (no robot at 3.8).
    const lab = WORKSHOP_BASES.find((b) => b.archetype === "lab");
    if (lab === undefined) throw new Error("no lab base");
    const forced: HeroDraws = {
      slab: false,
      turret: false,
      picks: [{ take: true, roll: forcedPoolRoll("lab", "garden-robot") }],
    };
    for (const ceiling of [3.0, 3.6, 3.69])
      expect(placeHeroes({ ...lab.base, ceiling }, forced)).toEqual([]);
    for (const ceiling of [3.7, 3.8, 5.0])
      expect(
        placeHeroes({ ...lab.base, ceiling }, forced).map((h) => h.kind),
      ).toEqual(["garden-robot"]);
    for (const salience of [null, 0, 3]) {
      const room = generateRoom({
        ...CANNED_WORKSHOP,
        type: "guide",
        salience,
      });
      expect(room.ceiling).toBeLessThan(3.7);
      expect(
        roomWithForcedHero(
          { ...CANNED_WORKSHOP, type: "guide", salience },
          "garden-robot",
        ).placed,
      ).toBeNull();
    }
    expect(
      roomWithForcedHero(
        { ...CANNED_WORKSHOP, type: "guide", salience: 4 },
        "garden-robot",
      ).placed,
    ).toBe("garden-robot");
    // Every generated hero stays under its room's ceiling less the gap,
    // over rooms generated at every whole salience (and none), so the
    // ceilings run from 3.0 m to 5.0 m and cross the robot's gate. The
    // canned places alone sit at 4.0 m and more, where the gate never shuts.
    const ceilings = new Set<number>();
    let low = 0;
    let robots = 0;
    for (const p of [CANNED_WORKSHOP, CANNED_HUB, CANNED_BRIDGE])
      for (const [a, type] of Object.entries(TYPES))
        for (const salience of [null, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]) {
          const base = siteBase(
            generateRoom({ ...p, type, status: "stable", salience }),
          );
          ceilings.add(base.ceiling);
          for (const r of reseeded(base, a === "lab" ? 40 : 10)) {
            if (r.ceiling < 3.7) low++;
            for (const h of place(base, r)) {
              if (h.kind === "garden-robot") robots++;
              expect(
                HERO_CATALOGUE[h.kind].top + HERO_CEILING_GAP,
                `${h.kind} under ${String(r.ceiling)}`,
              ).toBeLessThanOrEqual(r.ceiling);
            }
          }
        }
    expect(Math.min(...ceilings)).toBe(LOWEST_CEILING);
    expect(Math.max(...ceilings)).toBe(5.0);
    expect(low).toBeGreaterThan(0);
    // The robot does stand where the ceiling lets it.
    expect(robots).toBeGreaterThan(0);
  }, 60_000);

  it("lets the player walk under the block and stops them at the board and the cloud (Review Focus 2)", () => {
    // Mutation caught: the block blocking, or a floater without a blocker.
    for (const kind of [
      "question-block",
      "hoverboard",
      "flying-cloud",
    ] as const) {
      const { room, placed } = roomWithForcedHero(CANNED_WORKSHOP, kind);
      expect(placed, kind).toBe(kind);
      const h = room.heroes.find((x) => x.kind === kind);
      if (h === undefined) throw new Error(kind);
      const box = heroFootprint(h);
      const blockers = blockersFor(room);
      const has = blockers.some(
        (b) =>
          b.x0 === box.x0 &&
          b.x1 === box.x1 &&
          b.z0 === box.z0 &&
          b.z1 === box.z1,
      );
      expect(has, kind).toBe(kind !== "question-block");
      const bare = { ...room, props: [], heroes: [] };
      const without = reachedTargets(bare, blockersFor(bare));
      const withAll = reachedTargets(room, blockers);
      for (const t of without)
        expect(withAll.has(t), `${kind} ${t}`).toBe(true);
      expect(
        dressingSites(room).lanes.some((l) => overlaps(box, l)),
        kind,
      ).toBe(false);
    }
  });

  it("keeps tall heroes off pipe runs, spans and the ceiling band (Review Focus 4)", () => {
    // Mutation caught: isTallHero's pipe-run clause dropped (the tank or the
    // rocket lands under an engineering pipe run), the police box's edge
    // not reserved, or the ceiling gate dropped (the robot under 3.0 m).
    const eng = [...WORKSHOP_BASES, ...HUB_BASES].filter(
      (b) => b.archetype === "engineering",
    );
    let seen = 0;
    for (const { base } of eng) {
      const low = { ...base, ceiling: LOWEST_CEILING };
      const pipes = low.decor
        .map((d) => pipeRunBox(d, low.hall))
        .filter((b) => b !== null);
      expect(pipes.length).toBeGreaterThan(0);
      for (const kind of HERO_KINDS.filter(isTallHero)) {
        if (kind === "black-slab") continue;
        // The robot is a lab kind: forced through the lab pool on this
        // engineering layout, under the 3.0 m ceiling it must never stand
        // under, so the ceiling assert below has a case that can fail.
        const pool = kind === "garden-robot" ? "lab" : "engineering";
        for (const r0 of reseeded(low, 40)) {
          const r: SiteBase = { ...r0, archetype: pool };
          const draws: HeroDraws =
            kind === "question-block"
              ? { slab: false, turret: false, block: true, picks: [] }
              : ANY_POOL.some(([k]) => k === kind)
                ? {
                    slab: false,
                    turret: false,
                    picks: [],
                    any: { take: true, roll: forcedAnyRoll(kind) },
                  }
                : {
                    slab: false,
                    turret: false,
                    picks: [
                      {
                        take: true,
                        roll: forcedPoolRoll(pool, kind),
                      },
                    ],
                  };
          const heroes = place(base, r, draws);
          const room = withHeroes(r, heroes);
          for (const h of heroes) {
            seen++;
            const moat = grow(heroFootprint(h), HERO_CLEAR);
            for (const p of pipes)
              expect(overlaps(moat, p), h.kind).toBe(false);
            expect(
              HERO_CATALOGUE[h.kind].top + HERO_CEILING_GAP,
            ).toBeLessThanOrEqual(r.ceiling);
            // Span lines are not edge-anchored; H4 already keeps them off
            // every reserved box, and dress.test.ts pins that.
            const edges = new Set(heroEdges(h).map(edgeKey));
            for (const p of room.props)
              if (
                p.anchor !== "floor" &&
                p.kind !== "span-duct" &&
                p.kind !== "span-tray"
              )
                expect(
                  edges.has(edgeKey(edgeOf(p))),
                  `${h.kind} ${p.kind}`,
                ).toBe(false);
          }
        }
      }
    }
    expect(seen).toBeGreaterThan(50);
  }, 60_000);

  it("keeps every hero in its hall, on its floor, out of every lane, its moat clear of every taken box and every other hero", () => {
    for (const { archetype, base } of EVERY_BASE)
      for (const r of reseeded(base, 100))
        expectHeroInvariants(
          `${base.permalink} ${archetype}`,
          r,
          place(base, r),
          sitesOf(base),
        );
  }, 30_000);

  it("never takes a fixture edge, an entrance or doorway edge, an edge beside a way or a backed hero's run end", () => {
    for (const { base } of EVERY_BASE) {
      const sites = sitesOf(base);
      const beside = new Set<string>();
      for (const f of base.fixtures) {
        if (f.kind !== "door" && f.kind !== "hatch" && f.kind !== "portal")
          continue;
        for (const run of sites.runs) {
          const i = run.findIndex((e) => edgeKey(e) === edgeKey(f.slot));
          if (i < 0) continue;
          for (const n of [run[i - 1], run[i + 1]])
            if (n !== undefined) beside.add(edgeKey(n));
        }
      }
      const ends = new Set(
        sites.runs
          .flatMap((run) => [run[0], run.at(-1)])
          .flatMap((e) => (e === undefined ? [] : [edgeKey(e)])),
      );
      for (const r of reseeded(base, 100))
        for (const h of place(base, r))
          for (const e of heroEdges(h)) {
            const k = edgeKey(e);
            expect(sites.free.has(k), `${h.kind} ${k}`).toBe(true);
            expect(beside.has(k), `${h.kind} ${k}`).toBe(false);
            if (HERO_FOOTING[h.kind] === "backed")
              expect(ends.has(k), `${h.kind} ${k}`).toBe(false);
          }
    }
  }, 30_000);

  it("keeps a cabinet's use point on the floor and clear of every blocker", () => {
    let seen = 0;
    for (const { base } of [...WORKSHOP_BASES, ...HUB_BASES])
      for (const r of reseeded(base, 400)) {
        const all = place(base, r);
        const heroes = all.filter((h) => HERO_CATALOGUE[h.kind].use !== null);
        if (heroes.length === 0) continue;
        const room = withHeroes(r, all);
        const blockers = blockersFor(room);
        for (const h of heroes) {
          const p = heroUsePoint(h);
          if (p === null) throw new Error(h.kind);
          const cell = { x0: p.x, x1: p.x + 1e-6, z0: p.z, z1: p.z + 1e-6 };
          expect(fitsFloor(r, cell), h.kind).toBe(true);
          for (const b of blockers)
            expect(distanceTo(p.x, p.z, b), h.kind).toBeGreaterThanOrEqual(
              PLAYER_RADIUS,
            );
          seen++;
        }
      }
    expect(seen).toBeGreaterThan(0);
  }, 30_000);

  // One room per kind: the first reseeded workshop, hub or bridge layout
  // that places it.
  for (const kind of HERO_KINDS)
    it(`reaches with a ${kind} every target the room reaches without heroes and props`, () => {
      let found: RoomSpec | null = null;
      for (const { base } of EVERY_BASE) {
        for (const r of reseeded(base, 1000)) {
          const heroes = place(base, r);
          if (heroes.some((h) => h.kind === kind)) {
            found = withHeroes(r, heroes);
            break;
          }
        }
        if (found !== null) break;
      }
      if (found === null) throw new Error(`no layout places a ${kind}`);
      const bare = { ...found, props: [], heroes: [] };
      const without = reachedTargets(bare, blockersFor(bare));
      const withAll = reachedTargets(found, blockersFor(found));
      for (const t of without)
        expect(withAll.has(t), `${kind} ${t}`).toBe(true);
      for (const f of found.fixtures) {
        if (f.kind !== "door" && f.kind !== "hatch" && f.kind !== "portal")
          continue;
        const a = arrivalPoint(f.slot);
        for (const h of found.heroes) {
          const b = heroBlocker(h);
          if (b !== null)
            expect(distanceTo(a.x, a.z, b), kind).toBeGreaterThanOrEqual(
              PLAYER_RADIUS,
            );
        }
      }
    }, 30_000);

  it("gives the same heroes whatever order the lists come in", () => {
    // The canned workshop draws no hero at its own seed and the hub only
    // the hoverboard, so the lists are reversed on reseeds, and some of
    // those must carry heroes.
    let placed = 0;
    for (const { base } of [...HUB_BASES, ...WORKSHOP_BASES])
      for (const r of reseeded(base, 20)) {
        const heroes = place(base, r);
        const shuffled = {
          ...r,
          fixtures: [...r.fixtures].reverse(),
          decor: [...r.decor].reverse(),
        };
        // The shuffled room works out its own sites from its own lists.
        expect(placeHeroes(shuffled)).toEqual(heroes);
        if (heroes.length > 0) placed++;
      }
    expect(placed).toBeGreaterThan(0);
  }, 30_000);

  it("draws the slab before the turret, so a one-hero hall that draws both stands the slab", () => {
    for (const { archetype, base } of BRIDGE_BASES) {
      expect(heroCap(base.hall), archetype).toBe(1);
      expect(
        placeHeroes(base, { slab: true, turret: true, picks: [] }).map(
          (h) => h.kind,
        ),
        archetype,
      ).toEqual(["black-slab"]);
    }
  });

  it("pins the empty place's own hero exactly, so a change to the candidate order shows", () => {
    // A 5 by 6 lab hall at its own seed draws the turret. It stands in the
    // south-west corner cell (0, 5), turned north towards the hall's
    // centre (2.5, 3), on the seed of that corner candidate.
    const room = generateRoom(DEGENERATE_PLACES.empty);
    expect(room.hall).toEqual({ x0: 0, y0: 0, x1: 5, y1: 6 });
    expect(room.heroes).toEqual([
      {
        kind: "turret",
        variant: 0,
        x: 0.5,
        y: 5.5,
        turn: 0,
        seed: seedFor(room.seed, "hero", 0, 5, "corner"),
      },
    ]);
    expect(room.heroes[0]?.seed).toBe(184992945357087);
  });

  it("places heroes in degenerate rooms without breaking an invariant (Review Focus 1)", () => {
    const rooms = [
      ...Object.entries(DEGENERATE_PLACES).map(([name, p]) => ({
        name,
        place: p,
      })),
      { name: "over cap", place: OVER_CAP },
      {
        name: "workshop draft",
        place: { ...CANNED_WORKSHOP, status: "draft" },
      },
    ];
    let dressed = 0;
    for (const { name, place: p } of rooms) {
      const base = siteBase(generateRoom(p));
      let checkedHere = 0;
      for (const r of reseeded(base, 200)) {
        const heroes = place(base, r);
        expectHeroInvariants(name, r, heroes, sitesOf(base));
        if (heroes.length === 0) continue;
        const room = withHeroes(r, heroes);
        const reserve = heroReserve(heroes);
        for (const q of room.props) {
          const b = propFootprint(q);
          if (b !== null)
            for (const box of reserve.boxes)
              expect(overlaps(b, box), `${name} ${q.kind}`).toBe(false);
        }
        dressed++;
        if (++checkedHere >= 20) break;
      }
    }
    expect(dressed).toBeGreaterThan(0);
  }, 60_000);

  it("keeps readers and signs where they were when a hero hangs on the same wall (Review Focus 2)", () => {
    // The mandatory ones only (step 2 of the dressing): the council and
    // archive palettes also pick a sign plate as an optional wall prop, and
    // a hero may well take the edge one of those would have had.
    const marks = (room: RoomBase) =>
      dressCandidates(room)
        .filter(
          ({ prop: p, mandatory }) =>
            mandatory &&
            (p.kind === "keycard-reader" || p.kind === "sign-plate"),
        )
        .map(({ prop: p }) => `${p.kind} ${String(p.x)},${String(p.y)}`)
        .sort();
    let checked = 0;
    for (const { base } of EVERY_BASE) {
      const plain = marks({ ...base, heroes: [] });
      let checkedHere = 0;
      for (const r of reseeded(base, 200)) {
        const onWalls = place(base, r).filter(
          (h) => HERO_FOOTING[h.kind] !== "free",
        );
        if (onWalls.length === 0) continue;
        expect(marks({ ...base, heroes: onWalls }), base.permalink).toEqual(
          plain,
        );
        checked++;
        if (++checkedHere >= 20) break;
      }
    }
    expect(checked).toBeGreaterThan(20);
  }, 60_000);

  it("gives a cabinet's use point in world metres, HERO_USE_OUT in front of its face", () => {
    for (const kind of ["arcade-cabinet", "recruit-cabinet"] as const)
      for (const side of SIDES) {
        const h = heroAt(kind, 0, side);
        const p = heroUsePoint(h);
        if (p === null) throw new Error(kind);
        const box = heroFootprint(h);
        expect(distanceTo(p.x, p.z, box), `${kind} ${side}`).toBeCloseTo(
          HERO_USE_OUT,
          9,
        );
      }
    expect(heroUsePoint(heroAt("turret", 0))).toBeNull();
  });
});
