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

/**
 * Anything placed in a room.
 *
 * Every fixture stands in a wall slot and carries a seed of its own, keyed by
 * a name (a target, a tag, an address, a category, a heading and its
 * occurrence), never by its position in a list, so adding one thing leaves
 * the others as they were.
 *
 * - A `terminal` shows a `## ` section: its heading, its first lines, and in
 *   `section` how many sections of the same heading came before it, which is
 *   how the CRT reader finds the right one of two equal headings.
 * - A `door` is an outgoing relation and a `portal` a prose wikilink. Both
 *   carry the place they open onto in `address`; when it is null the way is
 *   sealed and `sealedLabel` says why: `?FILE NOT FOUND` for a target the
 *   index did not resolve, `NO ROUTE` for one it resolved but the graph did
 *   not locate. `sealedLabel` is null exactly when `address` is set.
 * - A `hatch` is an inbound reference: it leads back to the engram that
 *   points here, which is why it always has an address.
 * - A `machine` is a tag, a `poster` the observations of one category, and
 *   the `placard` at the entrance the frontmatter.
 */
export type Fixture =
  | {
      kind: "terminal";
      slot: WallSlot;
      heading: string;
      lines: string[];
      /** The occurrence of this heading among the sections, from 0. */
      section: number;
      seed: number;
    }
  | {
      kind: "door";
      slot: WallSlot;
      style: DoorStyle;
      relType: string;
      label: string;
      address: PlaceAddress | null;
      sealedLabel: string | null;
      seed: number;
    }
  | {
      kind: "portal";
      slot: WallSlot;
      label: string;
      address: PlaceAddress | null;
      crossDomain: boolean;
      sealedLabel: string | null;
      seed: number;
    }
  | {
      kind: "hatch";
      slot: WallSlot;
      label: string;
      address: PlaceAddress;
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
  | {
      kind: "poster";
      slot: WallSlot;
      category: string;
      lines: string[];
      seed: number;
    }
  | { kind: "placard"; slot: WallSlot; lines: string[] };

/**
 * The free-standing furniture of each archetype: the command console and
 * captain's chair of a bridge, the round table and chairs of a council
 * chamber, the generator and ceiling pipe runs of an engineering bay, the
 * shelf rows of an archive, and the island and specimen tanks of a lab.
 */
export type DecorKind =
  | "command-console"
  | "captain-chair"
  | "round-table"
  | "council-chair"
  | "generator"
  | "pipe-run"
  | "shelf-row"
  | "lab-island"
  | "specimen-tank";

/**
 * A free-standing piece of furniture, centred on a floor point, turned by
 * quarter turns.
 *
 * `x` and `y` are continuous cell units, not cell indices: the point
 * `(x, y)` lies at `(x * CELL, y * CELL)` metres, so the centre of cell
 * `(3, 4)` is `(3.5, 4.5)` and a piece centred on a cell border has a whole
 * number there.
 *
 * At `turn` 0 a piece faces north (towards smaller `y`), and the first
 * dimension of its footprint runs along `x` and the second along `y`. Each
 * turn rotates it a quarter clockwise seen from above (north, east, south,
 * west), so at turns 1 and 3 the two dimensions swap axes.
 */
export interface Decor {
  kind: DecorKind;
  /** East-west position of its centre, in cell units. */
  x: number;
  /** North-south position of its centre, in cell units. */
  y: number;
  /** 0-3 quarter turns clockwise from facing north. */
  turn: number;
  seed: number;
}

/** Where a prop hangs: on a wall, on the floor, or from the ceiling. */
export type PropAnchor = "wall" | "floor" | "ceiling";

/** The kinds of prop that hang or stand flush against a wall. */
export type WallPropKind =
  | "locker-bank"
  | "extinguisher"
  | "first-aid"
  | "intercom"
  | "keycard-reader"
  | "vent-grille"
  | "sign-plate"
  | "breaker-box"
  | "wall-monitor"
  | "padded-panel"
  | "light-strip"
  | "cable-tray"
  | "pipe-bundle"
  | "tool-board"
  | "conduit-cabinet"
  | "stowage-net"
  | "pipe-riser";

/** The kinds of prop that stand on the floor and collide with the player. */
export type FloorPropKind =
  | "crate"
  | "barrel"
  | "trolley"
  | "stool"
  | "filing-cabinet"
  | "storage-shelf"
  | "planter"
  | "bench"
  | "specimen-shelf"
  | "fume-cabinet"
  // condition extras
  | "traffic-cone"
  | "ladder"
  | "tool-cart"
  | "toppled-crate"
  | "debris-pile"
  | "cable-coil";

/** The kinds of prop that hang from the ceiling, out of the player's way. */
export type CeilingPropKind =
  | "duct"
  | "ceiling-tray"
  | "cable-loop"
  | "beacon"
  // condition extra
  | "loose-cable"
  // spans the hall wall to wall
  | "span-duct"
  | "span-tray";

/** Every kind of set dressing the dressing pass can place. */
export type PropKind = WallPropKind | FloorPropKind | CeilingPropKind;

/**
 * One piece of set dressing: pure decoration, never data. `x` and `y` follow
 * `Decor`'s continuous cell convention; a wall or ceiling prop sits at its
 * wall edge's wall point (see `wallAnchor` in `sites.ts`). `turn` is 0-3
 * quarter turns clockwise from facing north. `variant` indexes the kind's
 * variants in `PROP_CATALOGUE`. `seed` is
 * `seedFor(roomSeed, "prop", cx, cy, token)` from the integer anchor cell.
 */
export interface Prop {
  kind: PropKind;
  variant: number;
  anchor: PropAnchor;
  x: number;
  y: number;
  turn: number;
  seed: number;
}

/**
 * A rectangle of cells, `x1` and `y1` exclusive, in grid coordinates (the
 * same convention as a light zone).
 */
export interface Rect {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/**
 * An axis-aligned floor rectangle, in metres (not cells), `x` west to east
 * and `z` north to south: what a scaffold frame, a fixture's or a piece of
 * furniture's footprint and the player's blockers are measured in. Unlike a
 * `Rect` it need not sit on the grid.
 */
export interface Box {
  x0: number;
  z0: number;
  x1: number;
  z1: number;
}

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

/**
 * A room, ready to be meshed.
 *
 * The room is a grid of cells (see `world/layout.ts`): a main hall, and
 * where the content needs them, overflow bays east of it and a backlink
 * corridor west of it. `width` and `depth` are the grid's, `hall` is the
 * main hall's rectangle inside it, and `grid` says per cell whether it is
 * floor. Bays and the corridor share the hall's ceiling.
 */
export interface RoomSpec {
  version: number;
  seed: number;
  domain: string;
  permalink: string;
  title: string;
  archetype: Archetype;
  condition: Condition;
  /** Cells west to east, of the whole grid. */
  width: number;
  /** Cells north to south, of the whole grid. */
  depth: number;
  /** One string per row, `"."` for floor and `" "` for void, each `width` long. */
  grid: string[];
  /** The main hall inside the grid; the entrance is on its south wall. */
  hall: Rect;
  /**
   * The overflow bays east of the hall, west to east, as `planLayout` built
   * them; empty when the hall's walls hold everything.
   */
  bays: Rect[];
  /** The backlink corridor west of the hall, or null when there is none. */
  corridor: Rect | null;
  /**
   * The entrance cell, on the hall's south wall: the way in is that cell's
   * `s` edge. The generator states it so the dressing never has to read it
   * back from the spawn.
   */
  entrance: { x: number; y: number };
  /** Ceiling height in metres. */
  ceiling: number;
  /** Where the player enters: a cell, facing into the room. */
  spawn: { x: number; y: number; yaw: number };
  fixtures: Fixture[];
  /** The archetype's free-standing furniture, in the hall. */
  decor: Decor[];
  /**
   * The scaffold frames of a room under construction, in metres, and none
   * in any other room. The generator puts them up once (`scaffoldFor` in
   * `generate.ts`), so the walls the player collides with and the poles the
   * renderer builds are read from the same boxes.
   */
  scaffold: Box[];
  /**
   * The set dressing (`dressRoom` in `dress.ts`): pure decoration, never
   * data, placed after everything else so it never moves a fixture.
   */
  props: Prop[];
  lights: LightZone[];
  /** How many fixtures found no wall slot and were left out. */
  dropped: number;
  /** Inbound references past the hatches, named on the placard. */
  inboundMore: number;
}
