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
 *   mesh at run time. An `under` spot is catalogued only where a standing
 *   player can see it: under an open-fronted desk, table, bench or trough
 *   (the workbench's lower shelf, the hydroponics trough, the round
 *   table's rim, the bench's seat) and a hero's `under` spots. The class
 *   names the curio slot, not the geometry, so it is not always a spot
 *   tucked beneath something: the service trolley's own deck top is an
 *   `under` spot too, the trap and the case riding it in the open the way
 *   a curio never does anywhere else in the catalogue. A terminal carries
 *   no `under` spot at all: its knee space holds the swivel chair, between
 *   the wall and every spot a player can stand on, so a curio there could
 *   never be seen.
 * - **C4. One transform.** Every local point goes to the world through
 *   `turnedPoint` (`footprints.ts`), a wall host from its slot's wall
 *   anchor and a free host from its centre (`hostSurfaces`).
 * - **C5. No blocker of their own.** Every surface lies inside its host's
 *   footprint, which already blocks the player, so a curio changes no
 *   reach.
 * - **C6. Seven slots.** A room draws each of the `retro`, `gear`, `ball`,
 *   `under`, `tech`, `radar` and `capsule` slots once, rolling only over
 *   the kinds that fit it, so it holds at most one curio per slot. The two
 *   computers and the space bricks share the retro slot, so the two
 *   computers never stand in one generated room; the reactor case and the
 *   drone share the tech slot. The radar and the capsule case each have a
 *   slot of their own, so either can stand beside a ball.
 * - **C7. Rates.** Each slot's chance (`RETRO_SHARE`, `GEAR_SHARE`,
 *   `BALL_SHARE`, `UNDER_SHARE`, `TECH_SHARE`, `RADAR_SHARE`,
 *   `CAPSULE_SHARE`) and its weighted pool (`RETRO_POOLS` and
 *   `GEAR_POOLS` and `TECH_POOLS` per archetype, `BALL_POOL`,
 *   `UNDER_POOL`, `RADAR_POOL`, `CAPSULE_POOL`); a drawn ball tries the
 *   hall's corners at `BALL_FLOOR`. The radar takes at
 *   `RADAR_BESIDE_BALL` in a room that holds a star ball, and the capsule
 *   case at `CAPSULE_BESIDE` in a room that holds a star ball or the
 *   radar (2.6f C4: each share 1.5 times its 2.6b and 2.6d share).
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
 * - **2.6f C7, C8.** The neighbours' skip: a pick that a room's
 *   neighbours draw raw is made again from the other fitting kinds, and a
 *   one-kind slot is dropped for a lower-seeded neighbour that draws it
 *   (`Near` in `sites.ts`, step 3 below).
 * - **The lift.** One curio hovers: the drone's mesh starts `CURIO_LIFT`
 *   over its surface (`curioLift`). Its box still runs from the surface
 *   to its top for the fit, the ceiling and the clash, so nothing stands
 *   under it; framing aims at its middle (`curioMid`).
 *
 * The pass (`placeCurios`), in order:
 *
 * 1. **Draws.** `curioDraws(room)`, which is `curioDrawsOf(room.seed)`: one
 *    stream of `seedFor(seed, "curio", "draw")` draws the retro slot's
 *    chance (`RETRO_SHARE`) and roll, the gear slot's (`GEAR_SHARE`), the
 *    ball slot's (`BALL_SHARE`) and its floor chance (`BALL_FLOOR`), the
 *    under slot's (`UNDER_SHARE`) and the tech slot's (`TECH_SHARE`), then
 *    one uniform and a roll each for the radar and the capsule case, the
 *    uniform giving both the slot's own chance and its paired chance.
 *    Everything is always drawn, and the later slots come after every
 *    earlier value, so no earlier draw moves. `curioDrawsOf` reads nothing
 *    of a room but its seed, so a neighbour's raw curios (`rawCurios`, 2.6f
 *    C6) are made from its seed alone: it steps through the same slots,
 *    picking each from its whole pool (`poolOf`) with no fit filter and no
 *    layout, leaving out a slot whose pool is the archetype's own
 *    (`ARCHETYPE_SLOTS`) when the archetype is unknown.
 * 2. **Surfaces.** `hostSurfaces(room)`, once.
 * 3. **Slots, in order: retro, gear, ball, under, tech, radar, capsule.**
 *    A slot that takes (`slotTakes`: its own chance, or for the radar and
 *    the capsule case their paired chance when a curio placed before it
 *    in this pass is a partner, a star ball for the radar, a star ball or
 *    the radar for the capsule case) takes its pool
 *    (`RETRO_POOLS[room.archetype]`, `GEAR_POOLS[room.archetype]`,
 *    `BALL_POOL`, `UNDER_POOL`, `TECH_POOLS[room.archetype]`,
 *    `RADAR_POOL`, `CAPSULE_POOL`, or only the kind its draw forces),
 *    keeps the kinds with a candidate that fits right now, picks one by
 *    `pickByRoll` and places it, skipping its neighbours' kinds (2.6f C7,
 *    C8): when a neighbour draws the picked kind raw (`near.curios`), the
 *    same roll picks again from the fitting kinds less every neighbour's
 *    (`skipNear`, all of them when that leaves none), and a slot whose
 *    pool is one kind is not taken when a lower-seeded neighbour draws
 *    that kind (`near.curiosBelow`). A forced draw reads neither. A slot
 *    with no fitting kind stays empty.
 *    A floor ball that fits no corner falls back to the surfaces: the
 *    slot runs again with the same roll, so the ball rate stays
 *    `BALL_SHARE`. Nothing is drawn again.
 * 4. **Candidates of a kind.** A floor ball (the ball slot when it drew
 *    its floor chance) takes the hall's corners (`cornerSpots`); any other
 *    kind takes every host surface of one of its classes, and a
 *    `floorOnly` kind only those of `h` 0. A candidate's seed is
 *    `seedFor(room.seed, "curio", ...key)` of its surface, or
 *    `seedFor(room.seed, "curio", cx, cy, "floor")` for a corner (C9).
 *    A kind that `prefers` a host tries that host's surfaces first; then
 *    they are tried in seed order, ties by their order in `hostSurfaces`
 *    or among the corners.
 * 5. **A trial.** The candidate's own stream draws the variant (the pink
 *    gadget's cluster only in a lab or an engineering room), a turn roll
 *    and the fractions `u` and `v`, all of them always. The facing turns
 *    the curio (C10, `curioTurn`), and `u` and `v` place its box in the
 *    surface's slack inside `CURIO_MARGIN`; a floor ball stands at its
 *    corner point at the turn roll. The first candidate that fits
 *    (step 6) and clashes with no curio placed before (step 7) is the
 *    curio, with its candidate's seed.
 * 6. **Fitting a surface** (`curioFits`, C12): the variant stands on the
 *    surface's class (`curioClasses`: the upright lit swords never on a
 *    shelf class top), a `floorOnly` kind only at `h` 0, its box lies
 *    inside the surface's box less `CURIO_MARGIN`, its top is at most the
 *    surface's `clear`, and its top stays `CURIO_CEILING_GAP` under the
 *    ceiling.
 * 7. **Clashing** (`curiosClash`): two curios whose plan boxes, one grown
 *    by `CURIO_GAP`, overlap and whose height ranges overlap.
 * 8. **Corner spots** (`cornerSpots`): the hall's four corner cells, each
 *    with the point 0.12 m in from both hall walls, where the cell is
 *    floor, both hall wall edges are free, carry no wall prop and are no
 *    hero's edge, and the ball's box keeps clear of every lane, taken box,
 *    floor prop and hero. Only this step reads `dressingSites`.
 * 9. **The output** is sorted by `CURIO_ORDER`.
 *
 * `curioOn` makes one curio on a given surface the same way, for the
 * hand-built rooms.
 *
 * This is the generator side: it imports `core/seed.ts`, `footprints.ts`,
 * `heroes.ts`, `layout.ts`, `sites.ts`, `types.ts` and `units.ts`, and
 * never `move.ts`, `generate.ts`, `interact.ts`, `malfunction.ts` or
 * anything under `render/` (ruling 20). `sites.test.ts` and
 * `dress.test.ts` keep it so.
 */

