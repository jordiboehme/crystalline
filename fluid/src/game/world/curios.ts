/**
 * The curios (2.6b): small, rare things on a room's desks, benches, tables
 * and shelves, and a few under its desks. They are pure decoration like the
 * set dressing and the hero props, never data, and a third family beside
 * both, with their own kinds (`CurioKind`), their own list
 * (`RoomSpec.curios`) and this module. The rulings, in short:
 *
 * - **C1. Their own family.** A curio is `{ kind, variant, x, y, h, turn,
 *   seed }`: its box's centre in cell units, the height `h` of the surface
 *   it stands on in metres, a quarter turn and the seed it was accepted on.
 *   It is neither a prop nor a hero, so the prop cap, the prop footprints
 *   and the hero moats never see it. It never collides, reserves nothing
 *   and is not counted by the density.
 * - **C2. Last, reading only.** The pass runs last in `generateRoom`, after
 *   the dressing, on a `CurioBase` (the room less its curios). It reads the
 *   fixtures, decor, heroes and props and changes none of them.
 * - **C3. Catalogued surfaces.** Every host kind's tops and under spots are
 *   rectangles in its local terms at turn 0 (`SurfaceSpec`): the fixtures'
 *   (`FIXTURE_SURFACES`), the decor's (`DECOR_SURFACES`), the floor props'
 *   per variant (`PROP_SURFACES`) and the heroes' (`HERO_CATALOGUE[kind]
 *   .surfaces` and `.under`). Each has its height, its free height `clear`
 *   (`OPEN_CLEAR` for an open top) and its class. Nothing is read off a
 *   mesh at run time.
 * - **C4. One transform.** Every local point goes to the world through
 *   `turnedPoint` (`footprints.ts`), a wall host from its slot's wall
 *   anchor and a free host from its centre (`hostSurfaces`).
 * - **C5. No blocker of their own.** Every surface lies inside its host's
 *   footprint, which already blocks the player, so a curio changes no
 *   reach.
 * - **C6. Four slots.** A room draws each of the `retro`, `gear`, `ball` and
 *   `under` slots once, rolling only over the kinds that fit it, so it
 *   holds at most one curio per slot.
 * - **C7. Rates.** Each slot's chance (`RETRO_SHARE`, `GEAR_SHARE`,
 *   `BALL_SHARE`, `UNDER_SHARE`) and its weighted pool (`RETRO_POOL`,
 *   `GEAR_POOLS` per archetype, `BALL_POOL`, `UNDER_POOL`); a drawn ball
 *   tries the hall's corners at `BALL_FLOOR`.
 * - **C8. The laptop is too big for a terminal end** and stands on the
 *   wider tops only; nothing is widened for it.
 * - **C9. Seeds by anchor.** A candidate's seed is made from its host's
 *   anchor ints and a token (`HostSurface.key`), never from a position in a
 *   list.
 * - **C10. Facing.** A kind is `fixed` (faces its host's front), `front`
 *   (never faces the wall) or `any`.
 * - **C11. Goldens.** Only the `curios` key of a golden ever moves for them.
 * - **C12. Heights.** A curio's top stays under its surface's `clear` and
 *   `CURIO_CEILING_GAP` under the ceiling.
 *
 * This is the generator side: it imports `footprints.ts`, `heroes.ts`,
 * `sites.ts`, `types.ts` and `units.ts`, and never `move.ts`,
 * `generate.ts`, `interact.ts`, `malfunction.ts` or anything under
 * `render/` (ruling 20). `sites.test.ts` and `dress.test.ts` keep it so.
 */

import { HERO_FOOTING, OPEN_CLEAR, heroTurn, turnedBox } from "./footprints";
import { heroSurfaces, heroUnder } from "./heroes";
import { wallAnchor, type CurioBase } from "./sites";
import type {
  Archetype,
  Box,
  Curio,
  CurioKind,
  DecorKind,
  FloorPropKind,
  MachineKind,
  PropKind,
  SurfaceClass,
  SurfaceSpec,
} from "./types";
import { CELL } from "./units";

