/**
 * The prop catalogue: every kind of set dressing the dressing pass can
 * place, and the tables that decide what goes where.
 *
 * Props are pure decoration. They never encode data; the room's content
 * speaks only through its fixtures. This module holds no placement logic,
 * only:
 * - `PROP_CATALOGUE`, one `PropEntry` per kind, saying its anchor, how many
 *   variants it has, whether it is a wall or ceiling run, whether it stands
 *   only against a free wall, and whether it is a condition extra a palette
 *   never picks;
 * - `PALETTES`, the weighted picks each archetype draws free wall edges,
 *   floor spots and runs from, plus `FILLER`, a low weight given to a few
 *   utility kinds in every archetype's wall palette so a palette with few
 *   wall entries (the archive's signs) does not read as a shop of one thing
 *   (ruling 8);
 * - `EXTRAS`, the condition-only kinds and their counts (ruling 12): they
 *   are placed before the regular floor props, on the same floor spots, and
 *   a regular palette never draws one of them, which is why the `extra`
 *   kinds, like the toppled crate, are marked apart in `PROP_CATALOGUE`
 *   rather than folded into a palette. `wallBacked` (ruling 9) is a
 *   separate constraint, only against a free wall: a wall-backed kind like
 *   the fume cabinet still sits in a regular palette;
 * - the density and geometry constants placement reads: `PROP_CAP`,
 *   `WALL_SHARE`, `WALL_SIDE_SHARE`, `LOOP_SHARE`, `EXTINGUISHER_EVERY`,
 *   `LANE_WIDTH` and `LANE_DEPTH`.
 *
 * Every kind gets 2 variants, the crate 3 and the sign plate 6, its six
 * pictograms (ruling 13).
 *
 * This is a leaf of the generator side: it imports only `types.ts`, so
 * `sites.ts` and `dress.ts` can read it without reaching `move.ts` or
 * `generate.ts` (ruling 20).
 */

import type {
  Archetype,
  Condition,
  FloorPropKind,
  PropAnchor,
  PropKind,
  WallPropKind,
} from "./types";

/** One entry of the catalogue: what a kind is and how it may be placed. */
export interface PropEntry {
  anchor: PropAnchor;
  /** How many variants the models build: 2, 3 for the crate, 6 for the sign plate. */
  variants: number;
  /** A horizontal run, emitted one segment per wall edge (ruling 3). */
  run: boolean;
  /** Stands only against a free wall (ruling 9). */
  wallBacked: boolean;
  /** Placed only as a condition extra, never by a palette (ruling 12). */
  extra: boolean;
}

/** An archetype's weighted picks and density knobs. */
export interface Palette {
  /** Weighted picks for free wall edges, before the filler. */
  wall: readonly (readonly [WallPropKind, number])[];
  /** The wall run kind, or null. */
  wallRun: "cable-tray" | "pipe-bundle" | null;
  /** Weighted picks for floor spots. */
  floor: readonly (readonly [FloorPropKind, number])[];
  /** The ceiling run kind, or null. */
  ceilingRun: "duct" | "ceiling-tray" | null;
  /** Corner-zone props: 1 always (bridge) or 1 to 2. */
  cornerMax: 1 | 2;
  /** Chance a free wall-side cell gets a floor prop. */
  wallSide: number;
}

/** How many of a condition's extra kind to place, drawn once from its own seed. */
export interface ExtraRule {
  kind: PropKind;
  min: number;
  max: number;
}

/** Every kind of prop the dressing pass can place, once each. */
export const PROP_KINDS: readonly PropKind[] = [
  // wall
  "locker-bank",
  "extinguisher",
  "first-aid",
  "intercom",
  "keycard-reader",
  "vent-grille",
  "sign-plate",
  "breaker-box",
  "wall-monitor",
  "padded-panel",
  "light-strip",
  "cable-tray",
  "pipe-bundle",
  // floor
  "crate",
  "barrel",
  "trolley",
  "stool",
  "filing-cabinet",
  "storage-shelf",
  "planter",
  "bench",
  "specimen-shelf",
  "fume-cabinet",
  "traffic-cone",
  "ladder",
  "tool-cart",
  "toppled-crate",
  "debris-pile",
  "cable-coil",
  // ceiling
  "duct",
  "ceiling-tray",
  "cable-loop",
  "beacon",
  "loose-cable",
];

/**
 * The catalogue: what each kind is and how it may be placed. `PROP_KINDS`
 * lists exactly the same kinds, once each.
 */