import { createRng, seedFor } from "../core/seed";
import {
  HERO_FOOTING,
  OPEN_CLEAR,
  heroFootprint,
  heroTurn,
  propFootprint,
  turnedBox,
} from "./footprints";
import { heroEdges, heroSurfaces, heroUnder } from "./heroes";
import { isFloor } from "./layout";
import {
  EPS,
  dressingSites,
  edgeKey,
  edgeOf,
  grow,
  NO_NEAR,
  overlaps,
  pickByRoll,
  round3,
  skipNear,
  wallAnchor,
  type CurioBase,
  type Near,
} from "./sites";
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
  "treasure-radar",
  "capsule-case",
  "reactor-case",
  "hover-drone",
  "breadbin-computer",
  "slim-computer",
  "space-bricks",
  "soot-puffs",
];

/** Which of a room's seven draws a kind comes from (C6, 2.6d C5). */
export type CurioSlot =
  "retro" | "gear" | "ball" | "under" | "tech" | "radar" | "capsule";

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
  /**
   * Per variant, the classes that variant keeps of `classes`, where a
   * variant stands on fewer: the upright lit swords never go on a shelf
   * class top. A kind without it stands every variant on `classes`.
   */
  variantClasses?: readonly (readonly SurfaceClass[])[];
  facing: "fixed" | "front" | "any";
  /**
   * Set on a kind that stands only at floor level (2.6d C7): of its
   * classes' surfaces it keeps those of `h` 0, in the pass
   * (`candidatesOf`) and in the fit (`curioFits`) alike, so it never rides
   * a trolley deck, a workbench's lower shelf or a bench's shelf.
   */
  floorOnly?: true;
  /**
   * A host name (`HostSurface.host`, such as `"terminal"`) whose surfaces
   * the kind tries first (2.6d C8): its candidates on that host come
   * before every other, each group in seed order. A kind without it tries
   * every candidate in seed order alone.
   */
  prefers?: string;
}

/**
 * Every curio kind (C19, 2.6d C3): its slot (C6), its variant count and one
 * size per variant, the surface classes it stands on (C3), its facing
 * (C10), and for the soot puffs `floorOnly` and for the drone `prefers`. No
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
    // Only the lying sword in its cradle goes on a shelf or a cabinet; the
    // lit blade upright in its stand stands on a desk, a bench or a table.
    variantClasses: [
      ["desk", "bench", "table", "shelf"],
      ["desk", "bench", "table"],
      ["desk", "bench", "table"],
    ],
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
  "treasure-radar": {
    slot: "radar",
    variants: 1,
    facing: "front",
    classes: ["desk", "bench", "table", "shelf"],
    sizes: [{ width: 0.085, depth: 0.1, top: 0.03 }],
  },
  "capsule-case": {
    slot: "capsule",
    variants: 1,
    facing: "front",
    classes: ["desk", "bench", "table", "shelf"],
    sizes: [{ width: 0.2, depth: 0.14, top: 0.155 }],
  },
  "reactor-case": {
    slot: "tech",
    variants: 1,
    facing: "front",
    classes: ["desk", "bench", "table", "shelf"],
    sizes: [{ width: 0.16, depth: 0.16, top: 0.2 }],
  },
  "hover-drone": {
    slot: "tech",
    variants: 1,
    facing: "front",
    classes: ["desk", "bench", "table"],
    prefers: "terminal",
    sizes: [{ width: 0.18, depth: 0.18, top: 0.53 }],
  },
  "breadbin-computer": {
    slot: "retro",
    variants: 1,
    facing: "fixed",
    classes: ["desk", "bench", "table"],
    sizes: [{ width: 0.4, depth: 0.21, top: 0.075 }],
  },
  "slim-computer": {
    slot: "retro",
    variants: 1,
    facing: "fixed",
    classes: ["desk", "bench", "table"],
    sizes: [{ width: 0.41, depth: 0.25, top: 0.07 }],
  },
  "space-bricks": {
    slot: "retro",
    variants: 1,
    facing: "front",
    classes: ["desk", "bench", "table", "shelf"],
    sizes: [{ width: 0.25, depth: 0.15, top: 0.09 }],
  },
  "soot-puffs": {
    slot: "under",
    variants: 2,
    facing: "front",
    classes: ["under"],
    floorOnly: true,
    sizes: [
      { width: 0.22, depth: 0.14, top: 0.07 },
      { width: 0.34, depth: 0.2, top: 0.08 },
    ],
  },
};

/**
 * The retro slot's base pool (C7): the laptop now and then, the rest
 * rarer; `RETRO_POOLS` builds on it. Order is part of the seeded result.
 */
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
/** The under slot's pool: the trap, the case and the puffs alike. */
export const UNDER_POOL = [
  ["trap-box", 1],
  ["fuel-case", 1],
  ["soot-puffs", 1],
] as const satisfies readonly (readonly [CurioKind, number])[];

