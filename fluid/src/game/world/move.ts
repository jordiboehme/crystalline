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
 * machine sized by its kind (`FOOTPRINTS` in `footprints.ts`), each piece
 * of the archetype's furniture turned with it, the scaffold frames of a room
 * under construction, every floor prop of the set dressing and every hero
 * prop but the flush wall-mounted ones (`heroBlocker`), and in the console
 * room its free-standing console (`interiorFootprint`). The grid's
 * bounding rectangle is still clamped to as a backstop. The two axes are
 * resolved one after the other, which is what lets the player slide along a
 * wall instead of stopping dead when walking into it at an angle.
 */

import { TICK_HZ } from "../core/loop";
import { forwardOf, rightOf } from "../gl/math";
import {
  decorFootprint,
  footprintOf,
  heroBlocker,
  propFootprint,
} from "./footprints";
import { interiorFootprint } from "./consoleRoom";
import { isFloor } from "./layout";
import type { Box, RoomSpec } from "./types";
import { CELL } from "./units";

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
  /** -1 right, 1 left (the turn keys). */
  turn: number;
  /** Mouse pixels since the last tick. */
  lookDx: number;
  lookDy: number;
  /** Whether the run key is held: `RUN_FACTOR` times the walk speed. */
  run: boolean;
}

/** The player's collision radius, in metres. */
export const PLAYER_RADIUS = 0.35;
/** Eye height above the floor, in metres. */
export const EYE_HEIGHT = 1.6;
/** How far the camera can pitch up or down, in radians (+-30 degrees). */
export const MAX_PITCH = Math.PI / 6;

const DT = 1 / TICK_HZ;
const MAX_SPEED = 7;
/**
 * How much faster running is than walking: twice, as in the classic
 * layout, so 14 m/s, 0.4 m a tick. Turning keeps its speed.
 */
export const RUN_FACTOR = 2;
/** Fraction of velocity kept per tick with no input: quick stops. */
const FRICTION = 0.55;
const TURN_SPEED = 3;
const MOUSE_SENSITIVITY = 0.0025;

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
 * The boxes the player collides with besides the grid: the terminals and
 * machines (`footprintOf`), the furniture (`decorFootprint`), the scaffold
 * frames the generator put up (`room.scaffold`), the floor props of the
 * set dressing (`propFootprint`), the heroes (`heroBlocker`) and the
 * console room's fittings (`interiorFootprint`: the console's desk). Flush
 * fixtures, pipe runs, the wall and ceiling props, the flush wall heroes
 * and the flush fittings are left out: they hang on a wall or overhead.
 * Built once per room, not per tick.
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
  out.push(...room.scaffold);
  for (const p of room.props) {
    const box = propFootprint(p);
    if (box !== null) out.push(box);
  }
  for (const h of room.heroes) {
    const box = heroBlocker(h);
    if (box !== null) out.push(box);
  }
  for (const p of room.interior ?? []) {
    const box = interiorFootprint(p);
    if (box !== null) out.push(box);
  }
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
 * One step of the move: the player's circle at (px, pz) moved by (dx, dz),
 * clamped to the grid's rectangle and pushed out of whatever it then
 * overlaps (see `stepPlayer`), and whether anything stopped it.
 */
function moveCircle(
  px: number,
  pz: number,
  dx: number,
  dz: number,
  room: RoomSpec,
  blockers: readonly Box[],
): { x: number; z: number; collided: boolean } {
  const minX = PLAYER_RADIUS;
  const maxX = room.width * CELL - PLAYER_RADIUS;
  const minZ = PLAYER_RADIUS;
  const maxZ = room.depth * CELL - PLAYER_RADIUS;
  const clampX = (v: number) => Math.max(minX, Math.min(maxX, v));
  const clampZ = (v: number) => Math.max(minZ, Math.min(maxZ, v));

  const rawX = px + dx;
  const rawZ = pz + dz;
  const tx = clampX(rawX);
  const tz = clampZ(rawZ);
  let x = tx;
  let z = tz;
  // A wall on the grid's edge stops the player through the clamp, not
  // through `hits`, and must stop the stride just the same.
  let collided = tx !== rawX || tz !== rawZ;
  if (hits(tx, tz, room.grid, blockers)) {
    collided = true;
    const pushed = pushOut(tx, tz, tx - px, tz - pz, room.grid, blockers);
    const free =
      pushed !== null &&
      !hits(clampX(pushed[0]), clampZ(pushed[1]), room.grid, blockers);
    if (pushed !== null && free) {
      x = clampX(pushed[0]);
      z = clampZ(pushed[1]);
    } else {
      // Backstop: milestone 1's axis by axis rule.
      x = hits(tx, pz, room.grid, blockers) ? px : tx;
      z = hits(x, tz, room.grid, blockers) ? pz : tz;
    }
  }
  return { x, z, collided };
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
 * anything. The grid's bounding rectangle is clamped to first, and a
 * clamp counts as a collision too: a wall on the grid's edge (every hall's
 * north wall, and its west wall when there is no corridor) takes the
 * velocity into it just as a wall of void cells does. A move longer than
 * `PLAYER_RADIUS` (a run) is taken in equal steps no longer than it, each
 * resolved that way (`moveCircle`).
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
  const top = intent.run ? MAX_SPEED * RUN_FACTOR : MAX_SPEED;
  let vx = p.vx * FRICTION + wx * top * (1 - FRICTION);
  let vz = p.vz * FRICTION + wz * top * (1 - FRICTION);

  // A move longer than the player's radius is taken in equal steps no
  // longer than it (running is two), so the push-out never finds the
  // centre past the middle of a thin blocker and pushes it out the far
  // side. A walk is always one step.
  const steps = Math.max(
    1,
    Math.ceil((Math.hypot(vx, vz) * DT) / PLAYER_RADIUS),
  );
  let x = p.x;
  let z = p.z;
  let collided = false;
  for (let i = 0; i < steps; i++) {
    const moved = moveCircle(
      x,
      z,
      (vx * DT) / steps,
      (vz * DT) / steps,
      room,
      blockers,
    );
    x = moved.x;
    z = moved.z;
    if (moved.collided) collided = true;
  }
  if (collided) {
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

/**
 * The vertical look handed to `stepPlayer`, after the player's choice of
 * inverted look (key I): the mouse's `dy` as it came, or negated when
 * inverted, so moving the mouse forward looks down instead of up. Kept apart from `stepPlayer` so the choice lives in one place and
 * the session only passes a flag.
 */
export function lookDelta(dy: number, inverted: boolean): number {
  return inverted ? -dy : dy;
}