/** Every curio kind, once each, in catalogue order. */
export const CURIO_KINDS: readonly CurioKind[] = [
  "light-sword",
  "green-pistol",
  "pink-gadget",
  "wing-meter",
  "pocket-console",
  "tape-drive",
  "tape-player",
  "video-tape",
  "beige-laptop",
  "star-ball",
  "catch-ball",
  "trap-box",
  "fuel-case",
];

/** Which of a room's four draws a kind comes from (C6). */
export type CurioSlot = "retro" | "gear" | "ball" | "under";

/** One variant's box in metres at turn 0: `width` along `a`, `depth` along `d`, `top` above its base. */
export interface CurioSize {
  width: number;
  depth: number;
  top: number;
}

/**
 * One kind's entry: its slot, its variants' sizes, the surface classes it
 * stands on and how it turns (C10).
 */
export interface CurioEntry {
  slot: CurioSlot;
  variants: number;
  sizes: readonly CurioSize[];
  classes: readonly SurfaceClass[];
  facing: "fixed" | "front" | "any";
}

/**
 * Every curio kind (C19): its slot (C6), its variant count and one size per
 * variant, the surface classes it stands on (C3) and its facing (C10). No
 * size is wider or deeper than 0.65 m. The sword's three variants are the
 * blade lying in a small cradle, then the lit blade upright in its stand in
 * blue and in green (C13); the gadget's are one alone and a cluster; the
 * video tape's are one in its sleeve and one lying bare beside it.
 *
 * It is typed as the whole record rather than `as const`, so an entry read
 * by a `CurioKind` is a plain `CurioEntry` (`classes.includes` then takes
 * any class, which a union of literal tuples would refuse).
 */
export const CURIO_CATALOGUE: Readonly<Record<CurioKind, CurioEntry>> = {
  "light-sword": {
    slot: "gear",
    variants: 3,
    facing: "any",
    classes: ["desk", "bench", "table", "shelf"],
    sizes: [
      { width: 0.3, depth: 0.1, top: 0.08 },
      { width: 0.14, depth: 0.14, top: 1.22 },
      { width: 0.14, depth: 0.14, top: 1.22 },
    ],
  },
  "green-pistol": {
    slot: "gear",
    variants: 1,
    facing: "any",
    classes: ["desk", "bench"],
    sizes: [{ width: 0.26, depth: 0.1, top: 0.17 }],
  },
  "pink-gadget": {
    slot: "gear",
    variants: 2,
    facing: "any",
    classes: ["shelf", "desk", "bench"],
    sizes: [
      { width: 0.12, depth: 0.12, top: 0.23 },
      { width: 0.36, depth: 0.16, top: 0.23 },
    ],
  },
  "wing-meter": {
    slot: "gear",
    variants: 1,
    facing: "any",
    classes: ["desk", "bench"],
    sizes: [{ width: 0.32, depth: 0.1, top: 0.3 }],
  },
  "pocket-console": {
    slot: "retro",
    variants: 1,
    facing: "front",
    classes: ["desk", "bench", "table", "shelf"],
    sizes: [{ width: 0.1, depth: 0.07, top: 0.155 }],
  },
  "tape-drive": {
    slot: "retro",
    variants: 1,
    facing: "front",
    classes: ["desk", "bench", "table", "shelf"],
    sizes: [{ width: 0.2, depth: 0.15, top: 0.055 }],
  },
  "tape-player": {
    slot: "retro",
    variants: 1,
    facing: "front",
    classes: ["desk", "bench", "table", "shelf"],
    sizes: [{ width: 0.32, depth: 0.16, top: 0.035 }],
  },
  "video-tape": {
    slot: "retro",
    variants: 2,
    facing: "front",
    classes: ["desk", "bench", "table", "shelf"],
    sizes: [
      { width: 0.2, depth: 0.11, top: 0.03 },
      { width: 0.42, depth: 0.12, top: 0.03 },
    ],
  },
  "beige-laptop": {
    slot: "retro",
    variants: 1,
    facing: "fixed",
    classes: ["desk", "table", "bench"],
    sizes: [{ width: 0.41, depth: 0.4, top: 0.33 }],
  },
  "star-ball": {
    slot: "ball",
    variants: 1,
    facing: "any",
    classes: ["desk", "shelf", "table"],
    sizes: [{ width: 0.08, depth: 0.08, top: 0.075 }],
  },
  "catch-ball": {
    slot: "ball",
    variants: 1,
    facing: "any",
    classes: ["desk", "shelf", "table"],
    sizes: [{ width: 0.08, depth: 0.08, top: 0.075 }],
  },
  "trap-box": {
    slot: "under",
    variants: 1,
    facing: "fixed",
    classes: ["under"],
    sizes: [{ width: 0.6, depth: 0.3, top: 0.18 }],
  },
  "fuel-case": {
    slot: "under",
    variants: 1,
    facing: "fixed",
    classes: ["under"],
    sizes: [{ width: 0.4, depth: 0.28, top: 0.24 }],
  },
};

