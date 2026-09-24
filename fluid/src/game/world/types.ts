/**
 * The shapes the generator speaks: what a place is made from, and the room
 * it becomes.
 *
 * `PlaceInput` is the content side, narrowed to what the station draws. It is
 * filled from the engram detail and the neighbourhood graph the reading
 * screen already fetches, so the game never needs an endpoint of its own.
 *
 * `RoomSpec` is the other side: plain JSON, integer cells, no GPU handles and
 * no DOM. Same input and same `GAME_VERSION` give the same `RoomSpec` byte
 * for byte, which is what the golden tests pin and what lets a live change be
 * diffed room against room.
 */

/** A relation or prose wikilink, as the engram detail parses it. */
export interface PlaceReference {
  /** The relation type, or null for a prose wikilink. */
  relType: string | null;
  /** Where the brackets point: a domain prefix (or null) and the text. */
  target: { domain: string | null; target: string };
  /** Whether the index found something at the other end. */
  resolved: boolean;
  /** The title of the engram it lands on, when the graph located it. */
  targetTitle: string | null;
  /** Its salience, when known; it picks the door style. */
  targetSalience: number | null;
}

/** One place, narrowed to what the station draws. */
export interface PlaceInput {
  domain: string;
  permalink: string;
  title: string;
  type: string | null;
  status: string | null;
  salience: number | null;
  tags: readonly string[];
  /** The markdown as written; the `## ` sections become terminals. */
  content: string;
  relations: readonly PlaceReference[];
  links: readonly PlaceReference[];
}

/** A `## ` section of the body. */
export interface Section {
  heading: string;
  /** Its first non-empty lines, markup left as written. */
  lines: string[];
}

/** How the room looks after its status. */
export type Condition = "clean" | "construction" | "dim" | "derelict";

/** The kind of room a type becomes. */
export type Archetype =
  "bridge" | "council" | "engineering" | "archive" | "lab";

/** The door that fits a target's salience. */
export type DoorStyle = "sliding" | "bulkhead" | "blast";

/** Which wall of its cell a fixture stands against. */
export type Side = "n" | "e" | "s" | "w";

/** A place against a wall: the cell and the side of it. */
export interface WallSlot {
  x: number;
  y: number;
  side: Side;
}

/** The twelve machines a tag can become. */
export type MachineKind =
  | "workbench"
  | "lab-bench"
  | "server-rack"
  | "cryo-pod"
  | "fabricator"
  | "hydroponics"
  | "nav-table"
  | "comms-array"
  | "reactor-coupling"
  | "cargo-loader"
  | "med-scanner"
  | "containment";

/** Anything placed in a room. */
export type Fixture =
  | {
      kind: "terminal";
      slot: WallSlot;
      heading: string;
      lines: string[];
      seed: number;
    }
  | {
      kind: "door";
      slot: WallSlot;
      style: DoorStyle;
      relType: string;
      label: string;
      target: string;
      seed: number;
    }
  | {
      kind: "portal";
      slot: WallSlot;
      label: string;
      target: string;
      crossDomain: boolean;
      sealed: boolean;
      seed: number;
    }
  | {
      kind: "machine";
      slot: WallSlot;
      machine: MachineKind;
      tag: string;
      hue: number;
      seed: number;
    }
  | { kind: "placard"; slot: WallSlot; lines: string[] };

/** DOOM's light specials, the ones the station uses. */
export type LightSpecial = "steady" | "glow" | "flicker" | "strobe" | "failing";

/** A rectangle of cells sharing one light level and special. */
export interface LightZone {
  x0: number;
  y0: number;
  /** Exclusive. */
  x1: number;
  /** Exclusive. */
  y1: number;
  /** DOOM's scale, 0 to 255. */
  level: number;
  special: LightSpecial;
  seed: number;
}

/** A room, ready to be meshed. */
export interface RoomSpec {
  version: number;
  seed: number;
  domain: string;
  permalink: string;
  title: string;
  archetype: Archetype;
  condition: Condition;
  /** Cells east to west. */
  width: number;
  /** Cells north to south. */
  depth: number;
  /** Ceiling height in metres. */
  ceiling: number;
  /** Where the player enters: a cell, facing into the room. */
  spawn: { x: number; y: number; yaw: number };
  fixtures: Fixture[];
  lights: LightZone[];
}
