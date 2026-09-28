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
 *
 * Two hand-built rooms, the console room (`consoleRoom.ts`) and the
 * airlock (`airlock.ts`), also carry `interior`: their fittings, a family
 * of their own (`InteriorPiece`). No generated room has the key, and an
 * absent key is not written by `JSON.stringify`, so the goldens do not see
 * it.
 *
 * The station's other spaces (a deck's hub, the airlock, a hangar) carry
 * `space`, the kind of space they are; an engram room never has the key,
 * so the engram goldens do not see it either. A hangar alone carries
 * `hangar` (`HangarSpec`: its bay door, landing pads and gantries).
 *
 * `StationAddress` is a third family beside them: not what a room is built
 * from or turns into, but where in the station one is - the pure address
 * space `paths.ts`, `world/folders.ts` and the session agree on.
 */

/** Where a reference lands, once the graph located it. */
export interface PlaceAddress {
  domain: string;
  permalink: string;
}

/**
 * Where the game places a player (M3 C1): the pure address space the
 * session, the routes and the generators agree on, kept apart from
 * `PlaceAddress` because the airlock and a deck have neither an engram nor
 * a permalink of their own.
 *
 * `folder` is the raw tree path a folder is found at, `""` for a domain's
 * root. A deck's `section` is 0-based; `null` means unresolved - the section
 * that holds the engram the player walked up from, or the first, resolved
 * once the deck is built - so a session only ever holds and compares
 * resolved addresses (`sameStation` in `paths.ts` treats `null` as unequal
 * to any number, never as 0).
 */
export type StationAddress =
  | { kind: "airlock" }
  | { kind: "bridge"; domain: string }
  | { kind: "deck"; domain: string; folder: string; section: number | null }
  | { kind: "engram"; domain: string; permalink: string };

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
  /**
   * Where it lands; null when unresolved or not located (then the fixture
   * is sealed).
   */
  address: PlaceAddress | null;
  /** The title of the engram it lands on, when the graph located it. */
  targetTitle: string | null;
  /** Its salience, when known; it picks the door style. */
  targetSalience: number | null;
  /**
   * The `type` of the engram it lands on (2.6f C10): a string, null when
   * that engram carries none, and absent when it is unknown (a sealed way,
   * or a target the graph did not return). The generator reads it only to
   * know the archetype of the room this way leads to.
   */
  targetType?: string | null;
  /**
   * The local name of the domain the bracket text's prefix names, read
   * through the listing's spelling table (`domainNames.ts`), so a prefix
   * spelled with a canonical name or an alias names its domain here too.
   * Null for no prefix and for a prefix no domain answers to, which both
   * read at home. Absent when the listing could not be read; the generator
   * then compares the prefix as written.
   */
  targetDomain?: string | null;
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
  /**
   * The `type` of the engram that points here (2.6f C10): a string, null
   * when it carries none, absent when the graph did not return it.
   */
  type?: string | null;
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
 * - A `lift` (M3 C7, C26) is lift doors with a call panel: `stops` are the
 *   places it rides to, in the order its panel and overlay list them
 *   (`world/lifts.ts`), and `note` a status line only (`NO DECKS`,
 *   `?DECK LIST ERROR`, `?DOMAIN LIST ERROR`) or null. The overflow line
 *   `+N MORE` is never stored: the text layer works it out from
 *   `stops.length` against `LIFT_LINES`.
 * - A `screen` (M3 C8, C20, C24) is a wall screen of plain lines: a deck's
 *   name and count, the bridge's domain, the airlock's directory. `keys`
 *   are the indices of the lines drawn with the key pictogram (a private
 *   domain).
 * - An `exit` (M3 C28) is the sliding door on an engram room's entrance
 *   edge that leads up to its deck: `label` its `DECK <n> <FOLDER>` and
 *   `to` the station address it opens onto.
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
      /** Which recipe draws it (2.7 C1); absent reads as 0 (2.7 C6). */
      variant?: number;
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
      /** Which recipe draws it (2.7 C1); absent reads as 0 (2.7 C6). */
      variant?: number;
    }
  | {
      kind: "poster";
      slot: WallSlot;
      category: string;
      lines: string[];
      seed: number;
    }
  | { kind: "placard"; slot: WallSlot; lines: string[] }
  | {
      kind: "lift";
      slot: WallSlot;
      stops: LiftStop[];
      /** A status line, or null; never the `+N MORE` overflow line. */
      note: string | null;
      seed: number;
    }
  | {
      kind: "screen";
      slot: WallSlot;
      lines: string[];
      /** The indices of the lines drawn with the key pictogram. */
      keys: number[];
      seed: number;
      /**
       * The airlock's directory (M3 C24): a board `SCREEN_SCALE` times the
       * wall screen's size, hung high over the outer hatch. Absent on every
       * other screen, so their goldens carry no such key.
       */
      large?: true;
    }
  | {
      kind: "exit";
      slot: WallSlot;
      label: string;
      to: StationAddress;
      seed: number;
    };