/**
 * The retro slot's pool per archetype (2.6d C6): `RETRO_POOL`, then the
 * breadbin computer, the slim computer and the space bricks. The old kinds
 * keep their order and weights, so a room where no new kind fits keeps its
 * pick. Order is part of the seeded result.
 */
export const RETRO_POOLS = {
  bridge: [
    ...RETRO_POOL,
    ["breadbin-computer", 2],
    ["slim-computer", 2],
    ["space-bricks", 2],
  ],
  council: [
    ...RETRO_POOL,
    ["breadbin-computer", 1],
    ["slim-computer", 1],
    ["space-bricks", 1],
  ],
  engineering: [
    ...RETRO_POOL,
    ["breadbin-computer", 1],
    ["slim-computer", 1],
    ["space-bricks", 1],
  ],
  archive: [
    ...RETRO_POOL,
    ["breadbin-computer", 2],
    ["slim-computer", 2],
    ["space-bricks", 1],
  ],
  lab: [
    ...RETRO_POOL,
    ["breadbin-computer", 1],
    ["slim-computer", 1],
    ["space-bricks", 2],
  ],
} as const satisfies Record<
  Archetype,
  readonly (readonly [CurioKind, number])[]
>;

/**
 * The tech slot's pool per archetype (2.6d C5, C9): the reactor case in lab
 * and engineering rooms, the drone everywhere.
 */
export const TECH_POOLS = {
  bridge: [["hover-drone", 1]],
  council: [["hover-drone", 1]],
  engineering: [
    ["reactor-case", 2],
    ["hover-drone", 1],
  ],
  archive: [["hover-drone", 1]],
  lab: [
    ["reactor-case", 2],
    ["hover-drone", 1],
  ],
} as const satisfies Record<
  Archetype,
  readonly (readonly [CurioKind, number])[]
>;

/** The radar slot's pool (2.6d C5). */
export const RADAR_POOL = [
  ["treasure-radar", 1],
] as const satisfies readonly (readonly [CurioKind, number])[];
/** The capsule slot's pool (2.6d C5). */
export const CAPSULE_POOL = [
  ["capsule-case", 1],
] as const satisfies readonly (readonly [CurioKind, number])[];

/** Chance a room draws a retro curio (C7, 2.6f C4): about 1 room in 4. */
export const RETRO_SHARE = 1 / 4;
/** Chance a room draws a gear curio (C7, 2.6f C4): about 3 in 16. */
export const GEAR_SHARE = 3 / 16;
/** Chance a room draws a ball (2.6f C4): about 1 room in 20. */
export const BALL_SHARE = 1 / 20;
/** Chance a drawn ball tries the hall's corners instead of the surfaces (C7). */
export const BALL_FLOOR = 1 / 4;
/** Chance a room draws an under-desk curio (C7, 2.6f C4): about 3 in 20. */
export const UNDER_SHARE = 3 / 20;
/** Chance a room draws the tech slot (2.6d C9, 2.6f C4): about 3 in 20. */
export const TECH_SHARE = 3 / 20;
/** Chance a room draws the radar (2.6d C9, 2.6f C4: about 1 room in 16)... */
export const RADAR_SHARE = 1 / 16;
/** ...and its chance in a room that holds a star ball (2.6d C5). */
export const RADAR_BESIDE_BALL = 1 / 2;
/** Chance a room draws the capsule case (2.6d C9, 2.6f C4: about 1 room in 16)... */
export const CAPSULE_SHARE = 1 / 16;
/**
 * ...and its chance in a room that holds a star ball or the radar (2.6d
 * C5).
 */
export const CAPSULE_BESIDE = 1 / 3;

/**
 * The curio that hovers (2.6d C4): its underside's height over the surface
 * it hovers over, in metres. Its mesh is built at that height, the model
 * test's float check starts from it, and its lowest vertex is exactly this
 * height. The one exception to the float check among the curios; a test
 * names it, so a second one is a conscious change.
 */
export const CURIO_LIFT = { "hover-drone": 0.35 } as const satisfies Partial<
  Record<CurioKind, number>
>;

/**
 * A curio kind's lift (`CURIO_LIFT`), 0 for every kind that stands on its
 * surface.
 */
export function curioLift(kind: CurioKind): number {
  return (CURIO_LIFT as Partial<Record<CurioKind, number>>)[kind] ?? 0;
}
/** How far a curio's box keeps inside its surface's edges, in metres. */
export const CURIO_MARGIN = 0.02;
/** The least gap between two curios at the same height, in metres. */
export const CURIO_GAP = 0.03;
/** How far under the ceiling a curio's top stays, in metres (C12). */
export const CURIO_CEILING_GAP = 0.3;

