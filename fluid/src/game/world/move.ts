/**
 * Walking: DOOM's feel, the station's walls.
 *
 * Movement is fast with little inertia - the player reaches full speed in a
 * few ticks and stops almost as quickly - and the head bobs a little with the
 * stride. It advances one 35 Hz tick at a time, so it feels the same at any
 * frame rate.
 *
 * Collision is the player's circle against the room's grid and against the
 * blockers. Every void cell of the grid, and everything outside it, is a
 * solid square, so the walls between the hall, its bays and the backlink
 * corridor stop the player and the doorways between them let the player
 * through. The blockers are one box per free-standing thing: a terminal or
 * machine sized by its kind (`FOOTPRINTS`), each piece of the archetype's
 * furniture turned with it, and the scaffold frames of a room under
 * construction. The grid's bounding rectangle is still clamped to as a
 * backstop. The two axes are resolved one after the other, which is what
 * lets the player slide along a wall instead of stopping dead when walking
 * into it at an angle.
 */

import { TICK_HZ } from "../core/loop";
import { createRng } from "../core/seed";
import { forwardOf, rightOf } from "../gl/math";
import { CELL } from "./generate";
import { isFloor } from "./layout";
import type {
  Decor,
  DecorKind,
  Fixture,
  MachineKind,
  RoomSpec,
  WallSlot,
} from "./types";

/** The player's state after a tick. */
export interface Player {
  x: number;
  z: number;
  vx: number;
  vz: number;
  yaw: number;
  pitch: number;
  /** Stride phase in radians, for the head bob. */
  bob: number;
}

/** What the controls ask for during one tick. */
export interface Intent {
  /** -1 back, 1 forward. */
  forward: number;
  /** -1 left, 1 right. */
  strafe: number;
  /** -1 right, 1 left (arrow keys). */
  turn: number;
  /** Mouse pixels since the last tick. */
  lookDx: number;
  lookDy: number;
}

/** An axis-aligned floor rectangle the player cannot enter. */
export interface Box {
  x0: number;
  z0: number;
  x1: number;
  z1: number;
}

/** The player's collision radius, in metres. */
export const PLAYER_RADIUS = 0.35;
/** Eye height above the floor, in metres. */
export const EYE_HEIGHT = 1.6;
/** How far the camera can pitch up or down, in radians (+-30 degrees). */
export const MAX_PITCH = Math.PI / 6;

const DT = 1 / TICK_HZ;
const MAX_SPEED = 7;
/** Fraction of velocity kept per tick with no input: quick stops. */
const FRICTION = 0.55;
const TURN_SPEED = 3;
const MOUSE_SENSITIVITY = 0.0025;
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

/** How many scaffold frames a room under construction gets. */
const SCAFFOLD_FRAMES = 2;
/** A scaffold frame's side, in metres. */
export const SCAFFOLD_SIZE = 1.4;
/** The hall's margin around its interior band, in cells, as for decor. */
const BAND_MARGIN = 2;

/** The player at the room's entrance. */
export function spawnPlayer(room: RoomSpec): Player {
  return {
    x: (room.spawn.x + 0.5) * CELL,
    z: (room.spawn.y + 0.5) * CELL,
    vx: 0,
    vz: 0,
    yaw: room.spawn.yaw,
    pitch: 0,
    bob: 0,
  };
}

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
 * The scaffold frames of a room under construction, and none for any other
 * room: two frames, `SCAFFOLD_SIZE` metres square, placed from an rng seeded
 * with `room.seed`. Each frame's centre is drawn x then z inside the hall's
 * interior band (the hall without a two-cell margin, the band the decor
 * stands in), inset by half a frame so the whole frame stays inside it. The
 * smallest hall's band is one by two cells, still wide enough for a frame.
 *
 * A frame that would overlap a piece of the decor (`decorFootprint`) or the
 * frame kept before it is skipped rather than moved, so a room under
 * construction shows two frames, one or none. A skipped frame still uses its
 * two draws, so whether the first frame is kept never moves the second.
 *
 * The room mesh builds its poles at exactly these boxes, so what the player
 * sees is what blocks the player.
 */
export function scaffoldBoxes(room: RoomSpec): Box[] {
  if (room.condition !== "construction") return [];
  const half = SCAFFOLD_SIZE / 2;
  const x0 = (room.hall.x0 + BAND_MARGIN) * CELL + half;
  const x1 = (room.hall.x1 - BAND_MARGIN) * CELL - half;
  const z0 = (room.hall.y0 + BAND_MARGIN) * CELL + half;
  const z1 = (room.hall.y1 - BAND_MARGIN) * CELL - half;
  const taken: Box[] = [];
  for (const d of room.decor) {
    const box = decorFootprint(d);
    if (box !== null) taken.push(box);
  }
  const rng = createRng(room.seed);
  const out: Box[] = [];
  for (let i = 0; i < SCAFFOLD_FRAMES; i++) {
    const x = rng.range(x0, x1);
    const z = rng.range(z0, z1);
    const frame = { x0: x - half, x1: x + half, z0: z - half, z1: z + half };
    if (taken.some((b) => intersects(frame, b))) continue;
    taken.push(frame);
    out.push(frame);
  }
  return out;
}

