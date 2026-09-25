/**
 * The hero props: rare, large pieces a room is remembered by (H2). They are
 * pure decoration like the set dressing, never data, but they are not
 * props: they have their own kinds (`HeroKind`), their own list
 * (`RoomSpec.heroes`), their own footprints (`FOOTPRINTS.hero`, free of
 * `MAX_FLOOR_PROP`) and this module. A hall holds at most one, or two when
 * both its sides reach `HERO_BIG_HALL` cells (`heroCap`).
 *
 * The catalogue (`HERO_CATALOGUE`, H8) says for each kind where it stands
 * (`HeroPlacement`), how many variants it has, how tall it is, how many
 * wall edges it takes, the tops a later pass may set a small prop on and
 * where a player would stand to use it. The pools (`HERO_POOLS`, H7) weight
 * each archetype's own kinds; the turret and the black slab are drawn
 * apart from every pool, at `TURRET_SHARE` and `SLAB_SHARE`.
 *
 * What a hero reserves (`heroReserve`, H19) is what the dressing keeps off:
 * a free or backed hero's box grown by `HERO_CLEAR` on every side, a flush
 * wall hero's clear view box `HERO_VIEW` deep in front of it, and the wall
 * edges of every wall-anchored hero. The dressing reads it from the room
 * itself (`dressCandidates` in `dress.ts`, H3), so no caller can forget it.
 *
 * The pass (`placeHeroes`) runs in `generateRoom` after the scaffold and
 * before the dressing, on the room's sites (`dressingSites`, which read no
 * hero and no prop). In this order:
 *
 * 1. The draws (`heroDraws`, H6, H7): `seedFor(room.seed, "hero", "draw")`
 *    draws the slab's chance (`SLAB_SHARE`) and then the turret's
 *    (`TURRET_SHARE`); each of the `heroCap(room.hall)` pool slots draws a
 *    chance of `HERO_SHARE` and a roll from `seedFor(room.seed, "hero",
 *    "pick", i)`.
 * 2. The slab, when drawn: one candidate, at the hall's centre x with its
 *    south face on the hall's centre line, facing the entrance (turn 2,
 *    H9). It stands only where the centre is free: its box grown by
 *    `HERO_CLEAR` also keeps off every pipe run's box.
 * 3. The turret, when drawn and a slot is left: the cells of the hall's
 *    corner zones that kept all four spots, centred on the cell and turned
 *    to face the hall's centre (H10).
 * 4. The pool slots, in order, while a slot is left: a slot that drew its
 *    chance picks by its roll from the archetype's pool (`HERO_POOLS`) less
 *    the kinds already placed, and tries that kind. A kind that finds no
 *    place leaves its slot empty; nothing is drawn again.
 * 5. Trying a kind: its candidates by placement, in the order of their
 *    seeds (`seedFor(room.seed, "hero", <anchor ints>, <token>)`, H18; ties
 *    by the anchor's y, then x):
 *    - `wall` and `backed`: every free wall edge of the hall that is not
 *      beside a door, hatch or portal on its run (H5) and not a placed
 *      hero's edge, token `wall-<side>`, anchored at `wallAnchor`; a backed
 *      kind never takes a run's first or last edge (H24), and the core wall
 *      takes an edge and the next one of its run, anchored between them;
 *    - `band`: every half-cell point of the interior band, token `band`,
 *      its box inside the band, turned by its second draw;
 *    - `corner`: as in step 3, token `corner`;
 *    - `centre`: as in step 2, token `centre`.
 *    Each candidate's first draw is the variant. The first candidate whose
 *    hero's box fits the hall's floor and enters no lane is placed when,
 *    for a free or backed hero, the box grown by `HERO_CLEAR` overlaps no
 *    taken box and no placed hero (H20), and, for a flush one, the box
 *    grown by `HERO_CLEAR` overlaps no placed free or backed hero (the
 *    same moat, seen from the flush side). Anchors are rounded to three
 *    decimals before they are measured.
 * 6. The output, sorted by `HERO_ORDER`.
 *
 * The surface hook (H21, C3): `HERO_CATALOGUE[kind].surfaces` lists each
 * top in the hero's local terms (`SurfaceSpec`, with its free height
 * `clear` and its class `cls`), and `under` the spots below the top (the
 * floor in a knee space, or a lower shelf), which bypass the reserve since
 * they lie inside the hero's own box. `heroSurfaces` and `heroUnder` give
 * them in world metres for the curios (`curios.ts`), through `heroPoint`
 * and so `turnedPoint`. `heroUsePoint` gives a cabinet's use point the same
 * way.
 *
 * This is the generator side: it imports `footprints.ts`, `sites.ts`,
 * `types.ts`, `units.ts` and the seeds (and may import `props.ts`), and
 * never `move.ts`, `generate.ts`, `interact.ts`, `malfunction.ts` or
 * anything under `render/` (ruling 20). `heroes.test.ts`, `dress.test.ts`
 * and `sites.test.ts` keep it so.
 */

