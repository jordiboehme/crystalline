/**
 * The airlock (M3 C24, C29): the station's overview, `/π`, built by hand
 * like the console room rather than generated. It is where the player
 * starts and the one place every domain's bridge can be reached from.
 *
 * A round chamber: an 8 by 8 cell grid (16 m across) with its corners cut
 * in steps (`AIRLOCK_GRID`), so the shell draws a wall on every diagonal
 * step as on any floor cell next to void. `AIRLOCK_CEILING` (4.5 m) high,
 * condition `clean`, `space: "airlock"`, a plain finish (`plainFinish`),
 * steady white light at `AIRLOCK_LIGHT` in four zones that cover every
 * floor cell, and nothing a generated room carries besides: no decor,
 * scaffold, heroes, props, curios or decals.
 *
 * It holds two fixtures, facing each other across the chamber:
 *
 * 1. the lift on the entrance edge, cell (3, 7) south, whose stops are
 *    every domain's bridge (`airlockStops`: labelled with the domain's
 *    local name, sorted by `byLabel`, the key on every private one, `here`
 *    on the domain the player came from);
 * 2. the directory, a wall screen on the north edge of cell (3, 0): the
 *    heading `AIRLOCK`, then one line per domain in the lift's order with
 *    the key on every private one, `SCREEN_LINES` lines at most, the last
 *    one `+<n> MORE` when domains are left out. The lift overlay lists
 *    them all.
 *
 * A failed listing (`domains: null`) gives a lift with no stops and the
 * note `?DOMAIN LIST ERROR`, which the lift's retry reads, and the same
 * words as the directory's second line. An empty listing is no failure:
 * a lift with no stops and no note, and the heading alone.
 *
 * A pure function of its input: no clock and no counter, the room's seed
 * `seedFor(GAME_VERSION, "airlock")`, so the same input gives the same room
 * byte for byte (`golden/airlock.json`). The room has no domain or
 * permalink (the session holds the airlock's address itself); its title
 * is `AIRLOCK`, which the status line shows. On
 * the generator side: it imports the seeds, the version, the finish, the
 * lift words and the types, never `ui/`, `render/` or the session's
 * modules.
 */

import { seedFor } from "../core/seed";
import { GAME_VERSION } from "../version";
import { plainFinish } from "./finish";
import { LIFT_WORDS, SCREEN_LINES, airlockStops, moreLine } from "./lifts";
import type { LightZone, RoomSpec } from "./types";

/**
 * The airlock's cells (M3 C24), north row first: an 8 by 8 square with
 * two cells cut off each corner of the first and last rows and one off
 * each corner of the second and seventh, so the chamber reads round.
 */
export const AIRLOCK_GRID: readonly string[] = [
  "  ....  ",
  " ...... ",
  "........",
  "........",
  "........",
  "........",
  " ...... ",
  "  ....  ",
];

/** The airlock's cells along each side. */
const AIRLOCK_CELLS = 8;

/** The airlock's ceiling height, in metres (M3 C24). */
export const AIRLOCK_CEILING = 4.5;

/** The airlock's light level on DOOM's scale, steady everywhere. */
const AIRLOCK_LIGHT = 208;

/** The cells a side of one light zone: four zones, one per quarter. */
const ZONE = 4;

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
  const room = SCREEN_LINES - 1;
  const shown = stops.length <= room ? stops : stops.slice(0, room - 1);
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
 * The airlock's light: four steady zones of 4 by 4 cells at
 * `AIRLOCK_LIGHT`, one per quarter, each seeded
 * `seedFor(roomSeed, "light", zoneIndex)`. Together they cover the grid,
 * so every floor cell is lit; each quarter holds floor, so each hangs a
 * lamp.
 */
function lightsOf(seed: number): LightZone[] {
  const out: LightZone[] = [];
  for (let y0 = 0; y0 < AIRLOCK_CELLS; y0 += ZONE)
    for (let x0 = 0; x0 < AIRLOCK_CELLS; x0 += ZONE)
      out.push({
        x0,
        y0,
        x1: x0 + ZONE,
        y1: y0 + ZONE,
        level: AIRLOCK_LIGHT,
        special: "steady",
        seed: seedFor(seed, "light", out.length),
      });
  return out;
}

/**
 * The airlock (M3 C24, C29): see the module doc. Archetype `bridge` (the
 * console room's; nothing in the room draws by archetype, since it has no
 * decor), the hall the whole grid, the entrance cell (3, 7) and the spawn
 * on it facing north, towards the directory.
 */
export function airlockRoom(input: AirlockInput): RoomSpec {
  const seed = seedFor(GAME_VERSION, "airlock");
  const entrance = { x: 3, y: AIRLOCK_CELLS - 1 };
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
        slot: { x: entrance.x, y: 0, side: "n" },
        lines: screen.lines,
        keys: screen.keys,
        seed: seedFor(seed, "screen"),
      },
    ],
    decor: [],
    scaffold: [],
    heroes: [],
    props: [],
    curios: [],
    finish: plainFinish(0),
    decals: [],
    lights: lightsOf(seed),
    dropped: 0,
    inboundMore: 0,
  };
}
