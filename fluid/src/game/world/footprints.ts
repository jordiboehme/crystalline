/**
 * The floor each thing that stands in a room takes: what the player collides
 * with, what the detailed models must stay inside and what the generator
 * keeps clear when it puts up scaffolding, places heroes or dresses a room.
 * The things are fixtures, decor, floor props and the hero props
 * (`FOOTPRINTS.hero`, `heroFootprint`, `heroBlocker`), whose sizes stand
 * apart from the floor props' so `MAX_FLOOR_PROP` stays true of every
 * floor prop kind (H2). The three hovering heroes' lifts (`HERO_LIFT`,
 * `heroLift`, C4) live here too, since collision reads them: a hero lifted
 * to `HERO_WALK_UNDER` or more hangs over the player's head and does not
 * block.
 *
 * It also holds the one local-to-world transform (`turnedPoint`, C4, and
 * `turnedBox` over it) that the hero helpers and the curios' host surfaces
 * share, and `OPEN_CLEAR`, the free height an open top promises a curio
 * (C3).
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
  FloorPropKind,
  Fixture,
  Hero,
  HeroKind,
  MachineKind,
  Prop,
  PropKind,
  Rect,
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
 * No floor prop's footprint is wider or deeper than this, in metres: a
 * member of a 2 m corner-zone cell stands at least 0.3 m off each wall,
 * clear of the wall band (`WALL_PROP_DEPTH` 0.3), and two mid-hall clusters
 * one empty cell apart (`CLUSTER_INNER`) stay 2.6 m apart, past the 1.0 m
 * ring (`CLUSTER_CLEAR`).
 */