import { createRng, seedFor, type Rng } from "../core/seed";
import {
  FOOTPRINTS,
  HERO_FOOTING,
  OPEN_CLEAR,
  heroFootprint,
  heroTurn,
  pipeRunBox,
  turnedBox,
  turnedPoint,
} from "./footprints";
import {
  EPS,
  dressingSites,
  edgeKey,
  fitsFloor,
  grow,
  inside,
  interiorBand,
  overlaps,
  pickByRoll,
  round3,
  wallAnchor,
  type DressingSites,
  type Reserved,
  type SiteBase,
} from "./sites";
import type {
  Archetype,
  Box,
  Hero,
  HeroKind,
  HeroPlacement,
  Rect,
  Side,
  SurfaceClass,
  SurfaceSpec,
  WallSlot,
} from "./types";
import { CELL } from "./units";

/** Every hero kind, once each, in catalogue order. */
export const HERO_KINDS: readonly HeroKind[] = [
  "turret",
  "black-slab",
  "eye-panel",
  "photo-console",
  "laser-desk",
  "mess-table",
  "helper-robot",
  "sleep-ring",
  "dome-planters",
  "core-wall",
  "gun-rack",
  "gun-bench",
  "tube-bench",
  "field-pack",
  "arcade-cabinet",
  "recruit-cabinet",
];

/** One kind's entry: where it stands, how many variants, how tall, its edges and its tops. */
export interface HeroEntry {
  placement: HeroPlacement;
  variants: number;
  top: number;
  /** Wall edges a wall or backed kind takes: 1, or 2 for the core wall. */
  edges: 1 | 2;
  /**
   * The tops a curio may stand on (H21, C3), in the hero's local terms at
   * turn 0 (`SurfaceSpec`: `a` along its width, `d` along its depth from
   * the wall point for a wall-anchored hero or from the centre for a free
   * one), each with its free height `clear` and its class `cls`.
   */
  surfaces: readonly SurfaceSpec[];
  /**
   * Spots below the top a curio of class `under` may stand on (C3); they
   * bypass the reserve, since they lie inside the hero's own box.
   */
  under: readonly SurfaceSpec[];
  /**
   * Where a player would stand to use it, in the same local terms, or null:
   * the arcade cabinets keep one (spec: a later spec makes E open a game),
   * `HERO_USE_OUT` (0.45 m) in front of the cabinet's face on its centre
   * line. Nothing reads it in 2.6a but the tests.
   */
  use: { a: number; d: number } | null;
}

/**
 * How far in front of a cabinet's face its use point stands, in metres:
 * more than the player's radius (0.35 m), so a player standing there does
 * not touch the cabinet, and less than `HERO_CLEAR`, so the point lies in
 * the clear ring the cabinet reserves.
 */
export const HERO_USE_OUT = 0.45;

/**
 * Every hero kind (H8, H13): its placement, its variant count (one size
 * each in `FOOTPRINTS.hero`), its top in metres, its wall edges (1, unused,
 * for a kind that stands free), its surfaces (H21) and its use point. A
 * floor hero stays under 2.2 m but the slab (`SLAB_TOP`), a wall hero under
 * `HERO_WALL_TOP`. An arcade cabinet's three variants are its three games,
 * one size.
 */