/** The retro slot's pool (C7): the laptop now and then, the rest rarer. Order is part of the seeded result. */
export const RETRO_POOL = [
  ["beige-laptop", 4],
  ["pocket-console", 2],
  ["tape-drive", 2],
  ["tape-player", 2],
  ["video-tape", 2],
] as const satisfies readonly (readonly [CurioKind, number])[];

/** The gear slot's pool per archetype (C7). Order is part of the seeded result. */
export const GEAR_POOLS = {
  bridge: [
    ["light-sword", 2],
    ["wing-meter", 1],
    ["green-pistol", 1],
  ],
  council: [
    ["light-sword", 2],
    ["pink-gadget", 1],
  ],
  engineering: [
    ["light-sword", 1],
    ["green-pistol", 1],
    ["pink-gadget", 2],
    ["wing-meter", 2],
  ],
  archive: [
    ["light-sword", 2],
    ["wing-meter", 1],
  ],
  lab: [
    ["light-sword", 1],
    ["green-pistol", 3],
    ["pink-gadget", 2],
    ["wing-meter", 2],
  ],
} as const satisfies Record<
  Archetype,
  readonly (readonly [CurioKind, number])[]
>;

/** The ball slot's pool: the two balls alike. */
export const BALL_POOL = [
  ["star-ball", 1],
  ["catch-ball", 1],
] as const satisfies readonly (readonly [CurioKind, number])[];
/** The under slot's pool: the trap and the case alike. */
export const UNDER_POOL = [
  ["trap-box", 1],
  ["fuel-case", 1],
] as const satisfies readonly (readonly [CurioKind, number])[];

/** Chance a room draws a retro curio (C7). */
export const RETRO_SHARE = 1 / 6;
/** Chance a room draws a gear curio (C7). */
export const GEAR_SHARE = 1 / 8;
/** Chance a room draws a ball (spec: about 1 room in 30). */
export const BALL_SHARE = 1 / 30;
/** Chance a drawn ball tries the hall's corners instead of the surfaces (C7). */
export const BALL_FLOOR = 1 / 4;
/** Chance a room draws an under-desk curio (C7). */
export const UNDER_SHARE = 1 / 10;
/** How far a curio's box keeps inside its surface's edges, in metres. */
export const CURIO_MARGIN = 0.02;
/** The least gap between two curios at the same height, in metres. */
export const CURIO_GAP = 0.03;
/** How far under the ceiling a curio's top stays, in metres (C12). */
export const CURIO_CEILING_GAP = 0.3;