/**
 * The tops and under spots of the fixtures that have any (C3), local to
 * their wall slot (`d` from the wall): a terminal's two desk ends beside
 * its screen and key deck, a workbench's top and lower shelf, a lab
 * bench's two clear ends, and the floor under the hydroponics trough. The
 * terminal's ends are 0.22 m wide, too narrow for the laptop (C8).
 *
 * A terminal carries no under spot: the model draws its swivel chair in
 * that knee space (`render/models/terminal.ts`'s `TERMINAL_OCCLUDERS`),
 * between the wall and every spot a player can stand on, so no curio
 * placed there could ever be seen. The hydroponics trough stands on four
 * corner legs (`render/models/machines.ts`, legs inside `a` of +-0.77,
 * the trough's underside at 0.55 m), open at the front, so its floor spot
 * runs between the legs and under the whole trough. The lab bench is a
 * closed cabinet down to the floor and the nav table a solid pedestal, so
 * neither has one.
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
    hydroponics: [
      {
        a0: -0.75,
        a1: 0.75,
        d0: 0.12,
        d1: 0.68,
        h: 0,
        clear: 0.54,
        cls: "under",
      },
    ],
  },
} as const satisfies {
  terminal: readonly SurfaceSpec[];
  machine: Partial<Record<MachineKind, readonly SurfaceSpec[]>>;
};

/**
 * The tops and under spots of the decor that has any (C3), local to the
 * piece's centre: the lab island's clear stretch between its hood and its
 * sink, the round table's four places around its glowing disc, and then
 * the floor under the table's rim at the same four places.
 *
 * The round table (`render/models/decor.ts`'s `ROUND_TABLE`) is lathe
 * turned: a foot disc 0.5 m in radius, a column, and an underside that
 * flares from 0.64 m at the column to 0.72 m at 1.1 m out, so every spot
 * keeps outside the foot (0.55 m from the centre at its nearest) and
 * inside the flare (1.08 m at its farthest corner), and its `clear` of
 * 0.63 m stays under the flare's lowest point. It is open all round. The places along `d` take
 * the trap or the case; the places along `a` are 0.45 m wide there and
 * take only the case, since a `fixed` under curio keeps its width along
 * the host's `a`.
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
    {
      a0: -0.4,
      a1: 0.4,
      d0: -1.0,
      d1: -0.55,
      h: 0,
      clear: 0.63,
      cls: "under",
    },
    {
      a0: 0.55,
      a1: 1.0,
      d0: -0.4,
      d1: 0.4,
      h: 0,
      clear: 0.63,
      cls: "under",
    },
    {
      a0: -0.4,
      a1: 0.4,
      d0: 0.55,
      d1: 1.0,
      h: 0,
      clear: 0.63,
      cls: "under",
    },
    {
      a0: -1.0,
      a1: -0.55,
      d0: -0.4,
      d1: 0.4,
      h: 0,
      clear: 0.63,
      cls: "under",
    },
  ],
} as const satisfies Partial<Record<DecorKind, readonly SurfaceSpec[]>>;

/**
 * The shelf levels, tops and under spots of the floor props that have any
 * (C3), per variant, local to the prop's centre: the storage shelf's open
 * top (v0) or its three levels, each `clear` up to the next level's
 * underside, and its top (v1), each filing cabinet's top, the floor
 * under each bench's seat, and the service trolley's own deck top.
 *
 * The bench (`render/models/props/floor.ts`'s `bench`) is a slab seat from
 * 0.42 m on two slab legs whose inner faces stand at `a` of +-0.59, open
 * along `d` on both sides (v0) or at the front, under a back panel that
 * starts over the seat (v1). The spot keeps between the legs and under
 * the seat, which runs +-0.23 along `d` on v0 and -0.105 to 0.28 on v1.
 *
 * The trolley (`render/models/props/floor.ts`'s `trolley`) stands its
 * (single, in both variants) lower deck on four corner casters whose
 * posts rise to `TROLLEY.caster + 0.16` = 0.23 m: the floor beneath it
 * clears only 0.23 m, too low for the fuel case (0.24 m tall) and too far
 * under the deck, past the casters, for a standing player to ever see the
 * trap either. The ruling puts both curios on the deck instead, not
 * under it. v0 has one deck, its top at 0.27 m (0.23 m plus the deck's
 * own 0.04 m), open above with nothing of the trolley taller than its own
 * handle, which rises only from the back edge (`-d`). v1 adds a second
 * deck on four corner posts 0.3 m up: its lower deck top, at 0.27 m too,
 * sits under the upper one with a real 0.26 m gap, clearing the case by
 * only 0.02 m, and a sight line to a curio there must duck under the
 * upper deck's own footprint from outside it, needing about 2.4 m of
 * clear floor around the trolley before an eye-height line gets that low
 * soon enough - the same reach that starved the under-the-deck spot this
 * replaces. So v1 hosts on its upper deck instead, top at 0.57 m, open
 * above like v0's. Both spots stop short of the back edge rather than
 * dodge the handle's rails and its top bar there, which cross nearly the
 * whole width at that one edge.
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
  bench: [
    [
      {
        a0: -0.57,
        a1: 0.57,
        d0: -0.22,
        d1: 0.22,
        h: 0,
        clear: 0.41,
        cls: "under",
      },
    ],
    [
      {
        a0: -0.57,
        a1: 0.57,
        d0: -0.105,
        d1: 0.28,
        h: 0,
        clear: 0.41,
        cls: "under",
      },
    ],
  ],
  trolley: [
    [
      {
        a0: -0.4,
        a1: 0.4,
        d0: -0.15,
        d1: 0.24,
        h: 0.27,
        clear: OPEN_CLEAR,
        cls: "under",
      },
    ],
    [
      {
        a0: -0.46,
        a1: 0.46,
        d0: -0.15,
        d1: 0.27,
        h: 0.57,
        clear: OPEN_CLEAR,
        cls: "under",
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
 * The height of a curio's middle, in metres: halfway from its lift to its
 * top over its surface. What a frame aims at and a sight line ends on, so a
 * hovering curio is looked at, not the gap under it.
 */