export const MAX_FLOOR_PROP = 1.4;

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
  /** floor props, one size per variant */
  prop: Readonly<Record<FloorPropKind, readonly FloorSize[]>>;
  /**
   * The hero props, one size per variant, width by depth at turn 0: for a
   * wall-anchored kind the width runs along the wall and the depth out from
   * it (see `heroFootprint`).
   */
  hero: Readonly<Record<HeroKind, readonly FloorSize[]>>;
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
 * the piece. A shelf row is two cells long. The hero sizes are the
 * catalogue's (H8, `heroes.ts`); no hero is limited by `MAX_FLOOR_PROP`.
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
  prop: {
    crate: [
      { width: 0.8, depth: 0.8 },
      { width: 1.2, depth: 1.2 },
      { width: 1.2, depth: 1.2 },
    ],
    barrel: [
      { width: 0.65, depth: 0.65 },
      { width: 1.35, depth: 1.25 },
    ],
    trolley: [
      { width: 1.0, depth: 0.6 },
      { width: 1.1, depth: 0.65 },
    ],
    stool: [
      { width: 0.45, depth: 0.45 },
      { width: 0.5, depth: 0.5 },
    ],
    "filing-cabinet": [
      { width: 0.5, depth: 0.65 },
      { width: 0.5, depth: 0.65 },
    ],
    "storage-shelf": [
      { width: 1.2, depth: 0.5 },
      { width: 1.0, depth: 0.5 },
    ],
    planter: [
      { width: 0.7, depth: 0.7 },
      { width: 1.2, depth: 0.5 },
    ],
    bench: [
      { width: 1.4, depth: 0.5 },
      { width: 1.4, depth: 0.6 },
    ],
    "specimen-shelf": [
      { width: 1.0, depth: 0.45 },
      { width: 1.2, depth: 0.45 },
    ],
    "fume-cabinet": [
      { width: 1.2, depth: 0.75 },
      { width: 1.0, depth: 0.75 },
    ],
    "traffic-cone": [
      { width: 0.4, depth: 0.4 },
      { width: 0.4, depth: 0.4 },
    ],
    ladder: [
      { width: 0.55, depth: 0.8 },
      { width: 0.55, depth: 0.9 },
    ],
    "tool-cart": [
      { width: 0.9, depth: 0.55 },
      { width: 1.0, depth: 0.6 },
    ],
    "toppled-crate": [
      { width: 1.3, depth: 1.0 },
      { width: 1.3, depth: 1.3 },
    ],
    "debris-pile": [
      { width: 1.3, depth: 1.1 },
      { width: 1.4, depth: 1.2 },
    ],
    "cable-coil": [
      { width: 0.8, depth: 0.8 },
      { width: 1.0, depth: 0.7 },
    ],
    "crate-stack": [
      { width: 1.2, depth: 1.2 },
      { width: 1.3, depth: 1.1 },
    ],
    "drum-rack": [
      { width: 1.35, depth: 0.9 },
      { width: 1.3, depth: 1.0 },
    ],
    "gas-rack": [
      { width: 1.0, depth: 0.5 },
      { width: 1.2, depth: 0.6 },
    ],
    "potted-tree": [
      { width: 0.9, depth: 0.9 },
      { width: 1.0, depth: 1.0 },
    ],
  },
  hero: {
    turret: [{ width: 0.9, depth: 0.9 }],
    "black-slab": [{ width: 1.2, depth: 0.3 }],
    "eye-panel": [{ width: 0.9, depth: 0.25 }],
    "photo-console": [{ width: 1.8, depth: 0.9 }],
    "laser-desk": [{ width: 2.4, depth: 3.0 }],
    "mess-table": [{ width: 5.0, depth: 2.6 }],
    "helper-robot": [{ width: 1.2, depth: 1.4 }],
    "sleep-ring": [{ width: 5.0, depth: 5.0 }],
    "dome-planters": [
      { width: 3.2, depth: 1.6 },
      { width: 4.8, depth: 1.6 },
    ],
    "core-wall": [{ width: 3.6, depth: 0.3 }],
    "gun-rack": [{ width: 1.8, depth: 0.3 }],
    "gun-bench": [{ width: 1.9, depth: 0.9 }],
    "tube-bench": [{ width: 1.9, depth: 0.9 }],
    "field-pack": [
      { width: 0.9, depth: 0.7 },
      { width: 1.0, depth: 0.8 },
    ],
    "arcade-cabinet": [
      { width: 0.8, depth: 0.9 },
      { width: 0.8, depth: 0.9 },
      { width: 0.8, depth: 0.9 },
    ],
    "recruit-cabinet": [{ width: 1.0, depth: 1.2 }],
    "stone-hand": [{ width: 0.9, depth: 0.9 }],
    "question-block": [{ width: 0.6, depth: 0.6 }],
    "mech-head": [{ width: 1.6, depth: 2.0 }],
    "red-bike": [{ width: 3.0, depth: 1.0 }],
    hoverboard: [{ width: 0.9, depth: 0.25 }],
    "flying-cloud": [{ width: 1.0, depth: 1.9 }],
    "spider-tank": [{ width: 2.4, depth: 3.5 }],
    "garden-robot": [{ width: 1.8, depth: 1.5 }],
    "moon-rocket": [{ width: 1.4, depth: 1.4 }],
    "thunder-hammer": [{ width: 0.8, depth: 0.8 }],
    "police-box": [{ width: 1.3, depth: 1.3 }],
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

/**
 * The floor a prop takes: its variant's size from `FOOTPRINTS.prop`,
 * centred on `(x * CELL, y * CELL)` and turned like `decorFootprint`, width
 * and depth swapped at a quarter or three-quarter turn. Null for a wall or
 * ceiling prop, which does not collide. Throws on a variant index out of
 * range for the kind, and in the same words on a floor-anchored prop whose
 * kind has no floor sizes at all (a wall or ceiling kind): a generator bug
 * should not pass silently.
 */
export function propFootprint(prop: Prop): Box | null {
  if (prop.anchor !== "floor") return null;
  const table: Partial<Record<PropKind, readonly FloorSize[]>> =
    FOOTPRINTS.prop;
  const size = table[prop.kind]?.[prop.variant];
  if (size === undefined) {
    throw new Error(
      `propFootprint: ${prop.kind} has no variant ${String(prop.variant)}`,
    );
  }
  const sideways = prop.turn % 2 === 1;
  const hx = (sideways ? size.depth : size.width) / 2;
  const hz = (sideways ? size.width : size.depth) / 2;
  const cx = prop.x * CELL;
  const cz = prop.y * CELL;
  return { x0: cx - hx, x1: cx + hx, z0: cz - hz, z1: cz + hz };
}

/**
 * How a hero kind meets the floor: `flush` on its wall (a wall kind, no
 * collision), `backed` against its wall (the box runs out from the wall
 * point, like a fixture's footprint), or `free` (the box is centred on the
 * anchor). `heroes.ts` pins it against each kind's placement.
 */
export const HERO_FOOTING = {
  turret: "free",
  "black-slab": "free",
  "eye-panel": "flush",
  "photo-console": "backed",
  "laser-desk": "free",
  "mess-table": "free",
  "helper-robot": "free",
  "sleep-ring": "free",
  "dome-planters": "free",
  "core-wall": "flush",
  "gun-rack": "flush",
  "gun-bench": "backed",
  "tube-bench": "backed",
  "field-pack": "free",
  "arcade-cabinet": "backed",
  "recruit-cabinet": "backed",
  "stone-hand": "free",
  "question-block": "free",
  "mech-head": "free",
  "red-bike": "free",
  hoverboard: "free",
  "flying-cloud": "free",
  "spider-tank": "free",
  "garden-robot": "free",
  "moon-rocket": "free",
  "thunder-hammer": "free",
  "police-box": "backed",
} as const satisfies Record<HeroKind, "flush" | "backed" | "free">;

/**
 * The heroes that hover (C4): each one's underside, in metres above the
 * floor. Its mesh is built at that height (the instance still stands at
 * `y` 0), the model test's float check starts from it (`looseParts`'
 * `floor`), and its lowest vertex is exactly this height. These three
 * are the only exceptions to the float check, and `heroes.test.ts` names
 * them, so a fourth is a conscious change.
 */
export const HERO_LIFT = {
  "question-block": 2.3,
  hoverboard: 0.25,
  "flying-cloud": 0.4,
} as const satisfies Partial<Record<HeroKind, number>>;

/** A hero kind that hovers. */
export type LiftedHeroKind = keyof typeof HERO_LIFT;

/** A hero kind's lift (`HERO_LIFT`), 0 for every kind that stands on the floor. */
export function heroLift(kind: HeroKind): number {
  return (HERO_LIFT as Partial<Record<HeroKind, number>>)[kind] ?? 0;
}

/**
 * The lift from which a hero hangs over a walking player's head and
 * stops blocking (C5), in metres: well over the eye (1.6 m) and a head.
 * Only the question block (2.3) reaches it; the hoverboard and the cloud
 * block by their footprint.
 */
export const HERO_WALK_UNDER = 2.0;

/**
 * The way a thing at each quarter turn faces, as `[x, z]`: north at turn 0,
 * then east, south and west. A wall-anchored hero's box runs this way from
 * its wall point; the dev seam stands the player this way from a hero.
 */
export const HERO_FRONT: readonly (readonly [number, number])[] = [
  [0, -1],
  [1, 0],
  [0, 1],
  [-1, 0],
];

/**
 * How much free height an open top promises a curio (C3), in metres: a
 * lit sword in its stand (1.22) with a margin. The host-surface render
 * test pins that nothing of the host stands in that box.
 */
export const OPEN_CLEAR = 1.3;

/**
 * Where a local point `(a, d)` of a thing anchored at `(x, y)` (cell units)
 * and turned `turn` quarter turns lies, in world metres: `a` along its
 * width and `d` along its depth, in the terms the models are built in
 * (`frameAt` and `frameForSlot` in `render/kit.ts`). The point is
 * `anchor * CELL + along * a + front * d`, with `front = HERO_FRONT[turn]`
 * and `along = [front[1], -front[0]]`. Every helper that turns a local
 * point into the world goes through here (`heroPoint`, `hostSurfaces`),
 * so they all mirror the kit alike (C4).
 */
export function turnedPoint(
  x: number,
  y: number,
  turn: number,
  a: number,
  d: number,
): { x: number; z: number } {
  const t = ((Math.round(turn) % 4) + 4) % 4;
  const [fx, fz] = HERO_FRONT[t] ?? [0, -1];
  return { x: x * CELL + fz * a + fx * d, z: y * CELL - fx * a + fz * d };
}

/**
 * The world box, in metres, of a local rectangle `a0..a1` by `d0..d1` of a
 * thing anchored at `(x, y)` (cell units) and turned `turn` quarter turns:
 * its four corners through `turnedPoint`, the box spanning their extremes.
 * The one corner loop the hero tops and under spots (`heroSurfaces`,
 * `heroUnder`) and the curios' host surfaces (`hostSurfaces`) share.
 */
export function turnedBox(
  x: number,
  y: number,
  turn: number,
  r: { a0: number; a1: number; d0: number; d1: number },
): Box {
  const xs: number[] = [];
  const zs: number[] = [];
  for (const a of [r.a0, r.a1])
    for (const d of [r.d0, r.d1]) {
      const p = turnedPoint(x, y, turn, a, d);
      xs.push(p.x);
      zs.push(p.z);
    }
  return {
    x0: Math.min(...xs),
    x1: Math.max(...xs),
    z0: Math.min(...zs),
    z1: Math.max(...zs),
  };
}

/** A hero's quarter turn, 0 to 3, whatever whole number `turn` holds. */
export function heroTurn(h: Hero): number {
  return ((Math.round(h.turn) % 4) + 4) % 4;
}

/**
 * The floor a hero takes, in metres: its variant's size, width and depth
 * swapped at an odd turn, centred on the anchor for a `free` footing, and
 * for a wall-anchored one centred along the wall and running `depth` out
 * from the wall point along `HERO_FRONT`, so a backed hero's box equals
 * `footprint(edge, { along: width, out: depth })` exactly. A flush hero has
 * a box too (what the model checks start from), though it does not collide.
 *
 * `depth` overrides the variant's depth, keeping its width: a flush hero's
 * view box (`heroReserve` in `heroes.ts`) is its footprint `HERO_VIEW`
 * deep. Throws on a variant the kind does not have, in the words
 * `propFootprint` uses.
 */
export function heroFootprint(h: Hero, depth?: number): Box {
  const size = FOOTPRINTS.hero[h.kind][h.variant];
  if (size === undefined)
    throw new Error(
      `heroFootprint: ${h.kind} has no variant ${String(h.variant)}`,
    );
  const deep = depth ?? size.depth;
  const t = heroTurn(h);
  const sideways = t % 2 === 1;
  const hx = (sideways ? deep : size.width) / 2;
  const hz = (sideways ? size.width : deep) / 2;
  const ax = h.x * CELL;
  const az = h.y * CELL;
  if (HERO_FOOTING[h.kind] === "free")
    return { x0: ax - hx, x1: ax + hx, z0: az - hz, z1: az + hz };
  // Out from the wall point by the whole depth, measured from the wall
  // itself rather than from a centre, so the box's wall face and far face
  // come out exactly where `footprint` puts them.
  const [fx, fz] = HERO_FRONT[t] ?? [0, -1];
  const out = (a: number, f: number, half: number): [number, number] =>
    f === 0 ? [a - half, a + half] : f > 0 ? [a, a + deep] : [a - deep, a];
  const [x0, x1] = out(ax, fx, hx);
  const [z0, z1] = out(az, fz, hz);
  return { x0, x1, z0, z1 };
}

/**
 * What the player collides with: `heroFootprint`, or null for a flush
 * hero and for one that hovers over the player's head (`heroLift` at
 * least `HERO_WALK_UNDER`, C5).
 */
export function heroBlocker(h: Hero): Box | null {
  if (HERO_FOOTING[h.kind] === "flush") return null;
  if (heroLift(h.kind) >= HERO_WALK_UNDER) return null;
  return heroFootprint(h);
}

/** The longest pipe run, in metres. */
export const PIPE_MAX = 6;
/** How much shorter than the hall along its run a pipe run stays, in metres. */
export const PIPE_CLEARANCE = 1;
/**
 * Half a pipe run's width across its line, in metres: the brackets that
 * hang the pipes are the widest part. The model (`render/models/decor.ts`)
 * builds its brackets to this half-width, and `pipeRunBox` boxes it.
 */
export const PIPE_HALF = 0.3;

/**
 * How long a pipe run is: at most `PIPE_MAX` (6 m), and `PIPE_CLEARANCE`
 * (1 m) shorter than the hall is along the run (along x at turns 0 and 2,
 * along the grid's y at 1 and 3), so it never pokes through a wall. It lives
 * here, on the world side, so the generator can keep ceiling spans off the
 * pipe runs without importing a model (E3); the model reads it back.
 */
export function pipeLength(decor: Decor, hall: Rect): number {
  const cells = decor.turn % 2 === 0 ? hall.x1 - hall.x0 : hall.y1 - hall.y0;
  return Math.max(0, Math.min(PIPE_MAX, cells * CELL - PIPE_CLEARANCE));
}

/**
 * The plan box of a pipe run, in metres: `pipeLength / 2` either way along
 * its axis (x at turns 0 and 2, the grid's y at 1 and 3) and `PIPE_HALF`
 * across, centred on `(decor.x * CELL, decor.y * CELL)`. A pipe run hangs
 * from the ceiling, so it takes no floor (`decorFootprint` gives null), but
 * it hangs in the band a ceiling span hangs in: `sites.ts` keeps every span
 * line off this box. Null for any other kind of decor, and for a run whose
 * length is 0.
 */
export function pipeRunBox(decor: Decor, hall: Rect): Box | null {
  if (decor.kind !== "pipe-run") return null;
  const half = pipeLength(decor, hall) / 2;
  if (half <= 0) return null;
  const sideways = decor.turn % 2 === 1;
  const hx = sideways ? PIPE_HALF : half;
  const hz = sideways ? half : PIPE_HALF;
  const cx = decor.x * CELL;
  const cz = decor.y * CELL;
  return { x0: cx - hx, x1: cx + hx, z0: cz - hz, z1: cz + hz };
}