/**
 * The tops and under spots of the fixtures that have any (C3), local to
 * their wall slot (`d` from the wall): a terminal's two desk ends beside
 * its screen and key deck and its knee space, a workbench's top and lower
 * shelf, and a lab bench's two clear ends. The terminal's ends are 0.22 m
 * wide, too narrow for the laptop (C8).
 */
export const FIXTURE_SURFACES = {
  terminal: [
    {
      a0: -0.66,
      a1: -0.44,
      d0: 0.04,
      d1: 0.56,
      h: 0.78,
      clear: OPEN_CLEAR,
      cls: "desk",
    },
    {
      a0: 0.44,
      a1: 0.66,
      d0: 0.04,
      d1: 0.56,
      h: 0.78,
      clear: OPEN_CLEAR,
      cls: "desk",
    },
    { a0: -0.48, a1: 0.28, d0: 0.06, d1: 0.46, h: 0, clear: 0.7, cls: "under" },
  ],
  machine: {
    workbench: [
      {
        a0: -0.84,
        a1: 0.5,
        d0: 0.08,
        d1: 0.78,
        h: 0.9,
        clear: OPEN_CLEAR,
        cls: "bench",
      },
      {
        a0: -0.78,
        a1: 0.78,
        d0: 0.1,
        d1: 0.7,
        h: 0.22,
        clear: 0.6,
        cls: "under",
      },
    ],
    "lab-bench": [
      {
        a0: -0.86,
        a1: -0.57,
        d0: 0.24,
        d1: 0.76,
        h: 0.94,
        clear: OPEN_CLEAR,
        cls: "bench",
      },
      {
        a0: 0.68,
        a1: 0.86,
        d0: 0.24,
        d1: 0.76,
        h: 0.94,
        clear: OPEN_CLEAR,
        cls: "bench",
      },
    ],
  },
} as const satisfies {
  terminal: readonly SurfaceSpec[];
  machine: Partial<Record<MachineKind, readonly SurfaceSpec[]>>;
};

/**
 * The tops of the decor that has any (C3), local to the piece's centre: the
 * lab island's clear stretch between its hood and its sink, and the round
 * table's four places around its glowing disc.
 */
export const DECOR_SURFACES = {
  "lab-island": [
    {
      a0: -0.15,
      a1: 0.5,
      d0: -0.65,
      d1: 0.65,
      h: 0.96,
      clear: OPEN_CLEAR,
      cls: "bench",
    },
  ],
  "round-table": [
    {
      a0: -0.225,
      a1: 0.225,
      d0: -1.0,
      d1: -0.55,
      h: 0.78,
      clear: OPEN_CLEAR,
      cls: "table",
    },
    {
      a0: 0.55,
      a1: 1.0,
      d0: -0.225,
      d1: 0.225,
      h: 0.78,
      clear: OPEN_CLEAR,
      cls: "table",
    },
    {
      a0: -0.225,
      a1: 0.225,
      d0: 0.55,
      d1: 1.0,
      h: 0.78,
      clear: OPEN_CLEAR,
      cls: "table",
    },
    {
      a0: -1.0,
      a1: -0.55,
      d0: -0.225,
      d1: 0.225,
      h: 0.78,
      clear: OPEN_CLEAR,
      cls: "table",
    },
  ],
} as const satisfies Partial<Record<DecorKind, readonly SurfaceSpec[]>>;

/**
 * The shelf levels and tops of the floor props that have any (C3), per
 * variant, local to the prop's centre: the storage shelf's open top (v0)
 * or its three levels, each `clear` up to the next level's underside, and
 * its top (v1), and each filing cabinet's top.
 */