export const HERO_CATALOGUE = {
  turret: {
    placement: "corner",
    variants: 1,
    top: 1.3,
    edges: 1,
    surfaces: [],
    under: [],
    use: null,
  },
  "black-slab": {
    placement: "centre",
    variants: 1,
    top: 2.7,
    edges: 1,
    surfaces: [],
    under: [],
    use: null,
  },
  "eye-panel": {
    placement: "wall",
    variants: 1,
    top: 2.2,
    edges: 1,
    surfaces: [],
    under: [],
    use: null,
  },
  "photo-console": {
    placement: "backed",
    variants: 1,
    top: 1.9,
    edges: 1,
    surfaces: [],
    under: [],
    use: null,
  },
  "laser-desk": {
    placement: "band",
    variants: 1,
    top: 2.2,
    edges: 1,
    surfaces: [
      {
        a0: -1.1,
        a1: -0.35,
        d0: -0.2,
        d1: 0.4,
        h: 0.74,
        clear: 1.15,
        cls: "desk",
      },
    ],
    under: [
      {
        a0: -1.05,
        a1: -0.3,
        d0: -0.2,
        d1: 0.4,
        h: 0,
        clear: 0.68,
        cls: "under",
      },
    ],
    use: null,
  },
  "mess-table": {
    placement: "band",
    variants: 1,
    top: 1.1,
    edges: 1,
    surfaces: [
      {
        a0: -2.1,
        a1: 0.9,
        d0: -0.4,
        d1: 0.4,
        h: 0.76,
        clear: OPEN_CLEAR,
        cls: "table",
      },
    ],
    // Nothing under the top: the spine and its foot plate fill it (C3).
    under: [],
    use: null,
  },
  "helper-robot": {
    placement: "band",
    variants: 1,
    top: 1.6,
    edges: 1,
    surfaces: [],
    under: [],
    use: null,
  },
  "sleep-ring": {
    placement: "band",
    variants: 1,
    top: 1.4,
    edges: 1,
    surfaces: [],
    under: [],
    use: null,
  },
  "dome-planters": {
    placement: "band",
    variants: 2,
    top: 1.5,
    edges: 1,
    surfaces: [],
    under: [],
    use: null,
  },
  "core-wall": {
    placement: "wall",
    variants: 1,
    top: 2.25,
    edges: 2,
    surfaces: [],
    under: [],
    use: null,
  },
  "gun-rack": {
    placement: "wall",
    variants: 1,
    top: 1.9,
    edges: 1,
    surfaces: [],
    under: [],
    use: null,
  },
  "gun-bench": {
    placement: "backed",
    variants: 1,
    top: 1.4,
    edges: 1,
    surfaces: [
      {
        a0: -0.9,
        a1: -0.55,
        d0: 0.1,
        d1: 0.8,
        h: 0.9,
        clear: OPEN_CLEAR,
        cls: "bench",
      },
    ],
    under: [
      {
        a0: -0.83,
        a1: 0.83,
        d0: 0.13,
        d1: 0.77,
        h: 0.31,
        clear: 0.52,
        cls: "under",
      },
    ],
    use: null,
  },
  "tube-bench": {
    placement: "backed",
    variants: 1,
    top: 2.0,
    edges: 1,
    surfaces: [
      {
        a0: -0.9,
        a1: -0.3,
        d0: 0.1,
        d1: 0.8,
        h: 0.9,
        clear: OPEN_CLEAR,
        cls: "bench",
      },
    ],
    under: [
      {
        a0: -0.83,
        a1: 0.2,
        d0: 0.13,
        d1: 0.77,
        h: 0.31,
        clear: 0.52,
        cls: "under",
      },
    ],
    use: null,
  },
  "field-pack": {
    placement: "corner",
    variants: 2,
    top: 1.6,
    edges: 1,
    surfaces: [],
    under: [],
    use: null,
  },
  "arcade-cabinet": {
    placement: "backed",
    variants: 3,
    top: 1.95,
    edges: 1,
    surfaces: [],
    under: [],
    use: { a: 0, d: 0.9 + HERO_USE_OUT },
  },
  "recruit-cabinet": {
    placement: "backed",
    variants: 1,
    top: 2.0,
    edges: 1,
    surfaces: [],
    under: [],
    use: { a: 0, d: 1.2 + HERO_USE_OUT },
  },
} satisfies Record<HeroKind, HeroEntry>;

/**
 * Each archetype's weighted pool (H7); the turret and the slab are drawn
 * apart. The pools are listed in catalogue order, and that order is part
 * of the seeded result, so do not reorder them.
 */