/** True when two boxes share floor; touching edges do not count. */
function intersects(a: Box, b: Box) {
  return a.x0 < b.x1 && b.x0 < a.x1 && a.z0 < b.z1 && b.z0 < a.z1;
}

/**
 * The boxes the player collides with besides the grid: the terminals and
 * machines (`footprintOf`), the furniture (`decorFootprint`) and the
 * scaffold frames (`scaffoldBoxes`). Flush fixtures and pipe runs are left
 * out. Built once per room, not per tick.
 */
export function blockersFor(room: RoomSpec): Box[] {
  const out: Box[] = [];
  for (const f of room.fixtures) {
    const box = footprintOf(f);
    if (box !== null) out.push(box);
  }
  for (const d of room.decor) {
    const box = decorFootprint(d);
    if (box !== null) out.push(box);
  }
  out.push(...scaffoldBoxes(room));
  return out;
}

/** True when a circle of the player's radius at (x, z) overlaps the box. */
function overlaps(x: number, z: number, b: Box) {
  const nx = Math.max(b.x0, Math.min(x, b.x1));
  const nz = Math.max(b.z0, Math.min(z, b.z1));
  const dx = x - nx;
  const dz = z - nz;
  return dx * dx + dz * dz < PLAYER_RADIUS * PLAYER_RADIUS;
}

/**
 * Every box the player's circle at (x, z) could touch: the void cells under
 * its bounding square (anything outside the grid counts as void) and the
 * blockers.
 */
function nearbySolids(
  x: number,
  z: number,
  grid: readonly string[],
  blockers: readonly Box[],
): Box[] {
  const out: Box[] = [];
  const cx0 = Math.floor((x - PLAYER_RADIUS) / CELL);
  const cx1 = Math.floor((x + PLAYER_RADIUS) / CELL);
  const cz0 = Math.floor((z - PLAYER_RADIUS) / CELL);
  const cz1 = Math.floor((z + PLAYER_RADIUS) / CELL);
  for (let cy = cz0; cy <= cz1; cy++) {
    for (let cx = cx0; cx <= cx1; cx++) {
      if (isFloor(grid, cx, cy)) continue;
      out.push({
        x0: cx * CELL,
        x1: (cx + 1) * CELL,
        z0: cy * CELL,
        z1: (cy + 1) * CELL,
      });
    }
  }
  for (const b of blockers) out.push(b);
  return out;
}

/**
 * True when the player's circle at (x, z) overlaps a void cell or a
 * blocker.
 */
function hits(
  x: number,
  z: number,
  grid: readonly string[],
  blockers: readonly Box[],
) {
  return nearbySolids(x, z, grid, blockers).some((b) => overlaps(x, z, b));
}

/** How many push-outs one tick may take before it gives up on them. */
const PUSH_ITERATIONS = 8;
/** A push-out lands this far past touching, so rounding never re-overlaps. */
const PUSH_SLACK = 1e-9;
/**
 * How far a corner push is turned when the player walks dead on at the
 * corner, in radians: enough to pick a side, too little to be seen.
 */
const CORNER_TIE_TURN = 0.05;

/**
 * Pushes the player's circle at (x, z) out of whatever it overlaps, the
 * deepest overlap first, each along the shortest way out: straight off a
 * face, or away from a corner along the line from the corner to the centre.
 * Pushing away from a corner is what makes the player slide round it, since
 * the push is never straight back along the walk unless the walk points
 * dead at the corner. That one case is a tie, and it is broken by turning
 * the push a little to the walk's left, always the same way, so the player
 * slides off instead of balancing on the point. `(mx, mz)` is the move this
 * tick. Returns the free position, or null when a few pushes did not find
 * one (a gap narrower than the player).
 */
