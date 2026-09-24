/**
 * The shapes the generator speaks: what a place is made from, and the room
 * it becomes.
 *
 * `PlaceInput` is the content side, narrowed to what the station draws. It is
 * filled from the engram detail, the neighbourhood graph and the inbound
 * page the reading screen already fetches, so the game never needs an
 * endpoint of its own.
 *
 * `RoomSpec` is the other side: plain JSON, integer cells, no GPU handles and
 * no DOM. Same input and same `GAME_VERSION` give the same `RoomSpec` byte
 * for byte, which is what the golden tests pin and what lets a live change be
 * diffed room against room.
 */

/** Where a reference lands, once the graph located it. */
export interface PlaceAddress {
  domain: string;
  permalink: string;
}

/**
 * A relation or prose wikilink, as the engram detail parses it and the
 * neighbourhood graph places it.
 *
 * Three states, the same three the reading screen draws: resolved with an
 * address is a way through; resolved without one is a target the index found
 * but the graph did not place, which the generator seals as `NO ROUTE`; and
 * unresolved is the honest negative, sealed as not found.
 */
export interface PlaceReference {
  /** The relation type, or null for a prose wikilink. */
  relType: string | null;
  /** The bracket text, for labels and for sorting. */
  target: { domain: string | null; target: string };
  /** The index resolved it. */
  resolved: boolean;
  /** Where it lands; null when unresolved or not located (then the fixture is sealed). */
  address: PlaceAddress | null;
  /** The title of the engram it lands on, when the graph located it. */
  targetTitle: string | null;
  /** Its salience, when known; it picks the door style. */
  targetSalience: number | null;
}

/**
 * An engram that points here: a service hatch. The hatch leads back to it,
 * which is why only located engrams (they come with an address) become one.
 */
export interface PlaceInbound {
  address: PlaceAddress;
  title: string;
  /** The relation it points here with; `links_to` for a prose wikilink. */
  relType: string;
}

/** One observation bullet, in document order. */
export interface PlaceObservation {
  /** The bracket token it opens with, free form. */
  category: string | null;
  content: string;
}

/**
 * How many hatches a room carries at most. Beyond that the placard names the
 * rest as `+N MORE INBOUND` instead of the room growing without bound: an
 * engram a thousand others point at is a hub, and a hub is still one room.
 */
export const HATCH_CAP = 24;

/**
 * One place, narrowed to what the station draws.
 *
 * Every field is data the reading screen already fetches (the detail, the
 * neighbourhood graph and a page of inbound references), mapped by
 * `placeFromDetail` in `data/place.ts`.
 */
export interface PlaceInput {
  domain: string;
  permalink: string;
  title: string;
  type: string | null;
  status: string | null;
  salience: number | null;
  /** `valid_from`. Absent means it has always been valid. */
  validFrom: string | null;
  /** `valid_to`. Absent means it is valid forever. */
  validTo: string | null;
  tags: readonly string[];
  /** The markdown as written; the `## ` sections become terminals. */
  content: string;
  relations: readonly PlaceReference[];
  links: readonly PlaceReference[];
  /** At most HATCH_CAP, sorted by address. */
  inbound: readonly PlaceInbound[];
  /** The true inbound total, for the "+N MORE INBOUND" line. */
  inboundTotal: number;
  observations: readonly PlaceObservation[];
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
