/**
 * The floor each thing that stands in a room takes: what the player collides
 * with, what the detailed models must stay inside and what the generator
 * keeps clear when it puts up scaffolding or dresses a room.
 *
 * This is a leaf of the world: it imports only `units.ts` and `types.ts`,
 * so the generator (`generate.ts`) and the walking code (`move.ts`) can both
 * read it without importing each other. `footprints.test.ts` keeps it so.
 */

import { CELL } from "./units";
import type {
  Box,
  Decor,
  DecorKind,
  Fixture,
  MachineKind,
  WallSlot,
} from "./types";

/**
 * How deep a fixture stands out from its wall, in metres: the default
 * wall-fixture footprint, and every machine kind's depth unless
 * `FOOTPRINTS` says otherwise.
 */
export const FIXTURE_DEPTH = 0.9;
/**
 * How wide along the wall, in metres: the default wall-fixture footprint of
 * `footprint`, kept from milestone 1. The detailed models are narrower and
 * say so in `FOOTPRINTS`.
 */
export const FIXTURE_WIDTH = 2;

/**
 * A footprint against a wall, in metres: `along` the wall and `out` from it
 * into the room.
 */
export interface WallSize {
  along: number;
  out: number;
}

/**
 * A free-standing footprint, in metres, as the piece stands at turn 0
 * (facing north): `width` along x and `depth` along the grid's y (the
 * world's z). A quarter turn swaps the two.
 */
export interface FloorSize {
  width: number;
  depth: number;
}

/** The shape of `FOOTPRINTS`. */
export interface Footprints {
  terminal: WallSize;
  machine: Readonly<Record<MachineKind, WallSize>>;
  /** Null for decor the player walks under or past (the ceiling pipe runs). */
  decor: Readonly<Record<DecorKind, FloorSize | null>>;
}

/** Every machine kind that is not given a size of its own. */
const MACHINE_DEFAULT: WallSize = { along: 1.8, out: FIXTURE_DEPTH };

/**
 * The floor each thing that stands in a room takes, in metres: what the
 * player collides with and what the detailed models must stay inside.
 *
 * Terminals and machines stand against their wall slot's wall, centred on
 * the slot's cell; doors, portals, hatches, posters and the placard are flush
 * with the wall and take no floor at all, so they are not listed. A server
 * rack, a cryo pod and a comms array have sizes of their own and every other
 * machine is 1.8 m by 0.9 m. Decor is centred on its point and turned with
 * the piece. A shelf row is two cells long.
 */
export const FOOTPRINTS: Footprints = {
  terminal: { along: 1.4, out: 0.9 },
  machine: {
    workbench: MACHINE_DEFAULT,
    "lab-bench": MACHINE_DEFAULT,
    "server-rack": { along: 0.8, out: 1.0 },
    "cryo-pod": { along: 1.1, out: 1.0 },
    fabricator: MACHINE_DEFAULT,
    hydroponics: MACHINE_DEFAULT,
    "nav-table": MACHINE_DEFAULT,
    "comms-array": { along: 1.2, out: 0.6 },
    "reactor-coupling": MACHINE_DEFAULT,
    "cargo-loader": MACHINE_DEFAULT,
    "med-scanner": MACHINE_DEFAULT,
    containment: MACHINE_DEFAULT,
  },
  decor: {
    "command-console": { width: 3.0, depth: 1.0 },
    "captain-chair": { width: 0.8, depth: 0.8 },
    "round-table": { width: 2.4, depth: 2.4 },
    "council-chair": { width: 0.7, depth: 0.7 },
    generator: { width: 2.0, depth: 2.0 },
    "pipe-run": null,
    "shelf-row": { width: 2 * CELL, depth: 0.8 },
    "lab-island": { width: 3.0, depth: 1.4 },
    "specimen-tank": { width: 0.9, depth: 0.9 },
  },
};

/**
 * The floor footprint of something standing against its wall, centred on
 * the slot's cell: `along` metres wide along the wall and `out` metres deep
 * into the room. Without a size it is milestone 1's default wall fixture,
 * `FIXTURE_WIDTH` by `FIXTURE_DEPTH`.
 */
export function footprint(
  slot: WallSlot,
  size: WallSize = { along: FIXTURE_WIDTH, out: FIXTURE_DEPTH },
): Box {
  const cx = (slot.x + 0.5) * CELL;
  const cz = (slot.y + 0.5) * CELL;
  const half = size.along / 2;
  switch (slot.side) {
    case "n":
      return {
        x0: cx - half,
        x1: cx + half,
        z0: slot.y * CELL,
        z1: slot.y * CELL + size.out,
      };
    case "s":
      return {
        x0: cx - half,
        x1: cx + half,
        z0: (slot.y + 1) * CELL - size.out,
        z1: (slot.y + 1) * CELL,
      };
    case "w":
      return {
        x0: slot.x * CELL,
        x1: slot.x * CELL + size.out,
        z0: cz - half,
        z1: cz + half,
      };
    case "e":
      return {
        x0: (slot.x + 1) * CELL - size.out,
        x1: (slot.x + 1) * CELL,
        z0: cz - half,
        z1: cz + half,
      };
  }
}

/**
 * The floor a fixture takes, sized by its kind from `FOOTPRINTS`: a
 * terminal's desk, or a machine by its `MachineKind`. Doors, portals,
 * hatches, posters and the placard are flush with their wall and give null:
 * walking up to a door or hatch is how it gets used, so it must not block.
 */
export function footprintOf(fixture: Fixture): Box | null {
  switch (fixture.kind) {
    case "terminal":
      return footprint(fixture.slot, FOOTPRINTS.terminal);
    case "machine":
      return footprint(fixture.slot, FOOTPRINTS.machine[fixture.machine]);
    case "door":
    case "portal":
    case "hatch":
    case "poster":
    case "placard":
      return null;
  }
}

/**
 * The floor a piece of furniture takes: its `FOOTPRINTS` size centred on the
 * piece's point, with width and depth swapped at a quarter or three-quarter
 * turn. Null for a pipe run, which hangs from the ceiling.
 */
export function decorFootprint(decor: Decor): Box | null {
  const size = FOOTPRINTS.decor[decor.kind];
  if (size === null) return null;
  const sideways = decor.turn % 2 === 1;
  const hx = (sideways ? size.depth : size.width) / 2;
  const hz = (sideways ? size.width : size.depth) / 2;
  const cx = decor.x * CELL;
  const cz = decor.y * CELL;
  return { x0: cx - hx, x1: cx + hx, z0: cz - hz, z1: cz + hz };
}
