/**
 * The prop catalogue: every kind of set dressing the dressing pass can
 * place, and the tables that decide what goes where.
 *
 * Props are pure decoration. They never encode data; the room's content
 * speaks only through its fixtures. This module holds no placement logic,
 * only:
 * - `PROP_CATALOGUE`, one `PropEntry` per kind, saying its anchor, how many
 *   variants it has, whether it is a wall or ceiling run, whether it stands
 *   only against a free wall, whether it is a condition extra a palette
 *   never picks, whether it is a wide wall kind that reaches `WIDE_REACH` on
 *   both sides of its anchor, whether it keeps clear (a person reads or uses
 *   it, so nothing may stand in front of it), whether it is a ceiling span
 *   that crosses the hall rather than hugging a wall, and whether it is a
 *   tall floor kind (`TALL_MIN` or more in every variant, E5);
 * - `PALETTES`, the weighted picks each archetype draws free wall edges,
 *   floor spots, mid-hall clusters and runs from, plus `FILLER`, a low
 *   weight given to a few utility kinds in every archetype's wall palette
 *   so a palette with few wall entries (the archive's signs) does not read
 *   as a shop of one thing (ruling 8);
 * - `EXTRAS`, the condition-only kinds and their counts (ruling 12): they
 *   are placed before the regular floor props, on the same floor spots, and
 *   a regular palette never draws one of them, which is why the `extra`
 *   kinds, like the toppled crate, are marked apart in `PROP_CATALOGUE`
 *   rather than folded into a palette. `wallBacked` (ruling 9) is a
 *   separate constraint, only against a free wall: a wall-backed kind like
 *   the fume cabinet still sits in a regular palette;
 * - the density and geometry constants placement reads: `PROP_CAP`,
 *   `WALL_SHARE`, `WALL_SIDE_SHARE`, `LOOP_SHARE`, `EXTINGUISHER_EVERY`,
 *   `LANE_WIDTH`, `LANE_DEPTH`, `USE_LANE_DEPTH`, `TALL_MIN`, the cluster
 *   constants
 *   (`CLUSTER_BLOCK`, `CLUSTER_INNER`, `CLUSTER_SHARE`, `CLUSTER_MIN`,
 *   `CLUSTER_MAX` and `CLUSTER_CLEAR`) and the span constants
 *   (`SPAN_CELLS`, `SPAN_HALF` and `SPAN_SHARE`).
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
  /**
   * A wide wall kind: its model reaches at least `WIDE_REACH` on both sides
   * of its anchor at turn 0 (D4). Never true together with `run`.
   */
  wide: boolean;
  /**
   * A kind a person reads or uses at standing height, so no wall-side floor
   * prop may stand in front of it (D2 as amended: corner-zone spots ignore
   * it): every mandatory kind is keep-clear, and some optional kinds are
   * too.
   */
  keepClear: boolean;
  /**
   * A ceiling kind that crosses the hall's interior in a straight line of
   * segments, rather than hugging a wall (D9). Never true together with
   * `run` or `extra`.
   */
  span: boolean;
  /**
   * A floor kind whose every variant stands at least `TALL_MIN` tall; the
   * density measure counts them, `propModels.test.ts` pins it against
   * geometry.
   */
  tall: boolean;
}