/**
 * One stop a lift rides to (M3 C7, C24): the `label` its panel and overlay
 * show, the station address `to` it rides to, `key` when the stop is a
 * private domain (drawn with the key pictogram), and `here` when it is the
 * place the lift stands in (the current section of a deck, the domain the
 * player came from in the airlock), which the overlay marks.
 */
export interface LiftStop {
  label: string;
  to: StationAddress;
  key: boolean;
  here: boolean;
}

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
  /** Which recipe draws it (2.7 C1); absent reads as 0 (2.7 C6). */
  variant?: number;
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
  | "pipe-riser"
  // rare kinds (2.6d C10)
  | "saucer-poster";

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
  | "cable-coil"
  // tall floor kinds (E5), at least TALL_MIN tall
  | "crate-stack"
  | "drum-rack"
  | "gas-rack"
  | "potted-tree"
  // rare kinds (2.6d C10)
  | "ooze-canisters"
  | "designer-tower"
  | "gravity-console"
  | "marked-crate";

/** The kinds of prop that hang from the ceiling, out of the player's way. */
export type CeilingPropKind =
  | "duct"
  | "ceiling-tray"
  | "cable-loop"
  | "beacon"
  // condition extra
  | "loose-cable"
  // spans a line across the hall's interior, stopping one cell short of
  // every wall
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
 * The hero props (H17): rare, large pieces a room is remembered by, named
 * by their shape. They are not `PropKind`s: they have their own list
 * (`RoomSpec.heroes`), their own footprints and their own pass (`heroes.ts`).
 */
export type HeroKind =
  | "turret"
  | "black-slab"
  | "eye-panel"
  | "photo-console"
  | "laser-desk"
  | "mess-table"
  | "helper-robot"
  | "sleep-ring"
  | "dome-planters"
  | "core-wall"
  | "gun-rack"
  | "gun-bench"
  | "tube-bench"
  | "field-pack"
  | "arcade-cabinet"
  | "recruit-cabinet"
  | "stone-hand"
  | "question-block"
  | "mech-head"
  | "red-bike"
  | "hoverboard"
  | "flying-cloud"
  | "spider-tank"
  | "garden-robot"
  | "moon-rocket"
  | "thunder-hammer"
  | "police-box"
  | "slab-walker";

/**
 * Where a hero kind stands (H8): flush on one or two hall wall edges
 * (`wall`, no collision), on the floor against a hall wall edge (`backed`),
 * free in the hall's interior band (`band`), centred on a hall corner zone
 * (`corner`), at the hall centre (`centre`, the slab), or free on the
 * hall's floor with a walkway all round it and its front to the hall's
 * centre (`open`, the police box, 2.6e).
 */
export type HeroPlacement =
  "wall" | "backed" | "band" | "corner" | "centre" | "open";