export const HERO_POOLS = {
  bridge: [
    ["eye-panel", 4],
    ["photo-console", 4],
    ["laser-desk", 1],
    ["arcade-cabinet", 1],
    ["recruit-cabinet", 1],
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
  ],
  archive: [
    ["core-wall", 6],
    ["arcade-cabinet", 1],
    ["recruit-cabinet", 1],
  ],
  lab: [
    ["photo-console", 3],
    ["laser-desk", 2],
    ["dome-planters", 3],
    ["tube-bench", 2],
    ["field-pack", 1],
    ["arcade-cabinet", 1],
    ["recruit-cabinet", 1],
  ],
} satisfies Record<Archetype, readonly (readonly [HeroKind, number])[]>;

/** Both hall sides at least this many cells: a second hero (H6). */
export const HERO_BIG_HALL = 16;
/** Chance a pool slot draws a hero (H7). */
export const HERO_SHARE = 1 / 2;
/** Chance a room draws the turret (spec: about 1 room in 6). */
export const TURRET_SHARE = 1 / 6;
/** Chance a room draws the slab (spec: about 1 room in 40, where the centre is free). */
export const SLAB_SHARE = 1 / 40;
/** The moat around a free or backed hero, in metres (H19, H20): wider than the player (0.7 m). */
export const HERO_CLEAR = 1.0;
/** How deep the clear view box in front of a flush wall hero is, in metres (H19). */
export const HERO_VIEW = 1.5;
/** The slab's height (H9): nine of its 0.3 m depth. */
export const SLAB_TOP = 2.7;
/**
 * The world side's copy of the models' `WALL_TOP` (2.25), the highest a
 * flush hero reaches; `heroModels.test.ts` pins them equal (H13).
 */
export const HERO_WALL_TOP = 2.25;

/** How many heroes a hall holds (H6): 2 when both sides reach `HERO_BIG_HALL`, else 1. */
export function heroCap(hall: Rect): 1 | 2 {
  return hall.x1 - hall.x0 >= HERO_BIG_HALL &&
    hall.y1 - hall.y0 >= HERO_BIG_HALL
    ? 2
    : 1;
}

/** The wall a wall-anchored hero at each quarter turn stands on: `turnForSide` inverted. */
const SIDE_FOR_TURN: readonly Side[] = ["s", "w", "n", "e"];

/**
 * The wall edges a wall-anchored hero takes, from its wall point (the
 * inverse of `wallAnchor` in `sites.ts`): none for a `free` footing. The
 * side is the one whose `turnForSide` is the hero's turn. A one-edge hero's
 * cell is `(x - 0.5, y)` on an `n` wall, `(x - 0.5, y - 1)` on `s`,
 * `(x, y - 0.5)` on `w` and `(x - 1, y - 0.5)` on `e`. A two-edge hero (the
 * core wall) is anchored on the cell border between its two edges, so its
 * cells are columns `x - 1` and `x` of that row on an `n` or `s` wall, and
 * rows `y - 1` and `y` of that column on a `w` or `e` wall.
 */
export function heroEdges(h: Hero): WallSlot[] {
  if (HERO_FOOTING[h.kind] === "free") return [];
  const side = SIDE_FOR_TURN[heroTurn(h)] ?? "s";
  const two = HERO_CATALOGUE[h.kind].edges === 2;
  const along = side === "n" || side === "s";
  // The cell just inside the wall, before the anchor's along-wall half.
  const row = side === "s" ? h.y - 1 : h.y;
  const col = side === "e" ? h.x - 1 : h.x;
  if (along) {
    const y = Math.round(row);
    return two
      ? [
          { x: Math.round(h.x - 1), y, side },
          { x: Math.round(h.x), y, side },
        ]
      : [{ x: Math.round(h.x - 0.5), y, side }];
  }
  const x = Math.round(col);
  return two
    ? [
        { x, y: Math.round(h.y - 1), side },
        { x, y: Math.round(h.y), side },
      ]
    : [{ x, y: Math.round(h.y - 0.5), side }];
}

/**
 * What the heroes reserve for the dressing (H19): for a free or backed
 * hero its box (`heroFootprint`) grown by `HERO_CLEAR` on every side, which
 * for a backed hero is also the clear use box in front of it; for a flush
 * wall hero its view box, its own width and `HERO_VIEW` deep out from its
 * wall point along `HERO_FRONT` (`heroFootprint(h, HERO_VIEW)`). Every
 * wall-anchored hero, backed or flush, also reserves its wall edges
 * (`heroEdges`).
 */
