/**
 * The console room (2.6e C1, C3): the room inside the police box, built by
 * hand like `galleryRoom` and `heroHallRoom` rather than generated, so the
 * whole pipeline (entering, collision on the grid, lights, the renderer,
 * the looks, the bloom, a lost context) takes it as any other room.
 *
 * It is a 6 by 6 cell hall (12 m across), 4 m high, with nothing a
 * generated room carries: no fixtures, decor, scaffold, heroes, props or
 * curios, no decals, no title and a plain finish (`plainFinish`: accent
 * 0, wall pattern 0; its shell keeps its own tints and draws no accent
 * stripe whatever the finish says). What it holds instead are its fittings,
 * `RoomSpec.interior` (C2): every wall edge carries one flush piece (the
 * inner doors across the south row's cells 2 and 3, the scanner on cell
 * 4's south edge, a wall of roundels on each of the other 21 edges), and
 * the console stands free at the centre. `INTERIOR_CATALOGUE` gives each
 * kind's size, and `interiorFootprint` the one box a fitting makes the
 * player walk round: the console's. The flush pieces collide with nothing;
 * the grid's walls stop the player.
 *
 * The way out is the inner doors: `atConsoleExit` says when the player
 * stands in their doorway (C11). The player arrives 1.6 m in front of
 * them, facing the console, which is outside that zone.
 *
 * A pure constant: no clock and no counter, so the same call gives the same
 * room byte for byte. Like `canned.ts` it may import the seeds, the version,
 * the layout, the sites, the footprints, the finish, the types and the
 * units, never
 * `move.ts`, `interact.ts` or `render/`.
 */

import { seedFor } from "../core/seed";
import { GAME_VERSION } from "../version";
import { plainFinish } from "./finish";
import { turnedBox } from "./footprints";
import { wallAnchor } from "./sites";
import type {
  Box,
  InteriorKind,
  InteriorPiece,
  LightZone,
  RoomSpec,
  Side,
  WallSlot,
} from "./types";
import { CELL } from "./units";

/** The console room's cells along each side: a 6 by 6 hall, 12 m across. */
export const CONSOLE_CELLS = 6;

/** The console room's ceiling height, in metres. */
export const CONSOLE_CEILING = 4;

/**
 * How far out from the south wall the doorway's zone reaches, in metres
 * (C11): the player's centre within this of the wall is in it. The player
 * stops `PLAYER_RADIUS` (0.35 m) from the wall, so walking into the doors
 * always reaches it.
 */
export const EXIT_REACH = 0.6;

/**
 * Half the width of the doorway's zone, in metres (C11), either side of
 * the inner doors' centre line `EXIT_X`.
 */
export const EXIT_HALF = 1.0;

/** The inner doors' centre line, in metres from the west wall (C3). */
export const EXIT_X = 6;

/**
 * Each fitting kind's footing and size (C5 to C8): `flush` on a wall (its
 * back on the wall at `d = 0`, out to `depth`) or `free` (centred on its
 * point), `width` along the wall or across, `depth` out from the wall or
 * front to back, `top` its height and `variants` how many it has. The
 * roundel walls and the inner doors run to 0.05 m under the 4 m ceiling.
 */
export const INTERIOR_CATALOGUE: Record<
  InteriorKind,
  {
    footing: "flush" | "free";
    width: number;
    depth: number;
    top: number;
    variants: number;
  }
> = {
  "roundel-wall": {
    footing: "flush",
    width: 2.0,
    depth: 0.06,
    top: 3.95,
    variants: 3,
  },
  "inner-doors": {
    footing: "flush",
    width: 4.0,
    depth: 0.1,
    top: 3.95,
    variants: 1,
  },
  scanner: {
    footing: "flush",
    width: 2.0,
    depth: 0.14,
    top: 3.95,
    variants: 1,
  },
  console: { footing: "free", width: 2.4, depth: 2.4, top: 2.45, variants: 1 },
};

/**
 * The box a fitting makes the player walk round, in metres: the console's
 * `width` by `depth` centred on its point and turned with it (C8: the box
 * round the desk, so at the hexagon's corners the player stops a little
 * short), and null for a flush piece, which hangs on a wall the grid
 * already stops the player at.
 */
export function interiorFootprint(p: InteriorPiece): Box | null {
  const entry = INTERIOR_CATALOGUE[p.kind];
  if (entry.footing === "flush") return null;
  const hw = entry.width / 2;
  const hd = entry.depth / 2;
  return turnedBox(p.x, p.y, p.turn, { a0: -hw, a1: hw, d0: -hd, d1: hd });
}

/** The console room's light: one steady level for all four zones (C3). */
const CONSOLE_LIGHT = 208;

/** The side of a zone of the console room's light, in cells (C3). */
const ZONE = 3;

/** The walls in the order the pieces are laid: north, east, south, west. */
const SIDES: readonly Side[] = ["n", "e", "s", "w"];

/** The south row's cells the inner doors span (C3). */
const DOOR_CELLS: readonly number[] = [2, 3];

/** The south row's cell whose edge holds the scanner (C3). */
const SCANNER_CELL = 4;

/** The wall edge of `side` at position `i` along it. */
function edgeAt(side: Side, i: number): WallSlot {
  const last = CONSOLE_CELLS - 1;
  switch (side) {
    case "n":
      return { x: i, y: 0, side };
    case "e":
      return { x: last, y: i, side };
    case "s":
      return { x: i, y: last, side };
    case "w":
      return { x: 0, y: i, side };
  }
}

