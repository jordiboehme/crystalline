/**
 * The airlock (M3 C24, C29): the station's overview, `/π`, built by hand
 * like the console room rather than generated. It is where the player
 * starts, and its lift is the one that reaches every domain's bridge.
 *
 * A round chamber: an 11 by 11 cell grid (22 m across) whose rows are the
 * cells with their centre inside the circle round the grid's middle
 * (`AIRLOCK_GRID`), so every diagonal of the round wall is a staircase of
 * one-cell steps and the shell draws a wall on each step, as on any floor
 * cell next to void. `AIRLOCK_CEILING` (8.5 m) high, condition `clean`,
 * `space: "airlock"`, a plain finish (`plainFinish`), steady white light
 * at `AIRLOCK_LIGHT` over the whole grid, and no decor, scaffold, heroes,
 * props or curios. Its shell is drawn in the hand-built rooms' fixed
 * white, with no accent stripe and no lamp panels: the iris light is its
 * lamp.
 *
 * Everything stands on the room's north-south axis, the middle of cell
 * column `AXIS`, or symmetric about it:
 *
 * 1. the lift on the entrance edge, cell (5, 10) south, whose stops are
 *    every domain's bridge (`airlockStops`: labelled with the domain's
 *    local name, sorted by `byLabel`, the key on every private one, `here`
 *    on the domain the player came from);
 * 2. opposite it, on the north edge of cell (5, 0), the outer hatch: a big
 *    round sealed hatch in a hazard-striped collar with the `AIRLOCK`
 *    stencil, which never opens and is no fixture;
 * 3. over the hatch on the same edge, the directory: a screen with
 *    `large`, a board many times the wall screen's size, headed
 *    `AIRLOCK`, then one line per domain in the lift's order with the key
 *    on every private one, `SCREEN_LINES` lines at most, the last one
 *    `+<n> MORE` when domains are left out (the lift overlay lists them
 *    all), so it reads from the lift;
 * 4. two amber beacons either side of the hatch, `BEACON_OFFSET` cells
 *    from the axis, blinking in turn;
 * 5. the iris light in the ceiling over the floor's centre;
 * 6. on the floor round the centre, a hazard ring (`RING`), with a
 *    `CYCLE` stencil inside it on each side of the centre, one reading
 *    from the lift and one from the hatch;
 * 7. suit lockers along the straight runs of the west and east walls,
 *    rows `LOCKER_ROWS`, two to an edge.
 *
 * A failed listing (`domains: null`) gives a lift with no stops and the
 * note `?DOMAIN LIST ERROR` as its status line, and the same words as the
 * directory's second line. An empty listing is no failure: a lift with no
 * stops and no note, and the heading alone.
 *
 * A pure function of its input: no clock and no counter, the room's seed
 * `seedFor(GAME_VERSION, "airlock")` and every other seed drawn from it by
 * name, so the same input gives the same room byte for byte
 * (`golden/airlock.json`). The room has no domain or permalink (the
 * session holds the airlock's address itself); its title is `AIRLOCK`,
 * which the status line shows. On the generator side: it imports the
 * seeds, the version, the finish, the lift words, the wall anchors and the
 * types, never `ui/`, `render/` or the session's modules.
 */

import { seedFor } from "../core/seed";
import { GAME_VERSION } from "../version";
import { plainFinish } from "./finish";
import { LIFT_WORDS, SCREEN_LINES, airlockStops, moreLine } from "./lifts";
import { wallAnchor } from "./sites";
import type {
  Decal,
  InteriorKind,
  InteriorPiece,
  LightZone,
  RoomSpec,
  WallSlot,
} from "./types";

/**
 * The airlock's cells (M3 C24), north row first: the cells of an 11 by 11
 * grid whose centre lies inside the circle of radius 5.5 cells round the
 * grid's middle. Each corner steps in one cell a row over three rows, so
 * the chamber reads round.
 */
export const AIRLOCK_GRID: readonly string[] = [
  "   .....   ",
  "  .......  ",
  " ......... ",
  "...........",
  "...........",
  "...........",
  "...........",
  "...........",
  " ......... ",
  "  .......  ",
  "   .....   ",
];

/** The airlock's cells along each side. */
const AIRLOCK_CELLS = AIRLOCK_GRID.length;