export function heroReserve(heroes: readonly Hero[]): Reserved {
  const boxes: Box[] = [];
  const edges = new Set<string>();
  for (const h of heroes) {
    const footing = HERO_FOOTING[h.kind];
    if (footing === "flush") boxes.push(heroFootprint(h, HERO_VIEW));
    else {
      const box = heroFootprint(h);
      boxes.push({
        x0: box.x0 - HERO_CLEAR,
        x1: box.x1 + HERO_CLEAR,
        z0: box.z0 - HERO_CLEAR,
        z1: box.z1 + HERO_CLEAR,
      });
    }
    if (footing !== "free") for (const e of heroEdges(h)) edges.add(edgeKey(e));
  }
  return { boxes, edges };
}

/**
 * Where a hero's local point `(a, d)` lies in world metres: `a` along the
 * hero's width and `d` along its depth, in the terms the models are built
 * in (`frameAt` in `render/kit.ts`). It is `turnedPoint` (`footprints.ts`,
 * C4) at the hero's anchor and turn: `anchor * CELL + along * a + front *
 * d`, with `front = HERO_FRONT[turn]` (the frame's `inward`) and `along =
 * [front[1], -front[0]]`, the frame's `along`: `[-1, 0]` at turn 0, since
 * a piece facing north has its `along` running west. Every helper that
 * turns a catalogue point into the world goes through `turnedPoint`, so
 * they all mirror the kit alike (`heroes.test.ts` pins this against
 * `frameAt` and `turnPoint` at every turn, `curios.test.ts` pins
 * `turnedPoint` itself).
 */
export function heroPoint(
  h: Hero,
  a: number,
  d: number,
): { x: number; z: number } {
  return turnedPoint(h.x, h.y, heroTurn(h), a, d);
}

/** One of a hero's surfaces in world metres: its box, height, free height and class. */
export interface HeroSurface {
  box: Box;
  h: number;
  clear: number;
  cls: SurfaceClass;
}

/**
 * Each of `specs` in world metres for hero `h`: its four corners turned
 * as `heroPoint` turns them (`turnedBox` in `footprints.ts`), and its box
 * spanning their extremes. What `heroSurfaces` and `heroUnder` share.
 */
function heroRects(h: Hero, specs: readonly SurfaceSpec[]): HeroSurface[] {
  const turn = heroTurn(h);
  return specs.map((s) => ({
    box: turnedBox(h.x, h.y, turn, s),
    h: s.h,
    clear: s.clear,
    cls: s.cls,
  }));
}

/**
 * The hero's tops (H21) in world metres, each with its height `h`, its
 * free height `clear` and its class `cls`: each top's four corners through
 * `heroPoint`, and its box spanning their extremes.
 */
export function heroSurfaces(h: Hero): HeroSurface[] {
  const specs: readonly SurfaceSpec[] = HERO_CATALOGUE[h.kind].surfaces;
  return heroRects(h, specs);
}

/**
 * The hero's under spots (C3) in world metres, in the same shape as
 * `heroSurfaces`: the catalogue's `under` list turned the same way.
 */
export function heroUnder(h: Hero): HeroSurface[] {
  const specs: readonly SurfaceSpec[] = HERO_CATALOGUE[h.kind].under;
  return heroRects(h, specs);
}

/** The heroes' order in `RoomSpec.heroes`: by `y`, then `x`, then kind by code point. */
export const HERO_ORDER = (a: Hero, b: Hero): number =>
  a.y - b.y || a.x - b.x || (a.kind < b.kind ? -1 : a.kind > b.kind ? 1 : 0);

/**
 * Where a player would stand to use a hero, in world metres, or null for a
 * kind with no use point: the catalogue's `use` through `heroPoint`, as
 * `heroSurfaces` turns its tops. For a cabinet it is `HERO_USE_OUT` in
 * front of its face, on its centre line.
 */
export function heroUsePoint(h: Hero): { x: number; z: number } | null {
  const use = HERO_CATALOGUE[h.kind].use;
  if (use === null) return null;
  return heroPoint(h, use.a, use.d);
}

/** The draws of one room (H6, H7): the slab, the turret and each pool slot's chance and roll. */
export interface HeroDraws {
  slab: boolean;
  turret: boolean;
  picks: readonly { take: boolean; roll: number }[];
}

