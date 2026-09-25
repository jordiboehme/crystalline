/**
 * The hero catalogue, footprints and reservation: every kind's size,
 * footing, height band and pool, and what each hero blocks and reserves.
 * The pass that places heroes (`heroDraws`, `placeHeroes`) is held to its
 * rates, its rules and the flood fill in "the hero pass" below.
 */
import { describe, expect, it } from "vitest";

import { seedFor } from "../core/seed";
import { CANNED_BRIDGE, CANNED_HUB, CANNED_WORKSHOP } from "./canned";
import { dressCandidates, dressRoom } from "./dress";
import {
  FOOTPRINTS,
  HERO_FOOTING,
  footprint,
  heroBlocker,
  heroFootprint,
  propFootprint,
} from "./footprints";
import { generateRoom } from "./generate";
import {
  HERO_CATALOGUE,
  HERO_CLEAR,
  HERO_KINDS,
  HERO_POOLS,
  HERO_SHARE,
  HERO_USE_OUT,
  HERO_VIEW,
  HERO_WALL_TOP,
  SLAB_SHARE,
  SLAB_TOP,
  TURRET_SHARE,
  heroCap,
  heroDraws,
  heroEdges,
  heroReserve,
  heroSurfaces,
  heroUsePoint,
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
  fitsFloor,
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
  const room: RoomSpec = { ...base, heroes, props: [] };
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
      const pool = new Set<HeroKind>(HERO_POOLS[archetype].map(([k]) => k));
      for (const r of reseeded(base, 200))
        for (const h of place(base, r))
          if (h.kind !== "turret" && h.kind !== "black-slab")
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
    for (const { base } of HUB_BASES) {
      const shuffled = {
        ...base,
        fixtures: [...base.fixtures].reverse(),
        decor: [...base.decor].reverse(),
      };
      expect(placeHeroes(shuffled)).toEqual(placeHeroes(base));
    }
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
