/**
 * The hangar deck (M3 C13 to C18): one folder in five is a hangar instead
 * of a corridor hub, a hall 20 by 16 cells (40 by 32 m) under a 9 m
 * ceiling, with a bay door across its north wall, two landing pads on its
 * floor, two gantries overhead and a door to each engram on the other
 * walls.
 *
 * - **Which folder** (C13): `isHangar` - a non-root folder whose slug
 *   hashes (`seedFor("hangar", domain, folderSlug(folder))`) to 0 modulo
 *   `HANGAR_EVERY`, every section of it. The root is never a hangar, and
 *   nor is a folder whose slug is empty, which is numbered as the root.
 * - **The layout** (C14, C15): `hangarLayout` lays the hall out from plain
 *   inputs (the section's row count and the deck's seed), so `deck.ts`
 *   calls it from `generateDeck` and this module needs nothing of `deck.ts`
 *   at run time. The entrance is cell (10, 15), carrying the deck's lift;
 *   the deck's screen stands on edge (4, 0, n), beside the bay door. The
 *   bay door spans the north wall's cells 5 to 15 (exclusive), 7 m tall;
 *   the pads are cells x 2..9 and 11..18, y 4..11 (exclusive ends), each
 *   keeping a 13 by 13 by 7.5 m hero envelope; the gantries run over rows
 *   3 and 12 at 7.5 m, on a leg at each wall (`gantryLegEdges`). The door
 *   slots are `wallSlots` of the grid, west wall first, then east, south
 *   and north (each wall in `wallSlots`' own order), less the leg edges,
 *   the entrance edge and the bay door's span: 28 slots for at most 24
 *   doors. The zones are lit at `HANGAR_LIGHT`, steady (C19).
 * - **The dressing** (C16) is the ordinary pass: `dressingSites` takes the
 *   pads and the gantry legs as taken boxes and keeps the bay door's span
 *   and the leg edges free of anything hung on a wall, so the cargo
 *   gathers along the walls and between the pads.
 * - **The pad stencils** (C17) are laid by `placeDecals` itself, so every
 *   pass that re-runs the decals keeps them.
 * - **The pad hero seam** (C18): `withPadHeroes` stands a given list of
 *   heroes in the hangar and re-runs the dressing, the curios and the
 *   decals round them, as `withHeroes` does for an engram room.
 *   `hangarHeroes` is what the hangar heroes will be; it is empty until the
 *   prop round builds them. `generateDeck` ends a hangar with
 *   `withPadHeroes(room, [...room.heroes, ...hangarHeroes(room)])`, which
 *   changes nothing while the list is empty.
 *
 * The generator side: it imports the seed, the folder helpers, the layout,
 * the passes it re-runs and the leaf `hangarShape.ts`; never `deck.ts` at
 * run time (which imports this module) and never the session's modules.
 */

import { seedFor } from "../core/seed";
import { curioDraws, placeCurios } from "./curios";
import { placeDecals } from "./decals";
import { dressRoom } from "./dress";
import { folderSlug } from "./folders";
import { gantryLegEdges } from "./hangarShape";
import { HERO_ORDER } from "./heroes";
import { wallSlots } from "./layout";
import { NO_NEAR, edgeKey } from "./sites";
import type { HangarSpec, Hero, RoomSpec, Side, WallSlot } from "./types";

/** One folder in this many is a hangar (M3 C13). */
export const HANGAR_EVERY = 5;

/** A hangar's width in cells (M3 C14): 40 m. */
export const HANGAR_WIDTH = 20;

/** A hangar's depth in cells (M3 C14): 32 m. */
export const HANGAR_DEPTH = 16;

/** A hangar's ceiling in metres (M3 C14). */
export const HANGAR_CEILING = 9;

/** The light level of every hangar zone, steady (M3 C19). */
export const HANGAR_LIGHT = 224;

/** The hangar's entrance cell (M3 C14): the south wall's middle. */
const ENTRANCE = { x: 10, y: 15 } as const;

/** The deck's screen edge in a hangar (M3 C15): beside the bay door. */
const SCREEN: WallSlot = { x: 4, y: 0, side: "n" };

/** The bay door (M3 C15): north wall cells 5 to 15, exclusive, 7 m tall. */
const BAY_DOOR = { x0: 5, x1: 15, h: 7 } as const;

/** The two pads' cells (M3 C14), `x1` and `y1` exclusive. */
const PADS = [
  { x0: 2, y0: 4, x1: 9, y1: 11 },
  { x0: 11, y0: 4, x1: 18, y1: 11 },
] as const;