/**
 * A room's hero draws, from its own streams: `seedFor(room.seed, "hero",
 * "draw")` draws the slab's chance and then the turret's; pool slot `i`
 * (one per `heroCap`) draws its chance of `HERO_SHARE` and its roll from
 * `seedFor(room.seed, "hero", "pick", i)`.
 */
export function heroDraws(room: SiteBase): HeroDraws {
  const rng = createRng(seedFor(room.seed, "hero", "draw"));
  const slab = rng.chance(SLAB_SHARE);
  const turret = rng.chance(TURRET_SHARE);
  const picks = Array.from({ length: heroCap(room.hall) }, (_, i) => {
    const r = createRng(seedFor(room.seed, "hero", "pick", i));
    return { take: r.chance(HERO_SHARE), roll: r.next() };
  });
  return { slab, turret, picks };
}

/** One place a hero may be tried at: its seed, its anchor and how to make the hero. */
interface HeroCandidate {
  seed: number;
  x: number;
  y: number;
  make(rng: Rng): Hero;
}

/**
 * The quarter turn that faces a corner hero at `(x, y)` towards the hall's
 * centre as near as a quarter turn allows (H10): north or south when the
 * centre lies at least as far along y as along x, else east or west.
 * Exported so `canned.ts`'s hand-placed hero hall can turn its turret the
 * same way a generated room's corner hero would, rather than keep its own
 * copy.
 */
export function faceCentre(hall: Rect, x: number, y: number): number {
  const dx = (hall.x0 + hall.x1) / 2 - x;
  const dy = (hall.y0 + hall.y1) / 2 - y;
  if (Math.abs(dy) >= Math.abs(dx)) return dy < 0 ? 0 : 2;
  return dx > 0 ? 1 : 3;
}

/**
 * The hero pass: the heroes of a room, at most `heroCap(room.hall)` of
 * them, no two of a kind, sorted by `HERO_ORDER` (see the module doc's
 * numbered pass). `draws` default to the room's own (`heroDraws`) and
 * `sites` to `dressingSites(room)`; the tests pass forced draws, and one
 * sites object per layout, since the sites never read the seed. The
 * generator passes neither.
 */