export const PROP_CATALOGUE = {
  "locker-bank": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  extinguisher: {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "first-aid": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  intercom: {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "keycard-reader": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "vent-grille": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "sign-plate": {
    anchor: "wall",
    variants: 6,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "breaker-box": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "wall-monitor": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "padded-panel": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "light-strip": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "cable-tray": {
    anchor: "wall",
    variants: 2,
    run: true,
    wallBacked: false,
    extra: false,
  },
  "pipe-bundle": {
    anchor: "wall",
    variants: 2,
    run: true,
    wallBacked: false,
    extra: false,
  },
  crate: {
    anchor: "floor",
    variants: 3,
    run: false,
    wallBacked: false,
    extra: false,
  },
  barrel: {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  trolley: {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  stool: {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "filing-cabinet": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "storage-shelf": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  planter: {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  bench: {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "specimen-shelf": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "fume-cabinet": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: true,
    extra: false,
  },
  "traffic-cone": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: true,
  },
  ladder: {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: true,
    extra: true,
  },
  "tool-cart": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: true,
  },
  "toppled-crate": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: true,
  },
  "debris-pile": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: true,
  },
  "cable-coil": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: true,
  },
  duct: {
    anchor: "ceiling",
    variants: 2,
    run: true,
    wallBacked: false,
    extra: false,
  },
  "ceiling-tray": {
    anchor: "ceiling",
    variants: 2,
    run: true,
    wallBacked: false,
    extra: false,
  },
  "cable-loop": {
    anchor: "ceiling",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  beacon: {
    anchor: "ceiling",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
  },
  "loose-cable": {
    anchor: "ceiling",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: true,
  },
} satisfies Record<PropKind, PropEntry>;

/**
 * The low-weight wall filler every archetype's palette gets on top of its
 * own picks (ruling 8): without it the archive's palette, signs alone,
 * covers two thirds of its walls in one kind.
 */
export const FILLER: readonly (readonly [WallPropKind, number])[] = [
  ["vent-grille", 1],
  ["light-strip", 1],
  ["breaker-box", 1],
];

/** The five archetypes' weighted picks and density knobs. */
export const PALETTES = {
  bridge: {
    wall: [
      ["wall-monitor", 3],
      ["intercom", 3],
      ["light-strip", 3],
    ],
    wallRun: "cable-tray",
    floor: [
      ["stool", 3],
      ["crate", 3],
    ],
    ceilingRun: "ceiling-tray",
    cornerMax: 1,
    wallSide: 1 / 8,
  },
  engineering: {
    wall: [
      ["breaker-box", 3],
      ["vent-grille", 3],
      ["locker-bank", 3],
    ],
    wallRun: "pipe-bundle",
    floor: [
      ["barrel", 3],
      ["crate", 3],
      ["trolley", 3],
    ],
    ceilingRun: "duct",
    cornerMax: 2,
    wallSide: 1 / 4,
  },
  archive: {
    wall: [["sign-plate", 3]],
    wallRun: null,
    floor: [
      ["filing-cabinet", 3],
      ["storage-shelf", 3],
      ["crate", 3],
    ],
    ceilingRun: "ceiling-tray",
    cornerMax: 2,
    wallSide: 1 / 4,
  },
  lab: {
    wall: [
      ["first-aid", 3],
      ["extinguisher", 3],
      ["vent-grille", 3],
    ],
    wallRun: null,
    floor: [
      ["fume-cabinet", 3],
      ["specimen-shelf", 3],
      ["stool", 3],
      ["trolley", 3],
    ],
    ceilingRun: "duct",
    cornerMax: 2,
    wallSide: 1 / 4,
  },
  council: {
    wall: [
      ["padded-panel", 3],
      ["sign-plate", 3],
    ],
    wallRun: null,
    floor: [
      ["planter", 3],
      ["bench", 3],
    ],
    ceilingRun: null,
    cornerMax: 2,
    wallSide: 1 / 4,
  },
} satisfies Record<Archetype, Palette>;

/**
 * The condition extras (ruling 12): kinds a regular palette never draws,
 * placed only under their condition, before the regular floor props, on
 * the same floor spots. `clean` gets none.
 */
export const EXTRAS = {
  clean: [],
  construction: [
    { kind: "traffic-cone", min: 2, max: 4 },
    { kind: "ladder", min: 1, max: 1 },
    { kind: "tool-cart", min: 1, max: 1 },
  ],
  dim: [{ kind: "loose-cable", min: 2, max: 4 }],
  derelict: [
    { kind: "toppled-crate", min: 1, max: 2 },
    { kind: "debris-pile", min: 1, max: 1 },
    { kind: "cable-coil", min: 1, max: 3 },
  ],
} satisfies Record<Condition, readonly ExtraRule[]>;

/** At most this many props in one room; beyond it the cap drops the rest. */
export const PROP_CAP = 200;
/** About this share of free wall cells gets a prop. */
export const WALL_SHARE = 2 / 3;
/** Chance a free wall-side cell gets a floor prop, unless the palette says otherwise. */
export const WALL_SIDE_SHARE = 1 / 4;
/** Chance a ceiling tray segment carries a cable loop. */
export const LOOP_SHARE = 1 / 4;
/** An extinguisher goes on the wall run about every this many cells. */
export const EXTINGUISHER_EVERY = 6;
/** Width of a clear lane, in metres. */
export const LANE_WIDTH = 1.6;
/** Depth of a clear lane from the wall, in metres. */
export const LANE_DEPTH = 4.0;