/**
 * A pad's hero envelope in metres (M3 C14): the largest deferred hangar
 * hero, about 12.5 m long, and the 6 m sphere on legs both fit.
 */
const ENVELOPE = { w: 13, l: 13, h: 7.5 } as const;

/** The gantries (M3 C15): over rows 3 and 12, at 7.5 m. */
const GANTRIES = [
  { y: 3, h: 7.5 },
  { y: 12, h: 7.5 },
] as const;

/** The walls in the order the door slots are handed out (M3 C15). */
const WALL_ORDER: readonly Side[] = ["w", "e", "s", "n"];

/**
 * Whether a folder's deck is a hangar (M3 C13): a non-root folder whose
 * slug hashes to 0 modulo `HANGAR_EVERY`, the same answer for every
 * section of it. The root (`""`) never is, nor a folder whose slug is
 * empty (it is numbered as the root, `folderDeck`).
 */
export function isHangar(domain: string, folder: string): boolean {
  // The root's slug is empty too, so this one test keeps both out.
  const slug = folderSlug(folder);
  if (slug === "") return false;
  return seedFor("hangar", domain, slug) % HANGAR_EVERY === 0;
}

/**
 * A hangar's floor plan (M3 C14, C15): the grid, the entrance, the first
 * `rows` door slots in the order they are handed out, the screen's edge
 * and the structure. Plain inputs: `rows` is the section's engram count
 * and `seed` the deck's seed (each pad's seed is `seedFor(seed, "pad",
 * i)`). See the module doc for the numbers.
 */
export function hangarLayout(input: { rows: number; seed: number }): {
  grid: string[];
  entrance: { x: number; y: number };
  doorSlots: WallSlot[];
  screen: WallSlot;
  hangar: HangarSpec;
} {
  const grid = Array.from({ length: HANGAR_DEPTH }, () =>
    ".".repeat(HANGAR_WIDTH),
  );
  const hangar: HangarSpec = {
    bayDoor: { ...BAY_DOOR },
    pads: PADS.map((p, i) => ({
      ...p,
      seed: seedFor(input.seed, "pad", i),
      envelope: { ...ENVELOPE },
    })),
    gantries: GANTRIES.map((g) => ({ ...g })),
  };
  const taken = new Set(
    gantryLegEdges({ width: HANGAR_WIDTH, hangar }).map(edgeKey),
  );
  taken.add(edgeKey({ ...ENTRANCE, side: "s" }));
  const underBayDoor = (e: WallSlot) =>
    e.side === "n" && e.x >= BAY_DOOR.x0 && e.x < BAY_DOOR.x1;
  const slots = wallSlots(grid).filter(
    (e) => !taken.has(edgeKey(e)) && !underBayDoor(e),
  );
  const doorSlots = WALL_ORDER.flatMap((side) =>
    slots.filter((e) => e.side === side),
  ).slice(0, Math.max(0, input.rows));
  return {
    grid,
    entrance: { ...ENTRANCE },
    doorSlots,
    screen: { ...SCREEN },
    hangar,
  };
}

/**
 * The heroes a hangar's pads carry (M3 C18): none until the prop round
 * builds the hangar heroes, which plug in here and stand through
 * `withPadHeroes`.
 */
export function hangarHeroes(room: RoomSpec): Hero[] {
  void room;
  return [];
}

/**
 * `room` with `heroes` standing in it in place of its own, and the passes
 * that read the heroes run again round them (M3 C18), as `withHeroes`
 * (`generate.ts`) does for an engram room: the heroes sorted by
 * `HERO_ORDER` (a copy; the list handed in is left as it is), the props
 * dressed afresh (`dressRoom`, which keeps off what the heroes reserve and
 * off the pads and the gantry legs), the curios placed afresh with the
 * room's own draws and no neighbours (a deck has none), then the decals
 * laid afresh, the pad stencils among them. Nothing else of the room
 * moves. The heroes are taken as given, so a caller that stands heroes on
 * the pads hands the room's own heroes with them (`[...room.heroes,
 * ...hangarHeroes(room)]`). `withPadHeroes(room, room.heroes)` is `room`
 * again.
 */
export function withPadHeroes(
  room: RoomSpec,
  heroes: readonly Hero[],
): RoomSpec {
  const standing: RoomSpec = { ...room, heroes: [...heroes].sort(HERO_ORDER) };
  const dressed: RoomSpec = { ...standing, props: dressRoom(standing) };
  const placed: RoomSpec = {
    ...dressed,
    curios: placeCurios(dressed, curioDraws(dressed), NO_NEAR),
  };
  return { ...placed, decals: placeDecals(placed) };
}