export function curioMid(c: Curio): number {
  return c.h + (curioLift(c.kind) + curioSize(c).top) / 2;
}

/**
 * The surface classes curio `kind` of `variant` stands on: its
 * `variantClasses` entry when it has one, else its kind's `classes`.
 */
export function curioClasses(
  kind: CurioKind,
  variant: number,
): readonly SurfaceClass[] {
  const e = CURIO_CATALOGUE[kind];
  return e.variantClasses?.[variant] ?? e.classes;
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

/**
 * One slot's draw (C6): whether the room takes the slot, the roll that
 * picks its kind from the kinds that fit, and, on the dev seam's forced
 * path only, the one `kind` the slot then tries instead of its pool.
 */
export interface SlotDraw {
  take: boolean;
  roll: number;
  kind?: CurioKind;
}

/**
 * A paired slot's draw (2.6d C5): a `SlotDraw` whose `take` is its own
 * share, plus `paired`, whether the same uniform fell under the slot's
 * chance beside a partner. The slot then takes when `take`, or when
 * `paired` and the room already holds its partner (`slotTakes`).
 */
export interface PairedDraw extends SlotDraw {
  paired: boolean;
}

/**
 * A room's slot draws (C6, 2.6d C5); the ball slot also carries its floor
 * chance (`BALL_FLOOR`).
 */
export interface CurioDraws {
  retro: SlotDraw;
  gear: SlotDraw;
  ball: SlotDraw & { floor: boolean };
  under: SlotDraw;
  /**
   * The tech slot (the reactor case, the drone). Absent means not drawn,
   * so a forced-draws literal without it takes no tech curio;
   * `curioDraws` always sets it.
   */
  tech?: SlotDraw;
  /**
   * The radar slot, paired with a star ball. Absent means not drawn;
   * `curioDraws` always sets it.
   */
  radar?: PairedDraw;
  /**
   * The capsule slot, paired with a star ball or the radar. Absent means
   * not drawn; `curioDraws` always sets it.
   */
  capsule?: PairedDraw;
}

/** The slots in the order the pass fills them (C6, 2.6d C5). */
const SLOTS: readonly CurioSlot[] = [
  "retro",
  "gear",
  "ball",
  "under",
  "tech",
  "radar",
  "capsule",
];

/**
 * Whether a slot takes (2.6d C5): its draw's own chance, or, for the radar
 * and the capsule case, the paired chance when the room already holds a
 * partner among the curios placed before it in this pass: a star ball for
 * the radar, a star ball or the radar for the capsule case.
 */
function slotTakes(
  slot: CurioSlot,
  draw: SlotDraw | PairedDraw,
  held: readonly CurioKind[],
): boolean {
  if (draw.take) return true;
  if (!("paired" in draw) || !draw.paired) return false;
  const holds = (k: CurioKind) => held.includes(k);
  if (slot === "radar") return holds("star-ball");
  if (slot === "capsule") return holds("star-ball") || holds("treasure-radar");
  return false;
}

/**
 * A room's curio draws: `curioDrawsOf(room.seed)`.
 */
export function curioDraws(room: CurioBase): CurioDraws {
  return curioDrawsOf(room.seed);
}

/**
 * The draws of the room with seed `seed` (the module doc's step 1),
 * exactly as `curioDraws` makes them (2.6f C6): one stream of
 * `seedFor(seed, "curio", "draw")` draws, in this order, the retro slot's
 * chance of `RETRO_SHARE` and its roll, the gear slot's chance of
 * `GEAR_SHARE` and its roll, the ball slot's chance of `BALL_SHARE`, its
 * roll and its floor chance of `BALL_FLOOR`, the under slot's chance of
 * `UNDER_SHARE` and its roll, the tech slot's chance of `TECH_SHARE` and
 * its roll, then the radar's uniform and roll and the capsule case's
 * uniform and roll (2.6d C5): each uniform gives its `take` below its
 * share (`RADAR_SHARE`, `CAPSULE_SHARE`) and its `paired` below its chance
 * beside a partner (`RADAR_BESIDE_BALL`, `CAPSULE_BESIDE`). Everything is
 * drawn whether it is used or not, so no draw ever moves another. No draw
 * forces a kind. They read nothing of a room but its seed, so a
 * neighbour's draws (`rawCurios`) are made from its seed alone.
 */
export function curioDrawsOf(seed: number): CurioDraws {
  const rng = createRng(seedFor(seed, "curio", "draw"));
  const slot = (share: number): SlotDraw => {
    const take = rng.chance(share);
    return { take, roll: rng.next() };
  };
  const retro = slot(RETRO_SHARE);
  const gear = slot(GEAR_SHARE);
  const ballTake = rng.chance(BALL_SHARE);
  const ballRoll = rng.next();
  const floor = rng.chance(BALL_FLOOR);
  const under = slot(UNDER_SHARE);
  const tech = slot(TECH_SHARE);
  // One uniform per paired slot: `take` below its share, `paired` below
  // its chance beside a partner (2.6d C5). The same comparison as
  // `rng.chance`, so the share means what it says.
  const paired = (share: number, beside: number): PairedDraw => {
    const u = rng.next();
    return { take: u < share, paired: u < beside, roll: rng.next() };
  };
  const radar = paired(RADAR_SHARE, RADAR_BESIDE_BALL);
  const capsule = paired(CAPSULE_SHARE, CAPSULE_BESIDE);
  return {
    retro,
    gear,
    ball: { take: ballTake, roll: ballRoll, floor },
    under,
    tech,
    radar,
    capsule,
  };
}

/** The slots whose pool is the archetype's own (C7): left out for a neighbour of unknown type. */
const ARCHETYPE_SLOTS: ReadonlySet<CurioSlot> = new Set([
  "retro",
  "gear",
  "tech",
]);

/**
 * A room's raw curios (2.6f C6): the kinds its draws would place where
 * every kind fits and no neighbour is skipped, slot by slot in the pass's
 * order (`SLOTS`), each picked by its slot's roll from the slot's whole
 * pool (`poolOf`); a paired slot takes beside a partner listed before it
 * (`slotTakes`). A slot whose pool is the archetype's own is left out when
 * the archetype is unknown (null); the other slots read no archetype, so
 * the stand-in handed to `poolOf` for them never matters.
 */
export function rawCurios(
  draws: CurioDraws,
  archetype: Archetype | null,
): CurioKind[] {
  const out: CurioKind[] = [];
  for (const slot of SLOTS) {
    const draw = draws[slot];
    if (draw === undefined || !slotTakes(slot, draw, out)) continue;
    if (archetype === null && ARCHETYPE_SLOTS.has(slot)) continue;
    const kind = pickByRoll(
      draw.roll,
      poolOf(slot, draw, archetype ?? "bridge"),
    );
    if (kind !== null) out.push(kind);
  }
  return out;
}

/** How far in from both hall walls a floor ball's centre stands, in metres. */
const CORNER_IN = 0.12;
/**
 * How far a floor ball's box keeps from lanes, footprints, props and
 * heroes, in metres.
 */
const CORNER_CLEAR = 0.05;

/**
 * The hall's usable corner spots for a floor ball (the module doc's step
 * 4): of the four corner cells `(hall.x0, hall.y0)`, `(hall.x1 - 1,
 * hall.y0)`, `(hall.x0, hall.y1 - 1)` and `(hall.x1 - 1, hall.y1 - 1)`, in
 * that order, each with the point `CORNER_IN` in from both hall walls
 * (`x` and `y` in cell units), only the ones where
 *
 * - the cell is floor;
 * - both of its hall wall edges are free (`dressingSites(room).free`),
 *   carry no wall prop and are no hero's edge (`heroEdges`);
 * - the box of the biggest ball of `BALL_POOL` at that point, grown by
 *   `CORNER_CLEAR`, overlaps no lane, no taken box (fixtures, decor,
 *   scaffolding), no floor prop and no hero.
 *
 * Only the spots are returned, never the unusable corners. A hall one cell
 * wide gives the same cell twice with two different points, which is
 * harmless. This is the one path that calls `dressingSites`, so the pass
 * calls it only for a ball that drew its floor chance.
 */
export function cornerSpots(
  room: CurioBase,
): { cx: number; cy: number; x: number; y: number }[] {
  const hall = room.hall;
  const sites = dressingSites(room);
  const walled = new Set<string>();
  for (const p of room.props)
    if (p.anchor === "wall") walled.add(edgeKey(edgeOf(p)));
  for (const h of room.heroes)
    for (const e of heroEdges(h)) walled.add(edgeKey(e));
  const floorBoxes: Box[] = [
    ...sites.lanes,
    ...sites.taken,
    ...room.heroes.map((h) => heroFootprint(h)),
  ];
  for (const p of room.props) {
    const box = propFootprint(p);
    if (box !== null) floorBoxes.push(box);
  }
  let half = 0;
  for (const [kind] of BALL_POOL)
    for (const s of CURIO_CATALOGUE[kind].sizes)
      half = Math.max(half, s.width / 2, s.depth / 2);
  const inCells = CORNER_IN / CELL;
  const corners = [
    [hall.x0, hall.y0, "w", "n"],
    [hall.x1 - 1, hall.y0, "e", "n"],
    [hall.x0, hall.y1 - 1, "w", "s"],
    [hall.x1 - 1, hall.y1 - 1, "e", "s"],
  ] as const;
  const out: { cx: number; cy: number; x: number; y: number }[] = [];
  for (const [cx, cy, sx, sy] of corners) {
    if (!isFloor(room.grid, cx, cy)) continue;
    const edges = [sx, sy].map((side) => edgeKey({ x: cx, y: cy, side }));
    if (edges.some((k) => !sites.free.has(k) || walled.has(k))) continue;
    const x = round3(sx === "w" ? cx + inCells : cx + 1 - inCells);
    const y = round3(sy === "n" ? cy + inCells : cy + 1 - inCells);
    const box = grow(
      {
        x0: x * CELL - half,
        x1: x * CELL + half,
        z0: y * CELL - half,
        z1: y * CELL + half,
      },
      CORNER_CLEAR,
    );
    if (floorBoxes.some((b) => overlaps(box, b))) continue;
    out.push({ cx, cy, x, y });
  }
  return out;
}

/**
 * True when curio `c` stands on surface `s` of `room` (the module doc's
 * step 6, C12): its variant stands on the surface's class
 * (`curioClasses`), a `floorOnly` kind only on a surface of `h` 0 (2.6d
 * C7), its box lies inside the surface's box shrunk by
 * `CURIO_MARGIN` (less 2 mm, for the rounding of its centre to three
 * decimals of a cell), its top is at most the surface's `clear`, and the
 * surface's height plus its top stays `CURIO_CEILING_GAP` under the
 * ceiling.
 */
export function curioFits(room: CurioBase, c: Curio, s: HostSurface): boolean {
  const box = curioBox(c);
  const inner = grow(s.box, -(CURIO_MARGIN - 0.002));
  const top = curioSize(c).top;
  return (
    curioClasses(c.kind, c.variant).includes(s.cls) &&
    (CURIO_CATALOGUE[c.kind].floorOnly !== true || s.h === 0) &&
    box.x0 >= inner.x0 &&
    box.x1 <= inner.x1 &&
    box.z0 >= inner.z0 &&
    box.z1 <= inner.z1 &&
    top <= s.clear &&
    s.h + top <= room.ceiling - CURIO_CEILING_GAP
  );
}

/**
 * True when two curios are too close: their plan boxes, one grown by
 * `CURIO_GAP`, overlap, and their height ranges `[h, h + top]` overlap. So
 * a curio on a shelf level never clashes with one on the level below.
 */
export function curiosClash(a: Curio, b: Curio): boolean {
  if (!overlaps(grow(curioBox(a), CURIO_GAP), curioBox(b))) return false;
  const ta = a.h + curioSize(a).top;
  const tb = b.h + curioSize(b).top;
  return a.h < tb && b.h < ta;
}

/**
 * The quarter turn a curio of `facing` takes on surface `s` for a turn roll
 * of 0 to 3 (C10): an `any` curio adds the whole roll to the host's turn; a
 * `front` curio does too on a free host, and on a wall host reads a roll
 * of 2 as 0, so it never faces the wall; a `fixed` curio faces its wall
 * host's front, and on a free host its front or its back by the roll's
 * lowest bit.
 */
function curioTurn(
  facing: CurioEntry["facing"],
  s: HostSurface,
  roll: number,
): number {
  if (facing === "any" || (facing === "front" && s.free))
    return (s.turn + roll) % 4;
  if (facing === "front") return (s.turn + (roll === 2 ? 0 : roll)) % 4;
  return s.free ? (s.turn + 2 * (roll % 2)) % 4 : s.turn;
}

/**
 * Curio `kind` of `variant` at `turn` on `s`'s box, its box at fractions
 * `u` and `v` of the slack the box leaves inside `CURIO_MARGIN`, or null
 * when it leaves none (a slack within `EPS` of zero counts as zero). Its
 * centre is rounded to three decimals of a cell and its height is the
 * surface's. What a trial and `curioOn` share.
 */
function placeOn(
  s: HostSurface,
  kind: CurioKind,
  variant: number,
  turn: number,
  u: number,
  v: number,
  seed: number,
): Curio | null {
  const probe: Curio = { kind, variant, x: 0, y: 0, h: s.h, turn, seed };
  const b = curioBox(probe);
  const w = b.x1 - b.x0;
  const d = b.z1 - b.z0;
  // A slack of a hair under zero is float noise on an exact fit (the
  // laptop on a round table's place), not a curio too big.
  const rawX = s.box.x1 - s.box.x0 - w - 2 * CURIO_MARGIN;
  const rawZ = s.box.z1 - s.box.z0 - d - 2 * CURIO_MARGIN;
  if (rawX < -EPS || rawZ < -EPS) return null;
  const slackX = Math.max(0, rawX);
  const slackZ = Math.max(0, rawZ);
  const cx = s.box.x0 + CURIO_MARGIN + w / 2 + u * slackX;
  const cz = s.box.z0 + CURIO_MARGIN + d / 2 + v * slackZ;
  return { ...probe, x: round3(cx / CELL), y: round3(cz / CELL) };
}

/**
 * A curio of `kind` and `variant` on surface `s`, at fractions `u` and `v`
 * of the slack its box leaves inside `CURIO_MARGIN`, turned `turn` (the
 * surface's own by default) and carrying `seed`: how the hand-built rooms
 * put a curio exactly where they want it. It throws when the kind does not
 * stand on the surface's class, when its box does not fit the surface at
 * that turn, or when its top passes the surface's `clear`. It knows no
 * room, so the caller keeps it under the ceiling.
 */
export function curioOn(
  s: HostSurface,
  kind: CurioKind,
  variant: number,
  u: number,
  v: number,
  seed: number,
  turn: number = s.turn,
): Curio {
  const where = `curioOn: ${kind} ${String(variant)} on ${s.host}`;
  if (!curioClasses(kind, variant).includes(s.cls))
    throw new Error(`${where}: no ${s.cls} curio`);
  const c = placeOn(s, kind, variant, turn, u, v, seed);
  if (c === null) throw new Error(`${where}: too big for the surface`);
  if (curioSize(c).top > s.clear) throw new Error(`${where}: too tall`);
  return c;
}

/**
 * One place a curio may be tried at: its seed, its order among its
 * fellows, and either the surface it stands on or the corner point it
 * stands at.
 */
type CurioCandidate =
  | { seed: number; order: number; on: "surface"; surface: HostSurface }
  | { seed: number; order: number; on: "floor"; x: number; y: number };

/**
 * The candidates of `kind` in `room` (the module doc's step 4), in the
 * order they are tried: a kind's `prefers` host's surfaces first (2.6d
 * C8), then by seed, ties by their order in `surfaces` or among the
 * corners. A floor ball takes the corners (`corners`, worked out once and
 * only when asked), any other kind the surfaces of its classes, a
 * `floorOnly` kind only those of `h` 0 (2.6d C7).
 */
function candidatesOf(
  room: CurioBase,
  kind: CurioKind,
  floor: boolean,
  surfaces: readonly HostSurface[],
  corners: () => readonly { cx: number; cy: number; x: number; y: number }[],
): CurioCandidate[] {
  const e = CURIO_CATALOGUE[kind];
  const out: CurioCandidate[] = floor
    ? corners().map((c, order) => ({
        seed: seedFor(room.seed, "curio", c.cx, c.cy, "floor"),
        order,
        on: "floor" as const,
        x: c.x,
        y: c.y,
      }))
    : surfaces.flatMap((s, order) =>
        e.classes.includes(s.cls) && (e.floorOnly !== true || s.h === 0)
          ? [
              {
                seed: seedFor(room.seed, "curio", s.key[0], s.key[1], s.key[2]),
                order,
                on: "surface" as const,
                surface: s,
              },
            ]
          : [],
      );
  const first = (c: CurioCandidate) =>
    e.prefers !== undefined &&
    c.on === "surface" &&
    c.surface.host === e.prefers
      ? 0
      : 1;
  return out.sort(
    (a, b) => first(a) - first(b) || a.seed - b.seed || a.order - b.order,
  );
}

/**
 * One trial (the module doc's step 5): the candidate's stream draws the
 * variant (the pink gadget's cluster only in a lab or an engineering
 * room), a turn roll and `u` and `v`, all of them always. A floor ball
 * stands at its corner point at the turn roll; anything else is turned by
 * its facing and placed in its surface's slack. The curio, or null when it
 * does not fit (`curioFits`, or the ceiling for a floor ball) or clashes
 * with one of `placed`.
 */
function trial(
  room: CurioBase,
  kind: CurioKind,
  cand: CurioCandidate,
  placed: readonly Curio[],
): Curio | null {
  const e = CURIO_CATALOGUE[kind];
  const rng = createRng(cand.seed);
  let variant = rng.int(0, e.variants - 1);
  if (
    kind === "pink-gadget" &&
    room.archetype !== "lab" &&
    room.archetype !== "engineering"
  )
    variant = 0;
  const turnRoll = rng.int(0, 3);
  const u = rng.next();
  const v = rng.next();
  let c: Curio | null;
  if (cand.on === "floor") {
    c = {
      kind,
      variant,
      x: cand.x,
      y: cand.y,
      h: 0,
      turn: turnRoll,
      seed: cand.seed,
    };
    if (curioSize(c).top > room.ceiling - CURIO_CEILING_GAP) return null;
  } else {
    const s = cand.surface;
    const turn = curioTurn(e.facing, s, turnRoll);
    c = placeOn(s, kind, variant, turn, u, v, cand.seed);
    if (c === null || !curioFits(room, c, s)) return null;
  }
  const mine = c;
  return placed.some((p) => curiosClash(p, mine)) ? null : c;
}

/**
 * A slot's pool: the kind its draw forces, or its own weighted pool (C7,
 * 2.6d C6, C9).
 */
function poolOf(
  slot: CurioSlot,
  draw: SlotDraw,
  archetype: Archetype,
): readonly (readonly [CurioKind, number])[] {
  if (draw.kind !== undefined) return [[draw.kind, 1]];
  switch (slot) {
    case "retro":
      return RETRO_POOLS[archetype];
    case "gear":
      return GEAR_POOLS[archetype];
    case "ball":
      return BALL_POOL;
    case "under":
      return UNDER_POOL;
    case "tech":
      return TECH_POOLS[archetype];
    case "radar":
      return RADAR_POOL;
    case "capsule":
      return CAPSULE_POOL;
  }
}

/**
 * The curio pass (see the module doc's numbered list): the curios of
 * `room`, at most one per slot, sorted by `CURIO_ORDER`. `draws` default
 * to the room's own (`curioDraws`); the tests and the dev seam pass forced
 * ones. It reads the fixtures, decor, heroes and props and changes none
 * of them, and it never throws on a room with no host for a drawn kind:
 * that slot just stays empty. The fit filter and the placement share one
 * loop (`tryPlace`), so a kind that counted as fitting always lands.
 * `near` is what the room's neighbours draw raw (2.6f C7, C8), which the
 * pass skips outside a forced draw; it defaults to `NO_NEAR`, which skips
 * nothing, and the generator and the forced-hero and forced-prop seams
 * pass the room's own (`nearFor` in `generate.ts`).
 */
export function placeCurios(
  room: CurioBase,
  draws: CurioDraws = curioDraws(room),
  near: Near = NO_NEAR,
): Curio[] {
  const surfaces = hostSurfaces(room);
  let corners: ReturnType<typeof cornerSpots> | null = null;
  const cornersOnce = () => (corners ??= cornerSpots(room));
  const placed: Curio[] = [];
  for (const slot of SLOTS) {
    const draw = draws[slot];
    if (
      draw === undefined ||
      !slotTakes(
        slot,
        draw,
        placed.map((c) => c.kind),
      )
    )
      continue;
    const pool = poolOf(slot, draw, room.archetype);
    // A forced draw (the dev seam) never reads its neighbours (2.6f C9).
    const forced = draw.kind !== undefined;
    // A one-kind pool is a solo draw (2.6f C8): not tried when a neighbour
    // with a lower seed draws that kind raw.
    const only = pool.length === 1 ? pool[0]?.[0] : undefined;
    if (!forced && only !== undefined && near.curiosBelow.has(only)) continue;
    let floor = slot === "ball" && draws.ball.floor;
    const tryPlace = (kind: CurioKind): Curio | null => {
      for (const cand of candidatesOf(
        room,
        kind,
        floor,
        surfaces,
        cornersOnce,
      )) {
        const c = trial(room, kind, cand, placed);
        if (c !== null) return c;
      }
      return null;
    };
    let fitting = pool.filter(([kind]) => tryPlace(kind) !== null);
    // A floor ball with no corner it fits falls back to the surfaces, so
    // the ball rate stays at `BALL_SHARE`; nothing is drawn again.
    if (floor && fitting.length === 0) {
      floor = false;
      fitting = pool.filter(([kind]) => tryPlace(kind) !== null);
    }
    // The kind the roll gives from the fitting kinds; only when a
    // neighbour draws it raw, the same roll picks again from the fitting
    // kinds less every neighbour's, as far as they allow (2.6f C7). Every
    // fitting kind lands, so no fallback is needed.
    const raw = pickByRoll(draw.roll, fitting);
    const kind =
      raw !== null && !forced && near.curios.has(raw)
        ? pickByRoll(draw.roll, skipNear(fitting, near.curios))
        : raw;
    if (kind === null) continue;
    const c = tryPlace(kind);
    if (c !== null) placed.push(c);
  }
  return placed.sort(CURIO_ORDER);
}