function pushOut(
  x: number,
  z: number,
  mx: number,
  mz: number,
  grid: readonly string[],
  blockers: readonly Box[],
): [number, number] | null {
  const R = PLAYER_RADIUS;
  for (let i = 0; i < PUSH_ITERATIONS; i++) {
    let deepest: Box | null = null;
    let deepestD = Infinity;
    for (const b of nearbySolids(x, z, grid, blockers)) {
      if (!overlaps(x, z, b)) continue;
      const nx = Math.max(b.x0, Math.min(x, b.x1));
      const nz = Math.max(b.z0, Math.min(z, b.z1));
      const d = Math.hypot(x - nx, z - nz);
      if (d < deepestD) {
        deepestD = d;
        deepest = b;
      }
    }
    if (deepest === null) return [x, z];
    const b = deepest;
    const nx = Math.max(b.x0, Math.min(x, b.x1));
    const nz = Math.max(b.z0, Math.min(z, b.z1));
    if (deepestD === 0) {
      // The centre is inside the box: out through the nearest face.
      const exits = [
        { d: x - b.x0, x: b.x0 - R - PUSH_SLACK, z },
        { d: b.x1 - x, x: b.x1 + R + PUSH_SLACK, z },
        { d: z - b.z0, x, z: b.z0 - R - PUSH_SLACK },
        { d: b.z1 - z, x, z: b.z1 + R + PUSH_SLACK },
      ];
      exits.sort((a, c) => a.d - c.d);
      const exit = exits[0];
      if (exit === undefined) return null;
      x = exit.x;
      z = exit.z;
      continue;
    }
    let ux = (x - nx) / deepestD;
    let uz = (z - nz) / deepestD;
    const corner = (nx === b.x0 || nx === b.x1) && (nz === b.z0 || nz === b.z1);
    const len = Math.hypot(mx, mz);
    if (corner && len > 0) {
      // Dead on at the corner: the push points straight back along the walk.
      const cross = (ux * mz - uz * mx) / len;
      const dot = (ux * mx + uz * mz) / len;
      if (Math.abs(cross) < 1e-3 && dot < 0) {
        const c = Math.cos(CORNER_TIE_TURN);
        const s = Math.sin(CORNER_TIE_TURN);
        [ux, uz] = [ux * c - uz * s, ux * s + uz * c];
      }
    }
    x = nx + ux * (R + PUSH_SLACK);
    z = nz + uz * (R + PUSH_SLACK);
  }
  return hits(x, z, grid, blockers) ? null : [x, z];
}

/**
 * One tick of movement: turn and pitch from the intent, blend the velocity
 * towards the wished one, then move.
 *
 * The move is taken on both axes at once and the player's circle is then
 * pushed out of every void cell and blocker it overlaps (`pushOut`), so the
 * player slides along a wall walked into at an angle, slides round a corner
 * instead of snagging on it, and stops head on exactly at the wall's edge
 * plus the radius. The velocity becomes what the player really moved, so
 * the part of it that went into a wall is gone. Should the push-out find no
 * free place, milestone 1's rule takes over as a backstop: x then z, each
 * axis refused when it would overlap, which never leaves the player inside
 * anything. The grid's bounding rectangle is clamped to first.
 */
export function stepPlayer(
  p: Player,
  intent: Intent,
  room: RoomSpec,
  blockers: readonly Box[],
): Player {
  const yaw =
    p.yaw + intent.turn * TURN_SPEED * DT - intent.lookDx * MOUSE_SENSITIVITY;
  const pitch = Math.max(
    -MAX_PITCH,
    Math.min(MAX_PITCH, p.pitch - intent.lookDy * MOUSE_SENSITIVITY),
  );
  const [fx, fz] = forwardOf(yaw);
  const [rx, rz] = rightOf(yaw);
  let wx = fx * intent.forward + rx * intent.strafe;
  let wz = fz * intent.forward + rz * intent.strafe;
  const len = Math.hypot(wx, wz);
  if (len > 1) {
    wx /= len;
    wz /= len;
  }
  // Blend towards the wished velocity: most of the way in one tick.
  let vx = p.vx * FRICTION + wx * MAX_SPEED * (1 - FRICTION);
  let vz = p.vz * FRICTION + wz * MAX_SPEED * (1 - FRICTION);

  const minX = PLAYER_RADIUS;
  const maxX = room.width * CELL - PLAYER_RADIUS;
  const minZ = PLAYER_RADIUS;
  const maxZ = room.depth * CELL - PLAYER_RADIUS;
  const clampX = (v: number) => Math.max(minX, Math.min(maxX, v));
  const clampZ = (v: number) => Math.max(minZ, Math.min(maxZ, v));

  const tx = clampX(p.x + vx * DT);
  const tz = clampZ(p.z + vz * DT);
  let x = tx;
  let z = tz;
  if (hits(tx, tz, room.grid, blockers)) {
    const pushed = pushOut(tx, tz, tx - p.x, tz - p.z, room.grid, blockers);
    const free =
      pushed !== null &&
      !hits(clampX(pushed[0]), clampZ(pushed[1]), room.grid, blockers);
    if (pushed !== null && free) {
      x = clampX(pushed[0]);
      z = clampZ(pushed[1]);
    } else {
      // Backstop: milestone 1's axis by axis rule.
      x = hits(tx, p.z, room.grid, blockers) ? p.x : tx;
      z = hits(x, tz, room.grid, blockers) ? p.z : tz;
    }
    vx = (x - p.x) / DT;
    vz = (z - p.z) / DT;
  }

  const speed = Math.hypot(x - p.x, z - p.z) / DT;
  return {
    x,
    z,
    vx,
    vz,
    yaw,
    pitch,
    bob: p.bob + speed * DT * 2.2,
  };
}

/** The head bob for the current stride, in metres. */
export function headBob(p: Player): number {
  const speed = Math.min(1, Math.hypot(p.vx, p.vz) / MAX_SPEED);
  return Math.sin(p.bob) * 0.045 * speed;
}