/**
 * The cell column the room's north-south axis runs down the middle of:
 * the lift, the hatch, the directory and the iris light stand on it.
 */
export const AXIS = (AIRLOCK_CELLS - 1) / 2;

/**
 * The airlock's ceiling height, in metres (M3 C24): high enough for the
 * directory board over the outer hatch, and the iris light's top
 * (`INTERIOR_CATALOGUE`).
 */
export const AIRLOCK_CEILING = 8.5;

/** The airlock's light level on DOOM's scale, steady everywhere. */
const AIRLOCK_LIGHT = 208;

/**
 * How far each beacon stands from the axis along the north wall, in
 * cells: 2.7 m, clear of the hatch's collar (1.95 m).
 */
export const BEACON_OFFSET = 1.35;

/**
 * The rows of the west and east walls that carry suit lockers, two to an
 * edge, `LOCKER_ALONG` cells either side of the edge's middle: the middle
 * of the walls' straight runs (rows 3 to 7), a row clear of the steps at
 * either end.
 */
export const LOCKER_ROWS: readonly number[] = [4, 5, 6];

/** How far each of an edge's two lockers stands from its middle, in cells. */
const LOCKER_ALONG = 0.24;

/**
 * The hazard ring on the floor (M3 C24): its outer diameter and its band's
 * width, in metres, round the floor's centre; and how far each `CYCLE`
 * stencil's centre lies from the floor's centre, in cells, with the size
 * of its box.
 */
export const RING = {
  width: 6.4,
  band: 0.5,
  stencilAt: 0.75,
  stencilWidth: 2.4,
  stencilLength: 0.6,
} as const;

/**
 * What the airlock is built from (M3 C24, C29): the domain listing, each
 * domain's local name and whether it is private, or null when the listing
 * failed; and `here`, the domain the player came from (its stop is marked
 * on the lift), or null.
 */
export interface AirlockInput {
  domains: readonly { name: string; private: boolean }[] | null;
  here: string | null;
}

/**
 * The directory's lines and key lines (M3 C24): the heading, then the
 * labels in order while they fit in `SCREEN_LINES`, else as many as leave
 * room for the `+<n> MORE` line after them; `keys` the index of every
 * shown line whose stop is private.
 */
function directory(stops: readonly { label: string; key: boolean }[]): {
  lines: string[];
  keys: number[];
} {
  const fits = SCREEN_LINES - 1;
  const shown = stops.length <= fits ? stops : stops.slice(0, fits - 1);
  const lines: string[] = [LIFT_WORDS.airlock, ...shown.map((s) => s.label)];
  if (shown.length < stops.length)
    lines.push(moreLine(stops.length - shown.length));
  const keys: number[] = [];
  shown.forEach((s, i) => {
    if (s.key) keys.push(i + 1);
  });
  return { lines, keys };
}

/**
 * The airlock's light: one steady zone over the whole grid at
 * `AIRLOCK_LIGHT`, seeded `seedFor(roomSeed, "light", 0)`, so every floor
 * cell is lit alike.
 */
function lightsOf(seed: number): LightZone[] {
  return [
    {
      x0: 0,
      y0: 0,
      x1: AIRLOCK_CELLS,
      y1: AIRLOCK_CELLS,
      level: AIRLOCK_LIGHT,
      special: "steady",
      seed: seedFor(seed, "light", 0),
    },
  ];
}

/**
 * The airlock's fittings (M3 C24), in this order: the outer hatch on the
 * north edge of the axis's cell; the two beacons beside it, west (variant
 * 0) then east (variant 1); the iris light over the floor's centre; then
 * the suit lockers, the west wall's rows north to south and then the east
 * wall's, each edge's two from its north end. Each piece is seeded
 * `seedFor(roomSeed, kind, ...)` of its place.
 */