/**
 * One hero prop. `x` and `y` follow `Decor`'s continuous cell units: the
 * centre of its box for a band, corner or centre hero, and its wall point
 * for a wall or backed one (for a two-edge wall hero, the point where its
 * two edges meet). `turn` is 0-3 quarter turns clockwise from facing north;
 * a wall-anchored hero takes its wall's `turnForSide`. `variant` indexes
 * `FOOTPRINTS.hero[kind]` (an arcade cabinet's variant is its game). `seed`
 * is the seed of the candidate it was accepted on (H18).
 */
export interface Hero {
  kind: HeroKind;
  variant: number;
  x: number;
  y: number;
  turn: number;
  seed: number;
}

/**
 * The fittings of the hand-built rooms, named by their shape: the console
 * room's (2.6e C2) wall of roundels, inner doors, scanner and console, and
 * the airlock's (M3 C24) outer hatch, the amber beacons beside it, the
 * iris light in its ceiling and its suit lockers. They are not heroes: no
 * pool draws them and no generated room carries them.
 */
export type InteriorKind =
  | "roundel-wall"
  | "inner-doors"
  | "scanner"
  | "console"
  | "outer-hatch"
  | "beacon"
  | "iris-light"
  | "suit-locker";

/**
 * One fitting of a hand-built room, with a hero's conventions: `x` and `y`
 * in `Decor`'s continuous cell units, the centre of its box for a free or
 * overhead piece (the console, the iris light) and its wall point for a
 * piece on a wall (`wallAnchor` in `sites.ts`; the inner doors, two edges
 * wide, at the point between their two edges; a beacon or a locker moved
 * along its wall from an edge's point); `turn` 0-3 quarter turns clockwise
 * from facing north, a wall piece taking its wall's `turnForSide`;
 * `variant` indexes the kind's variants in `INTERIOR_CATALOGUE`
 * (`consoleRoom.ts`); `seed` its own seed.
 */
export interface InteriorPiece {
  kind: InteriorKind;
  variant: number;
  x: number;
  y: number;
  turn: number;
  seed: number;
}

/**
 * What a surface is for (C3): a desk top, a bench top, a table, a shelf
 * level or cabinet top, or an `under` spot for the trap and the case -
 * ordinarily below a host's top (the floor in its knee space, or a lower
 * shelf), but the service trolley's own deck top counts too: `under`
 * names the curio slot, not the geometry.
 */
export type SurfaceClass = "desk" | "bench" | "table" | "shelf" | "under";

/**
 * A rectangle a curio may stand on (C3), in its host's local terms at turn
 * 0, the terms the host's model is built in: `a` along its width, `d`
 * along its depth (from the wall for a wall host, from the centre for a
 * free one), `h` the height of the surface in metres, `clear` the free
 * height above it and `cls` what it is for.
 */
export interface SurfaceSpec {
  a0: number;
  a1: number;
  d0: number;
  d1: number;
  h: number;
  clear: number;
  cls: SurfaceClass;
}

/** The small curios (C19), named by their shape. */
export type CurioKind =
  | "light-sword"
  | "green-pistol"
  | "pink-gadget"
  | "wing-meter"
  | "pocket-console"
  | "tape-drive"
  | "tape-player"
  | "video-tape"
  | "beige-laptop"
  | "star-ball"
  | "catch-ball"
  | "trap-box"
  | "fuel-case"
  | "treasure-radar"
  | "capsule-case"
  | "reactor-case"
  | "hover-drone"
  | "breadbin-computer"
  | "slim-computer"
  | "space-bricks"
  | "soot-puffs";

/**
 * One curio (C1): `x` and `y` in `Decor`'s continuous cell units, the
 * centre of its box; `h` the height of the surface it stands on, in
 * metres (0 on the floor); `turn` 0 to 3 quarter turns; `variant` indexes
 * `CURIO_CATALOGUE[kind].sizes`; `seed` the seed of the candidate it was
 * accepted on (C9).
 */