export const PROP_SURFACES = {
  "storage-shelf": [
    [
      {
        a0: -0.55,
        a1: 0.55,
        d0: -0.2,
        d1: 0.2,
        h: 1.6,
        clear: OPEN_CLEAR,
        cls: "shelf",
      },
    ],
    [0.72, 1.08, 1.44]
      .map((h) => ({
        a0: -0.45,
        a1: 0.45,
        d0: -0.2,
        d1: 0.2,
        h,
        clear: 0.34,
        cls: "shelf" as const,
      }))
      .concat([
        {
          a0: -0.45,
          a1: 0.45,
          d0: -0.2,
          d1: 0.2,
          h: 1.8,
          clear: OPEN_CLEAR,
          cls: "shelf",
        },
      ]),
  ],
  "filing-cabinet": [
    [
      {
        a0: -0.2,
        a1: 0.2,
        d0: -0.28,
        d1: 0.28,
        h: 0.8,
        clear: OPEN_CLEAR,
        cls: "shelf",
      },
    ],
    [
      {
        a0: -0.2,
        a1: 0.2,
        d0: -0.28,
        d1: 0.28,
        h: 1.4,
        clear: OPEN_CLEAR,
        cls: "shelf",
      },
    ],
  ],
} satisfies Partial<Record<FloorPropKind, readonly (readonly SurfaceSpec[])[]>>;

/**
 * One surface of one host in world terms: `host` names the host's table
 * entry (`terminal`, `machine:<kind>`, `decor:<kind>`, `prop:<kind>`,
 * `hero:<kind>`), `anchorOf` is the host itself, `box` the surface in
 * world metres, `h`, `clear` and `cls` from its spec, `turn` the host's
 * quarter turn, `free` whether the host stands free (decor, floor prop or
 * free hero) rather than against a wall, and `key` the anchor ints and
 * token its candidates' seeds are made from (C9).
 */
export interface HostSurface {
  host: string;
  anchorOf: object;
  box: Box;
  h: number;
  clear: number;
  cls: SurfaceClass;
  turn: number;
  free: boolean;
  key: readonly [number, number, string];
}

/**
 * Every surface of every host in `room`, in world terms (C3, C4), in this
 * order, which later breaks the curio pass's seed ties:
 *
 * 1. the fixtures, in `room.fixtures` order: a terminal, or a machine whose
 *    kind has a `FIXTURE_SURFACES.machine` entry, anchored at its slot's
 *    `wallAnchor`, keyed `[slot.x, slot.y, "<side>-<host>-<j>"]`;
 * 2. the decor with a `DECOR_SURFACES` entry, anchored at its centre and
 *    turn, keyed `[round(2x), round(2y), "decor-<kind>-<j>"]`;
 * 3. the heroes, one at a time, each hero's tops (`heroSurfaces`, keyed
 *    `"hero-<kind>-<j>"`) and then its under spots (`heroUnder`, keyed
 *    `"hero-<kind>-under-<j>"`), at `[round(2x), round(2y)]`;
 * 4. the floor props with a `PROP_SURFACES` entry for their variant,
 *    anchored at their centre and turn, keyed `[round(2x), round(2y),
 *    "prop-<kind>-<j>"]`.
 *
 * `j` is the surface's index in its host's table. Every rectangle's four
 * corners go through `turnedPoint` (`turnedBox`), and its box spans their
 * extremes.
 */
