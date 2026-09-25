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
 * The surface hook (H21): `HERO_CATALOGUE[kind].surfaces` lists each top in
 * the hero's local terms, and `heroSurfaces` gives them in world metres, for
 * the small surface props of a later milestone.
 *
 * This is the generator side: it imports `footprints.ts`, `sites.ts`,
 * `types.ts` and `units.ts` (and may import `props.ts` and the seeds), and
 * never `move.ts`, `generate.ts`, `interact.ts`, `malfunction.ts` or
 * anything under `render/` (ruling 20). `heroes.test.ts`, `dress.test.ts`
 * and `sites.test.ts` keep it so.
 */

import {
  HERO_FOOTING,
  HERO_FRONT,
  heroFootprint,
  heroTurn,
} from "./footprints";
import { edgeKey, type Reserved } from "./sites";
import type {
  Archetype,
  Box,
  Hero,
  HeroKind,
  HeroPlacement,
  Rect,
  Side,
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

/**
 * A top a later pass may set a small prop on (H21), in the hero's local
 * terms at turn 0: `a` along its width, `d` along its depth (from the wall
 * point for a wall-anchored hero, from the centre for a free one), `h` the
 * height of the top in metres.
 */
export interface HeroSurfaceSpec {
  a0: number;
  a1: number;
  d0: number;
  d1: number;
  h: number;
}

/** One kind's entry: where it stands, how many variants, how tall, its edges and its tops. */
export interface HeroEntry {
  placement: HeroPlacement;
  variants: number;
  top: number;
  /** Wall edges a wall or backed kind takes: 1, or 2 for the core wall. */
  edges: 1 | 2;
  surfaces: readonly HeroSurfaceSpec[];
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
    use: null,
  },
  "black-slab": {
    placement: "centre",
    variants: 1,
    top: 2.7,
    edges: 1,
    surfaces: [],
    use: null,
  },
  "eye-panel": {
    placement: "wall",
    variants: 1,
    top: 2.2,
    edges: 1,
    surfaces: [],
    use: null,
  },
  "photo-console": {
    placement: "backed",
    variants: 1,
    top: 1.9,
    edges: 1,
    surfaces: [],
    use: null,
  },
  "laser-desk": {
    placement: "band",
    variants: 1,
    top: 2.2,
    edges: 1,
    surfaces: [{ a0: -1.1, a1: -0.35, d0: -0.2, d1: 0.4, h: 0.74 }],
    use: null,
  },
  "mess-table": {
    placement: "band",
    variants: 1,
    top: 1.1,
    edges: 1,
    surfaces: [{ a0: -2.1, a1: 0.9, d0: -0.4, d1: 0.4, h: 0.76 }],
    use: null,
  },
  "helper-robot": {
    placement: "band",
    variants: 1,
    top: 1.6,
    edges: 1,
    surfaces: [],
    use: null,
  },
  "sleep-ring": {
    placement: "band",
    variants: 1,
    top: 1.4,
    edges: 1,
    surfaces: [],
    use: null,
  },
  "dome-planters": {
    placement: "band",
    variants: 2,
    top: 1.5,
    edges: 1,
    surfaces: [],
    use: null,
  },
  "core-wall": {
    placement: "wall",
    variants: 1,
    top: 2.25,
    edges: 2,
    surfaces: [],
    use: null,
  },
  "gun-rack": {
    placement: "wall",
    variants: 1,
    top: 1.9,
    edges: 1,
    surfaces: [],
    use: null,
  },
  "gun-bench": {
    placement: "backed",
    variants: 1,
    top: 1.4,
    edges: 1,
    surfaces: [{ a0: -0.9, a1: -0.55, d0: 0.1, d1: 0.8, h: 0.9 }],
    use: null,
  },
  "tube-bench": {
    placement: "backed",
    variants: 1,
    top: 2.0,
    edges: 1,
    surfaces: [{ a0: -0.9, a1: -0.3, d0: 0.1, d1: 0.8, h: 0.9 }],
    use: null,
  },
  "field-pack": {
    placement: "corner",
    variants: 2,
    top: 1.6,
    edges: 1,
    surfaces: [],
    use: null,
  },
  "arcade-cabinet": {
    placement: "backed",
    variants: 3,
    top: 1.95,
    edges: 1,
    surfaces: [],
    use: { a: 0, d: 0.9 + HERO_USE_OUT },
  },
  "recruit-cabinet": {
    placement: "backed",
    variants: 1,
    top: 2.0,
    edges: 1,
    surfaces: [],
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
 * The hero's tops (H21) in world metres, each with its height `h`. A local
 * point `(a, d)` lies at `anchor * CELL + along * a + front * d`, with
 * `front = HERO_FRONT[turn]` and `along = [-front[1], front[0]]` (`[1, 0]`
 * at turn 0, turning clockwise with the hero); each top's box spans the
 * extremes of its four corners.
 */
export function heroSurfaces(h: Hero): { box: Box; h: number }[] {
  const [fx, fz] = HERO_FRONT[heroTurn(h)] ?? [0, -1];
  const ax = -fz;
  const az = fx;
  const ox = h.x * CELL;
  const oz = h.y * CELL;
  const specs: readonly HeroSurfaceSpec[] = HERO_CATALOGUE[h.kind].surfaces;
  return specs.map((s) => {
    const xs: number[] = [];
    const zs: number[] = [];
    for (const a of [s.a0, s.a1])
      for (const d of [s.d0, s.d1]) {
        xs.push(ox + ax * a + fx * d);
        zs.push(oz + az * a + fz * d);
      }
    return {
      box: {
        x0: Math.min(...xs),
        x1: Math.max(...xs),
        z0: Math.min(...zs),
        z1: Math.max(...zs),
      },
      h: s.h,
    };
  });
}

/** The heroes' order in `RoomSpec.heroes`: by `y`, then `x`, then kind by code point. */
export const HERO_ORDER = (a: Hero, b: Hero): number =>
  a.y - b.y || a.x - b.x || (a.kind < b.kind ? -1 : a.kind > b.kind ? 1 : 0);