export interface Curio {
  kind: CurioKind;
  variant: number;
  x: number;
  y: number;
  h: number;
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

/** The classic light specials, the ones the station uses. */
export type LightSpecial = "steady" | "glow" | "flicker" | "strobe" | "failing";

/** A rectangle of cells sharing one light level and special. */
export interface LightZone {
  x0: number;
  y0: number;
  /** Exclusive. */
  x1: number;
  /** Exclusive. */
  y1: number;
  /** The light scale, 0 to 255. */
  level: number;
  special: LightSpecial;
  seed: number;
}

/**
 * A room's finish (2.7 C8, C11): its accent's index in the look's accent
 * set, and the wall pattern of its hall, each overflow bay and its
 * corridor.
 *
 * `accent` is 0 to `ACCENT_COUNT - 1` (`variants.ts`), an index into
 * `Look.accents`, so the room keeps its colour family in every look. The
 * wall patterns are 0 to `WALL_PATTERNS - 1` (`finish.ts`): `bayWalls` has
 * one entry per overflow bay, in `RoomSpec.bays` order, and neither a bay's
 * nor the corridor's pattern is ever the hall's in a generated room. A room
 * with no corridor still carries `corridorWalls`.
 */
export interface Finish {
  accent: number;
  hallWalls: number;
  bayWalls: number[];
  corridorWalls: number;
}

/**
 * The decal kinds (2.7 C14), and the airlock's hazard `ring` round its
 * floor's centre (M3 C24), which no generated room lays.
 */
export type DecalKind =
  "chevrons" | "arrow" | "grime" | "streak" | "rust" | "stencil" | "ring";

/**
 * The words a floor stencil may read instead of a bay (M3 C24): `cycle`,
 * the airlock's floor stencil inside its hazard ring.
 */
export type StencilWord = "cycle";

/** What a decal lies on: a wall edge, the floor, or a large crate's face. */
export type DecalOn = "wall" | "floor" | "face";

/**
 * A stencil's text (2.7 C18, C19): its deck, its bay number, and its bay's
 * letter (0 none, 1 to 4 for A to D); `lines` 2 for the wall's two lines,
 * 1 for a floor's one.
 */
export interface StencilText {
  deck: number;
  bay: number;
  letter: number;
  lines: 1 | 2;
}

/**
 * One decal (2.7 C13). `x` and `y` in `Decor`'s continuous cell units:
 * the centre on the floor, the edge's wall point (`wallAnchor`) on a
 * wall, and the face's middle at floor level on a face. `turn` 0 to 3: on
 * the floor the way the decal's top points (0 north), on a wall or face
 * the turn of the surface's frame (`turnForSide`, or the crate's own face
 * turn). `along` moves it along a wall or face, in metres, in its frame's
 * `along` direction. `h` its bottom edge in metres (0 on the floor).
 * `width` across and `length` up (wall, face) or along `turn` (floor), in
 * metres. `variant` picks the kind's tile. `stencil` only on a stencil
 * that reads a deck and a bay, `word` only on one that reads a word (M3
 * C24): a stencil carries one or the other.
 *
 * A `ring` (M3 C24) lies on the floor centred on `x`, `y`: `width` its
 * outer diameter and `length` the width of its band, in metres, drawn in
 * the chevrons' hazard stripes all the way round; `turn`, `along` and `h`
 * are 0.
 */
export interface Decal {
  kind: DecalKind;
  on: DecalOn;
  x: number;
  y: number;
  turn: number;
  along: number;
  h: number;
  width: number;
  length: number;
  variant: number;
  seed: number;
  stencil?: StencilText;
  word?: StencilWord;
}

/**
 * A landing pad on a hangar's floor (M3 C14): a square of cells, `x1` and
 * `y1` exclusive, its own seed, and the hero envelope it keeps for the
 * hangar heroes the pad hero seam stands on it (`withPadHeroes`), `w`
 * across, `l` along and `h` up, in metres.
 */
export interface Pad {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  seed: number;
  envelope: { w: number; l: number; h: number };
}

/**
 * An overhead gantry in a hangar (M3 C15): a beam along row `y`'s centre,
 * wall to wall, at height `h` in metres, standing on a leg at each end
 * (`gantryLegEdges`).
 */
export interface Gantry {
  y: number;
  h: number;
}

/**
 * A hangar's structure (M3 C14, C15): the bay door across the north wall's
 * cells `x0` to `x1` (exclusive), `h` metres tall, shut and decorative; the
 * landing pads; the gantries overhead. Structure, not props: the room mesh
 * draws it.
 */
export interface HangarSpec {
  bayDoor: { x0: number; x1: number; h: number };
  pads: Pad[];
  gantries: Gantry[];
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
  /**
   * The kind of station space a room is when it is not an engram's room
   * (M3 C9, C14, C24): `airlock` for the domain overview, `deck` for a
   * folder's hub, `hangar` for the hangar variant of a deck. Absent on
   * every engram room (`generateRoom` never sets it), so the engram
   * goldens carry no such key. The builders set it right after
   * `permalink`, which is where the goldens read it.
   */
  space?: "airlock" | "deck" | "hangar";
  /**
   * A hangar's structure (M3 C14, C15): its bay door, its landing pads and
   * its gantries, drawn by the room mesh and read by the dressing (the pads
   * and the gantry legs are taken boxes, `world/hangarShape.ts`) and the
   * decals (the pad stencils). Present on a hangar alone (`space:
   * "hangar"`), absent on every other room, so no other golden sees it; the
   * builder sets it right after `space`.
   */
  hangar?: HangarSpec;
  title: string;
  archetype: Archetype;
  condition: Condition;
  /** Cells west to east, of the whole grid. */
  width: number;
  /** Cells north to south, of the whole grid. */
  depth: number;
  /**
   * One string per row, `"."` for floor and `" "` for void, each `width`
   * long.
   */
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
   * The hero props (`placeHeroes` in `heroes.ts`), at most `heroCap` of
   * them, placed after the scaffold and before the dressing, which keeps
   * off what they reserve. Sorted by `HERO_ORDER`.
   */
  heroes: Hero[];
  /**
   * The set dressing (`dressRoom` in `dress.ts`): pure decoration, never
   * data, placed after everything else so it never moves a fixture.
   */
  props: Prop[];
  /**
   * The curios (`placeCurios` in `curios.ts`, C1): small things on the
   * room's desks, benches, tables and shelves and under its desks, placed
   * last, after the dressing. They never collide and move nothing.
   */
  curios: Curio[];
  /**
   * The room's finish (`finishFor` in `finish.ts`, 2.7 C8, C11): its
   * accent and the wall pattern of its hall, each overflow bay and its
   * corridor. Written after the curios and before the lights, the goldens'
   * key order. A hand-built room takes `plainFinish`.
   */
  finish: Finish;
  /**
   * The decals (`placeDecals` in `decals.ts`, 2.7 C13 to C19): chevrons,
   * arrows, grime, streaks, rust and the deck and bay stencils, placed
   * after the curios as pure data, never over text. Written after the
   * finish and before the lights, the goldens' key order. A hand-built
   * room lays its own or none: the airlock its hazard ring and its two
   * `CYCLE` stencils, the others none.
   */
  decals: Decal[];
  /**
   * The fittings of a hand-built room (the console room's, 2.6e C2; the
   * airlock's, M3 C24); absent on every generated room. Drawn instanced
   * as their own family (`interiorInstances`), and the free-standing and
   * wall-backed ones collide (`interiorFootprint`).
   */
  interior?: InteriorPiece[];
  lights: LightZone[];
  /** How many fixtures found no wall slot and were left out. */
  dropped: number;
  /** Inbound references past the hatches, named on the placard. */
  inboundMore: number;
}