function interiorOf(seed: number): InteriorPiece[] {
  const piece = (
    kind: InteriorKind,
    variant: number,
    at: { x: number; y: number; turn: number },
    ...where: (string | number)[]
  ): InteriorPiece => ({
    kind,
    variant,
    x: at.x,
    y: at.y,
    turn: at.turn,
    seed: seedFor(seed, kind, ...where),
  });
  const north = wallAnchor({ x: AXIS, y: 0, side: "n" });
  const out: InteriorPiece[] = [piece("outer-hatch", 0, north)];
  out.push(
    piece("beacon", 0, { ...north, x: north.x - BEACON_OFFSET }, "west"),
    piece("beacon", 1, { ...north, x: north.x + BEACON_OFFSET }, "east"),
  );
  const middle = AIRLOCK_CELLS / 2;
  out.push(piece("iris-light", 0, { x: middle, y: middle, turn: 0 }));
  for (const [x, side] of [
    [0, "w"],
    [AIRLOCK_CELLS - 1, "e"],
  ] as const)
    for (const y of LOCKER_ROWS) {
      const edge: WallSlot = { x, y, side };
      const a = wallAnchor(edge);
      for (const shift of [-LOCKER_ALONG, LOCKER_ALONG])
        out.push(
          piece("suit-locker", 0, { ...a, y: a.y + shift }, x, y, side, shift),
        );
    }
  return out;
}

/**
 * The airlock's decals (M3 C24): the hazard ring round the floor's centre,
 * then its two `CYCLE` stencils inside it, the south one reading from the
 * lift (turn 0) and the north one from the hatch (turn 2).
 */
function decalsOf(seed: number): Decal[] {
  const middle = AIRLOCK_CELLS / 2;
  const stencil = (dy: number, turn: number, name: string): Decal => ({
    kind: "stencil",
    on: "floor",
    x: middle,
    y: middle + dy,
    turn,
    along: 0,
    h: 0,
    width: RING.stencilWidth,
    length: RING.stencilLength,
    variant: 0,
    seed: seedFor(seed, "decal", "cycle", name),
    word: "cycle",
  });
  return [
    {
      kind: "ring",
      on: "floor",
      x: middle,
      y: middle,
      turn: 0,
      along: 0,
      h: 0,
      width: RING.width,
      length: RING.band,
      variant: 0,
      seed: seedFor(seed, "decal", "ring"),
    },
    stencil(RING.stencilAt, 0, "south"),
    stencil(-RING.stencilAt, 2, "north"),
  ];
}

/**
 * The airlock (M3 C24, C29): see the module doc. Archetype `bridge` (the
 * console room's; nothing in the room draws by archetype, since it has no
 * decor), the hall the whole grid, the entrance cell on the axis in the
 * south row and the spawn on it facing north, towards the hatch and the
 * directory.
 */
export function airlockRoom(input: AirlockInput): RoomSpec {
  const seed = seedFor(GAME_VERSION, "airlock");
  const entrance = { x: AXIS, y: AIRLOCK_CELLS - 1 };
  const stops =
    input.domains === null ? [] : airlockStops(input.domains, input.here);
  const screen =
    input.domains === null
      ? { lines: [LIFT_WORDS.airlock, LIFT_WORDS.domainError], keys: [] }
      : directory(stops);
  return {
    version: GAME_VERSION,
    seed,
    domain: "",
    permalink: "",
    space: "airlock",
    title: LIFT_WORDS.airlock,
    archetype: "bridge",
    condition: "clean",
    width: AIRLOCK_CELLS,
    depth: AIRLOCK_CELLS,
    grid: [...AIRLOCK_GRID],
    hall: { x0: 0, y0: 0, x1: AIRLOCK_CELLS, y1: AIRLOCK_CELLS },
    bays: [],
    corridor: null,
    entrance,
    ceiling: AIRLOCK_CEILING,
    spawn: { ...entrance, yaw: 0 },
    fixtures: [
      {
        kind: "lift",
        slot: { ...entrance, side: "s" },
        stops,
        note: input.domains === null ? LIFT_WORDS.domainError : null,
        seed: seedFor(seed, "lift"),
      },
      {
        kind: "screen",
        slot: { x: AXIS, y: 0, side: "n" },
        lines: screen.lines,
        keys: screen.keys,
        seed: seedFor(seed, "screen"),
        large: true,
      },
    ],
    decor: [],
    scaffold: [],
    heroes: [],
    props: [],
    curios: [],
    finish: plainFinish(0),
    decals: decalsOf(seed),
    interior: interiorOf(seed),
    lights: lightsOf(seed),
    dropped: 0,
    inboundMore: 0,
  };
}