export function placeHeroes(
  room: SiteBase,
  draws: HeroDraws = heroDraws(room),
  sites: DressingSites = dressingSites(room),
): Hero[] {
  const hall = room.hall;
  const cap = heroCap(hall);
  const out: Hero[] = [];
  const band = interiorBand(hall);
  const pipeRuns = room.decor
    .map((d) => pipeRunBox(d, hall))
    .filter((b) => b !== null);

  // The run neighbours of every way's edge (H5), which no hero takes.
  const beside = new Set<string>();
  const ways = new Set(
    room.fixtures
      .filter(
        (f) => f.kind === "door" || f.kind === "hatch" || f.kind === "portal",
      )
      .map((f) => edgeKey(f.slot)),
  );
  for (const run of sites.runs)
    for (const [i, e] of run.entries()) {
      if (!ways.has(edgeKey(e))) continue;
      for (const n of [run[i - 1], run[i + 1]])
        if (n !== undefined) beside.add(edgeKey(n));
    }

  const fits = (h: Hero): boolean => {
    const kind = h.kind;
    const box = heroFootprint(h);
    if (!fitsFloor(room, box)) return false;
    if (sites.lanes.some((l) => overlaps(box, l))) return false;
    const placement = HERO_CATALOGUE[kind].placement;
    if (
      placement === "band" &&
      (band === null ||
        box.x0 < band.x0 * CELL - EPS ||
        box.x1 > band.x1 * CELL + EPS ||
        box.z0 < band.y0 * CELL - EPS ||
        box.z1 > band.y1 * CELL + EPS)
    )
      return false;
    if (HERO_FOOTING[kind] === "flush") {
      // The moat rule run the other way: a flush hero stays out of the
      // moat of every blocking hero placed before it.
      const ring = grow(box, HERO_CLEAR);
      return !out.some(
        (o) =>
          HERO_FOOTING[o.kind] !== "flush" && overlaps(ring, heroFootprint(o)),
      );
    }
    const moat = grow(box, HERO_CLEAR);
    const solid = [
      ...sites.taken,
      ...out.map((o) => heroFootprint(o)),
      ...(placement === "centre" ? pipeRuns : []),
    ];
    return !solid.some((b) => overlaps(moat, b));
  };

  // Every hero is made here: the candidate's first draw is the variant,
  // then `at` gives the anchor and turn (a band hero draws its turn second,
  // the slab's anchor reads the variant's depth), rounded to three
  // decimals before anything measures it.
  const hero = (
    kind: HeroKind,
    rng: Rng,
    seed: number,
    at: (variant: number) => { x: number; y: number; turn: number },
  ): Hero => {
    const variant = rng.int(0, HERO_CATALOGUE[kind].variants - 1);
    const { x, y, turn } = at(variant);
    return { kind, variant, x: round3(x), y: round3(y), turn, seed };
  };

  const candidates = (kind: HeroKind): HeroCandidate[] => {
    const entry = HERO_CATALOGUE[kind];
    const list: HeroCandidate[] = [];
    switch (entry.placement) {
      case "wall":
      case "backed": {
        const taken = new Set(out.flatMap(heroEdges).map(edgeKey));
        const usable = (e: WallSlot) => {
          const k = edgeKey(e);
          return (
            inside(hall, e.x, e.y) &&
            sites.free.has(k) &&
            !beside.has(k) &&
            !taken.has(k)
          );
        };
        for (const run of sites.runs)
          for (const [i, e] of run.entries()) {
            if (!usable(e)) continue;
            if (entry.placement === "backed" && !(i > 0 && i < run.length - 1))
              continue;
            let a = wallAnchor(e);
            if (entry.edges === 2) {
              const n = run[i + 1];
              if (n === undefined || !usable(n)) continue;
              const b = wallAnchor(n);
              a = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2, turn: a.turn };
            }
            const seed = seedFor(room.seed, "hero", e.x, e.y, `wall-${e.side}`);
            const { x, y, turn } = a;
            list.push({
              seed,
              x,
              y,
              make: (rng) => hero(kind, rng, seed, () => ({ x, y, turn })),
            });
          }
        break;
      }
      case "band": {
        if (band === null) break;
        for (let y2 = 2 * band.y0; y2 <= 2 * band.y1; y2++)
          for (let x2 = 2 * band.x0; x2 <= 2 * band.x1; x2++) {
            const seed = seedFor(room.seed, "hero", x2, y2, "band");
            const x = x2 / 2;
            const y = y2 / 2;
            list.push({
              seed,
              x,
              y,
              make: (rng) =>
                hero(kind, rng, seed, () => ({ x, y, turn: rng.int(0, 3) })),
            });
          }
        break;
      }
      case "corner":
        for (const zone of sites.zones.slice(0, 4)) {
          if (zone.spots.length !== 4) continue;
          for (const s of zone.spots) {
            const seed = seedFor(room.seed, "hero", s.cx, s.cy, "corner");
            const x = s.cx + 0.5;
            const y = s.cy + 0.5;
            const turn = faceCentre(hall, x, y);
            list.push({
              seed,
              x,
              y,
              make: (rng) => hero(kind, rng, seed, () => ({ x, y, turn })),
            });
          }
        }
        break;
      case "centre": {
        const seed = seedFor(room.seed, "hero", "centre");
        const x = (hall.x0 + hall.x1) / 2;
        list.push({
          seed,
          x,
          y: (hall.y0 + hall.y1) / 2,
          make: (rng) =>
            hero(kind, rng, seed, (variant) => ({
              x,
              y:
                (hall.y0 + hall.y1) / 2 -
                (FOOTPRINTS.hero[kind][variant]?.depth ?? 0) / 2 / CELL,
              turn: 2,
            })),
        });
        break;
      }
    }
    return list.sort((a, b) => a.seed - b.seed || a.y - b.y || a.x - b.x);
  };

  const tryPlace = (kind: HeroKind) => {
    for (const c of candidates(kind)) {
      const h = c.make(createRng(c.seed));
      if (fits(h)) {
        out.push(h);
        return;
      }
    }
  };

  if (draws.slab && out.length < cap) tryPlace("black-slab");
  if (draws.turret && out.length < cap) tryPlace("turret");
  const pool: readonly (readonly [HeroKind, number])[] =
    HERO_POOLS[room.archetype];
  for (const pick of draws.picks) {
    if (out.length >= cap) break;
    if (!pick.take) continue;
    const left = pool.filter(([k]) => !out.some((h) => h.kind === k));
    const kind = pickByRoll(pick.roll, left);
    if (kind !== null) tryPlace(kind);
  }
  return out.sort(HERO_ORDER);
}