export function hostSurfaces(room: CurioBase): HostSurface[] {
  const out: HostSurface[] = [];
  const machines: Partial<Record<MachineKind, readonly SurfaceSpec[]>> =
    FIXTURE_SURFACES.machine;
  const decorTable: Partial<Record<DecorKind, readonly SurfaceSpec[]>> =
    DECOR_SURFACES;
  const propTable: Partial<
    Record<PropKind, readonly (readonly SurfaceSpec[])[]>
  > = PROP_SURFACES;

  const push = (
    host: string,
    anchorOf: object,
    at: { x: number; y: number; turn: number },
    free: boolean,
    ints: readonly [number, number],
    token: (j: number) => string,
    specs: readonly SurfaceSpec[],
  ) => {
    for (const [j, s] of specs.entries())
      out.push({
        host,
        anchorOf,
        box: turnedBox(at.x, at.y, at.turn, s),
        h: s.h,
        clear: s.clear,
        cls: s.cls,
        turn: at.turn,
        free,
        key: [ints[0], ints[1], token(j)],
      });
  };

  for (const f of room.fixtures) {
    let host: string;
    let specs: readonly SurfaceSpec[] | undefined;
    if (f.kind === "terminal") {
      host = "terminal";
      specs = FIXTURE_SURFACES.terminal;
    } else if (f.kind === "machine") {
      host = `machine:${f.machine}`;
      specs = machines[f.machine];
    } else continue;
    if (specs === undefined) continue;
    const slot = f.slot;
    push(
      host,
      f,
      wallAnchor(slot),
      false,
      [slot.x, slot.y],
      (j) => `${slot.side}-${host}-${String(j)}`,
      specs,
    );
  }

  for (const d of room.decor) {
    const specs = decorTable[d.kind];
    if (specs === undefined) continue;
    push(
      `decor:${d.kind}`,
      d,
      d,
      true,
      [Math.round(2 * d.x), Math.round(2 * d.y)],
      (j) => `decor-${d.kind}-${String(j)}`,
      specs,
    );
  }

  for (const h of room.heroes) {
    const turn = heroTurn(h);
    const free = HERO_FOOTING[h.kind] === "free";
    const ints = [Math.round(2 * h.x), Math.round(2 * h.y)] as const;
    const lists = [
      [heroSurfaces(h), (j: number) => `hero-${h.kind}-${String(j)}`],
      [heroUnder(h), (j: number) => `hero-${h.kind}-under-${String(j)}`],
    ] as const;
    for (const [rects, token] of lists)
      for (const [j, r] of rects.entries())
        out.push({
          host: `hero:${h.kind}`,
          anchorOf: h,
          box: r.box,
          h: r.h,
          clear: r.clear,
          cls: r.cls,
          turn,
          free,
          key: [ints[0], ints[1], token(j)],
        });
  }

  for (const p of room.props) {
    if (p.anchor !== "floor") continue;
    const specs = propTable[p.kind]?.[p.variant];
    if (specs === undefined) continue;
    push(
      `prop:${p.kind}`,
      p,
      p,
      true,
      [Math.round(2 * p.x), Math.round(2 * p.y)],
      (j) => `prop-${p.kind}-${String(j)}`,
      specs,
    );
  }
  return out;
}

/**
 * A curio's variant size from `CURIO_CATALOGUE`. Throws on a variant index
 * out of range for its kind, in the words `propFootprint` uses: a
 * generator bug should not pass silently.
 */
export function curioSize(c: Curio): CurioSize {
  const sizes: readonly CurioSize[] = CURIO_CATALOGUE[c.kind].sizes;
  const size = sizes[c.variant];
  if (size === undefined)
    throw new Error(`curioSize: ${c.kind} has no variant ${String(c.variant)}`);
  return size;
}

/**
 * A curio's plan box, in metres: its variant's size centred on
 * `(c.x * CELL, c.y * CELL)`, width and depth swapped at an odd turn.
 */
export function curioBox(c: Curio): Box {
  const size = curioSize(c);
  const sideways = (((Math.round(c.turn) % 4) + 4) % 4) % 2 === 1;
  const hx = (sideways ? size.depth : size.width) / 2;
  const hz = (sideways ? size.width : size.depth) / 2;
  const cx = c.x * CELL;
  const cz = c.y * CELL;
  return { x0: cx - hx, x1: cx + hx, z0: cz - hz, z1: cz + hz };
}

/** The curios' order in `RoomSpec.curios`: by `y`, then `x`, then `h`, then kind by code point. */
export const CURIO_ORDER = (a: Curio, b: Curio): number =>
  a.y - b.y ||
  a.x - b.x ||
  a.h - b.h ||
  (a.kind < b.kind ? -1 : a.kind > b.kind ? 1 : 0);