/** An archetype's weighted picks and density knobs. */
export interface Palette {
  /** Weighted picks for free wall edges, before the filler. */
  wall: readonly (readonly [WallPropKind, number])[];
  /** The wall run kind, or null. */
  wallRun: "cable-tray" | "pipe-bundle" | null;
  /**
   * Weighted picks for floor spots. Every palette holds at least one tall
   * kind (E5), weighted 2 beside its plain kinds at 3, so a large hall
   * shows some height along its walls and in its corners.
   */
  floor: readonly (readonly [FloorPropKind, number])[];
  /** The ceiling run kind, or null. */
  ceilingRun: "duct" | "ceiling-tray" | null;
  /**
   * The ceiling span kind hung across a large hall (D9), or null for none:
   * engineering's decor already runs pipes along its ceiling, and the
   * council's ceiling stays bare. An archetype whose decor can include a
   * `pipe-run` must have none, since the pipe runs already cross its hall
   * overhead (`props.test.ts` holds the two together).
   */
  ceilingSpan: "span-duct" | "span-tray" | null;
  /** Corner-zone props: 1 always (bridge) or 1 to 2. */
  cornerMax: 1 | 2;
  /**
   * Chance a free wall-side cell gets a floor prop: 1/4 on the bridge, 1/2
   * everywhere else (E7). Keep-clear wall props still close the spots in
   * front of them.
   */
  wallSide: number;
  /**
   * Weighted picks for the members of a mid-hall cluster (D6): plain floor
   * kinds, never an extra and never a wall-backed kind, since a cluster
   * stands in the open, away from every wall.
   */
  cluster: readonly (readonly [FloorPropKind, number])[];
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
  "tool-board",
  "conduit-cabinet",
  "stowage-net",
  "pipe-riser",
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
  "crate-stack",
  "drum-rack",
  "gas-rack",
  "potted-tree",
  // ceiling
  "duct",
  "ceiling-tray",
  "cable-loop",
  "beacon",
  "loose-cable",
  "span-duct",
  "span-tray",
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
    wide: true,
    keepClear: false,
    span: false,
    tall: false,
  },
  extinguisher: {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: true,
    span: false,
    tall: false,
  },
  "first-aid": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: true,
    span: false,
    tall: false,
  },
  intercom: {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: true,
    span: false,
    tall: false,
  },
  "keycard-reader": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: true,
    span: false,
    tall: false,
  },
  "vent-grille": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "sign-plate": {
    anchor: "wall",
    variants: 6,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: true,
    span: false,
    tall: false,
  },
  "breaker-box": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "wall-monitor": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: true,
    span: false,
    tall: false,
  },
  "padded-panel": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: true,
    keepClear: false,
    span: false,
    tall: false,
  },
  "light-strip": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "cable-tray": {
    anchor: "wall",
    variants: 2,
    run: true,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "pipe-bundle": {
    anchor: "wall",
    variants: 2,
    run: true,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "tool-board": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: true,
    keepClear: false,
    span: false,
    tall: false,
  },
  "conduit-cabinet": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: true,
    keepClear: false,
    span: false,
    tall: false,
  },
  "stowage-net": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: true,
    keepClear: false,
    span: false,
    tall: false,
  },
  "pipe-riser": {
    anchor: "wall",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: true,
    keepClear: false,
    span: false,
    tall: false,
  },
  crate: {
    anchor: "floor",
    variants: 3,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  barrel: {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  trolley: {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  stool: {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "filing-cabinet": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "storage-shelf": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: true,
  },
  planter: {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  bench: {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "specimen-shelf": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: true,
  },
  "fume-cabinet": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: true,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: true,
  },
  "traffic-cone": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: true,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  ladder: {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: true,
    extra: true,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "tool-cart": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: true,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "toppled-crate": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: true,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "debris-pile": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: true,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "cable-coil": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: true,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "crate-stack": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: true,
  },
  "drum-rack": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: true,
  },
  "gas-rack": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: true,
  },
  "potted-tree": {
    anchor: "floor",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: true,
  },
  duct: {
    anchor: "ceiling",
    variants: 2,
    run: true,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "ceiling-tray": {
    anchor: "ceiling",
    variants: 2,
    run: true,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "cable-loop": {
    anchor: "ceiling",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  beacon: {
    anchor: "ceiling",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "loose-cable": {
    anchor: "ceiling",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: true,
    wide: false,
    keepClear: false,
    span: false,
    tall: false,
  },
  "span-duct": {
    anchor: "ceiling",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: true,
    tall: false,
  },
  "span-tray": {
    anchor: "ceiling",
    variants: 2,
    run: false,
    wallBacked: false,
    extra: false,
    wide: false,
    keepClear: false,
    span: true,
    tall: false,
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

/**
 * The five archetypes' weighted picks and density knobs. Every wall palette
 * holds at least one wide kind, weighted 4 to 6, above its small kinds at 2
 * (D4), so a free edge draws a prop that fills it more often than one that
 * dots it; the sign plate keeps 3 where it is the archive's and council's
 * own pick. `FILLER` stays at 1 each.
 */
export const PALETTES = {
  bridge: {
    wall: [
      ["wall-monitor", 2],
      ["intercom", 2],
      ["light-strip", 2],
      ["conduit-cabinet", 5],
      ["locker-bank", 3],
    ],
    wallRun: "cable-tray",
    floor: [
      ["stool", 3],
      ["crate", 3],
      ["crate-stack", 2],
    ],
    ceilingRun: "ceiling-tray",
    ceilingSpan: "span-tray",
    cornerMax: 1,
    wallSide: 1 / 4,
    cluster: [
      ["crate", 3],
      ["trolley", 2],
    ],
  },
  engineering: {
    wall: [
      ["breaker-box", 2],
      ["vent-grille", 2],
      ["locker-bank", 4],
      ["tool-board", 4],
      ["pipe-riser", 4],
    ],
    wallRun: "pipe-bundle",
    floor: [
      ["barrel", 3],
      ["crate", 3],
      ["trolley", 3],
      ["crate-stack", 2],
      ["drum-rack", 2],
    ],
    ceilingRun: "duct",
    ceilingSpan: null,
    cornerMax: 2,
    wallSide: 1 / 2,
    cluster: [
      ["crate", 3],
      ["barrel", 3],
      ["trolley", 2],
    ],
  },
  archive: {
    wall: [
      ["sign-plate", 3],
      ["conduit-cabinet", 4],
      ["stowage-net", 4],
    ],
    wallRun: null,
    floor: [
      ["filing-cabinet", 3],
      ["storage-shelf", 3],
      ["crate", 3],
      ["crate-stack", 2],
    ],
    ceilingRun: "ceiling-tray",
    ceilingSpan: "span-tray",
    cornerMax: 2,
    wallSide: 1 / 2,
    cluster: [
      ["crate", 3],
      ["trolley", 2],
      ["filing-cabinet", 1],
    ],
  },
  lab: {
    wall: [
      ["first-aid", 2],
      ["extinguisher", 2],
      ["vent-grille", 2],
      ["tool-board", 4],
      ["pipe-riser", 4],
    ],
    wallRun: null,
    floor: [
      ["fume-cabinet", 3],
      ["specimen-shelf", 3],
      ["stool", 3],
      ["trolley", 3],
      ["gas-rack", 2],
    ],
    ceilingRun: "duct",
    ceilingSpan: "span-duct",
    cornerMax: 2,
    wallSide: 1 / 2,
    cluster: [
      ["trolley", 3],
      ["crate", 2],
    ],
  },
  council: {
    wall: [
      ["padded-panel", 6],
      ["sign-plate", 3],
    ],
    wallRun: null,
    floor: [
      ["planter", 3],
      ["bench", 3],
      ["potted-tree", 2],
    ],
    ceilingRun: null,
    ceilingSpan: null,
    cornerMax: 2,
    wallSide: 1 / 2,
    cluster: [
      ["planter", 3],
      ["crate", 1],
    ],
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
/**
 * Depth of a clear lane from the wall in front of a door, hatch or portal,
 * in metres: the ways out keep the full 4 m, since a sliding door opens
 * from `APPROACH` (3 m) and nothing was shown to hold at less (E6).
 */
export const LANE_DEPTH = 4.0;
/**
 * Depth of the clear lane in front of a terminal or a machine, in metres
 * (E6). A fixture is used from within `REACH` (2.2 m) of its wall point, so
 * its whole usable depth lies inside the lane, and the use point the
 * reachability test aims at (at most 1.45 m out) keeps the player's circle
 * inside it too. Shallower than `LANE_DEPTH`, it frees the second-row cells
 * of a corner zone near a fixture for floor props.
 */
export const USE_LANE_DEPTH = 2.5;
/**
 * Width of the viewing lane in front of a poster or the placard, in metres:
 * the sheet's width along its wall. Floor props never enter it, so nothing
 * stands between the player and a sheet (its bottom edge hangs at 1.2 m,
 * lower than most floor props are tall).
 */
export const SHEET_LANE_WIDTH = 1.2;
/** Depth of the viewing lane in front of a poster or the placard, in metres. */
export const SHEET_LANE_DEPTH = 1.5;

/** A wide wall prop reaches at least this far either side of its anchor, in metres (D4). */
export const WIDE_REACH = 0.8;
/**
 * The wall band's depth, in metres: the world side's copy of the models'
 * `FLUSH_DEPTH`, pinned equal by `propModels.test.ts` (D3). `dress.ts` must
 * not import from `render/`, so this constant lives here rather than being
 * read from the models.
 */
export const WALL_PROP_DEPTH = 0.3;
/**
 * The least a tall floor kind's model stands, in metres (E5): every variant
 * of a kind marked `tall` in `PROP_CATALOGUE` reaches at least this height,
 * under `FLOOR_TOP` (2.2), so it reads over a cluster of ordinary props in a
 * large hall. `propModels.test.ts` pins it against the built geometry.
 */
export const TALL_MIN = 1.6;
/**
 * The side of a mid-hall cluster block, in cells (D6). The hall's interior
 * band is tiled from its north-west corner into blocks this size, partial
 * blocks dropped, and a cluster uses only a block's inner 2 by 2 cells, so
 * two clusters always stand at least 2 cells (4 m) apart.
 */
export const CLUSTER_BLOCK = 4;
/**
 * The side of a cluster block's inner square, in cells: the block's spots
 * are its cells `bx + 1` to `bx + CLUSTER_INNER` in both axes, one cell in
 * from its north-west corner, so a partial last row or column of the block
 * (when `CLUSTER_INNER` falls short of `CLUSTER_BLOCK - 1`) stays outside
 * the square and keeps two clusters' inner cells at least one cell apart.
 */
export const CLUSTER_INNER = 2;
/** Chance a cluster block holds a cluster (D6, D8). */
export const CLUSTER_SHARE = 1 / 2;
/** The fewest members a cluster is drawn with (D6). */
export const CLUSTER_MIN = 2;
/** The most members a cluster is drawn with (D6). */
export const CLUSTER_MAX = 3;
/**
 * How far a cluster member keeps clear of everything solid outside its own
 * cluster, in metres (D7): its box grown by this much overlaps no taken box
 * and no other floor prop. The player's circle is 0.7 m across, so a
 * cluster narrows a way but never closes one.
 */
export const CLUSTER_CLEAR = 1.0;
/** How many cells long one span segment is, in cell units (D9). */
export const SPAN_CELLS = 2;
/**
 * Half a span's width across its line, in metres: the plan box `sites.ts`
 * keeps clear of lamps and decor (D9).
 */
export const SPAN_HALF = 0.35;
/**
 * Chance a large hall whose palette has a `ceilingSpan` hangs one span line
 * (D9), drawn first from the room's own span stream, so some large halls
 * carry one and the rest stay clear overhead.
 */
export const SPAN_SHARE = 2 / 3;