/**
 * The room's fittings (C3): every wall edge north, east, south and west in
 * turn, each cell along its side in order, holds one flush piece at its
 * wall point (`wallAnchor`). The south row's cells 2 and 3 share one piece,
 * the inner doors, anchored between them at `(3, 6)`; cell 4's south edge
 * holds the scanner; every other edge a wall of roundels whose variant is
 * `seedFor(roomSeed, "roundel", x, y, side) % 3` (C5), so about two walls
 * in three carry a few glowing roundels. The console stands last, at the
 * centre `(3, 3)`, turn 0.
 */
function interiorOf(seed: number): InteriorPiece[] {
  const out: InteriorPiece[] = [];
  const piece = (
    kind: InteriorKind,
    variant: number,
    at: { x: number; y: number; turn: number },
    pieceSeed: number,
  ): InteriorPiece => ({
    kind,
    variant,
    x: at.x,
    y: at.y,
    turn: at.turn,
    seed: pieceSeed,
  });
  for (const side of SIDES) {
    for (let i = 0; i < CONSOLE_CELLS; i++) {
      const edge = edgeAt(side, i);
      const at = wallAnchor(edge);
      if (side === "s" && DOOR_CELLS.includes(i)) {
        // One piece for the two edges, at the point between them.
        if (i !== DOOR_CELLS[0]) continue;
        out.push(
          piece(
            "inner-doors",
            0,
            { x: i + 1, y: at.y, turn: at.turn },
            seedFor(seed, "inner-doors"),
          ),
        );
        continue;
      }
      if (side === "s" && i === SCANNER_CELL) {
        out.push(piece("scanner", 0, at, seedFor(seed, "scanner")));
        continue;
      }
      const s = seedFor(seed, "roundel", edge.x, edge.y, edge.side);
      out.push(
        piece(
          "roundel-wall",
          s % INTERIOR_CATALOGUE["roundel-wall"].variants,
          at,
          s,
        ),
      );
    }
  }
  const mid = CONSOLE_CELLS / 2;
  out.push(
    piece("console", 0, { x: mid, y: mid, turn: 0 }, seedFor(seed, "console")),
  );
  return out;
}

/**
 * The room's light (C3): four steady zones of 3 by 3 cells at level 208,
 * each seeded `seedFor(roomSeed, "light", zoneIndex)`: even white light
 * and four lamp panels.
 */
function lightsOf(seed: number): LightZone[] {
  const out: LightZone[] = [];
  for (let y0 = 0; y0 < CONSOLE_CELLS; y0 += ZONE)
    for (let x0 = 0; x0 < CONSOLE_CELLS; x0 += ZONE)
      out.push({
        x0,
        y0,
        x1: x0 + ZONE,
        y1: y0 + ZONE,
        level: CONSOLE_LIGHT,
        special: "steady",
        seed: seedFor(seed, "light", out.length),
      });
  return out;
}

/**
 * The console room (C1, C3): a 6 by 6 cell hall with no bays and no
 * corridor, 4 m high, archetype `bridge`, condition `clean`, seed
 * `seedFor(GAME_VERSION, "console-room")`, no domain, permalink or title
 * (the session gives it the domain and permalink of the room left, C12).
 * The entrance is the west cell of the inner doors, `(2, 5)`; the spawn
 * `(2.5, 4.7)`, facing north, is the point (6, 10.4) m: 1.6 m in front of
 * the doors on their centre line, looking at the console. Its fittings
 * are `interiorOf`'s, its light `lightsOf`'s. The same call gives the same
 * room byte for byte.
 */
export function consoleRoom(): RoomSpec {
  const seed = seedFor(GAME_VERSION, "console-room");
  return {
    version: GAME_VERSION,
    seed,
    domain: "",
    permalink: "",
    title: "",
    archetype: "bridge",
    condition: "clean",
    width: CONSOLE_CELLS,
    depth: CONSOLE_CELLS,
    grid: Array.from({ length: CONSOLE_CELLS }, () =>
      ".".repeat(CONSOLE_CELLS),
    ),
    hall: { x0: 0, y0: 0, x1: CONSOLE_CELLS, y1: CONSOLE_CELLS },
    bays: [],
    corridor: null,
    entrance: { x: 2, y: 5 },
    ceiling: CONSOLE_CEILING,
    spawn: { x: 2.5, y: 4.7, yaw: 0 },
    fixtures: [],
    decor: [],
    scaffold: [],
    heroes: [],
    props: [],
    curios: [],
    finish: plainFinish(0),
    decals: [],
    interior: interiorOf(seed),
    lights: lightsOf(seed),
    dropped: 0,
    inboundMore: 0,
  };
}

/**
 * Whether the player at `p` (metres) stands in the inner doors' doorway
 * (C11): the centre within `EXIT_REACH` of the south wall and within
 * `EXIT_HALF` of the doors' centre line `EXIT_X`. The arrival spot, 1.6 m
 * out, is outside it.
 */
export function atConsoleExit(p: { x: number; z: number }): boolean {
  const wall = CONSOLE_CELLS * CELL;
  return wall - p.z <= EXIT_REACH && Math.abs(p.x - EXIT_X) <= EXIT_HALF;
}
