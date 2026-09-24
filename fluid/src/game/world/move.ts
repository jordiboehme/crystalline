/**
 * Walking: DOOM's feel, the station's walls.
 *
 * Movement is fast with little inertia - the player reaches full speed in a
 * few ticks and stops almost as quickly - and the head bobs a little with the
 * stride. It advances one 35 Hz tick at a time, so it feels the same at any
 * frame rate.
 *
 * Collision is against the room's rectangle and against one box per fixture.
 * The two axes are resolved one after the other, which is what lets the
 * player slide along a wall instead of stopping dead when walking into it at
 * an angle.
 */

import { TICK_HZ } from "../core/loop";
import { forwardOf, rightOf } from "../gl/math";
import { CELL } from "./generate";
import type { RoomSpec, WallSlot } from "./types";

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
/** How deep a fixture stands out from its wall, in metres. */
export const FIXTURE_DEPTH = 0.9;
/** How wide along the wall, in metres. */
export const FIXTURE_WIDTH = 2;

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

/** The floor footprint of a fixture standing against its wall. */
export function footprint(slot: WallSlot): Box {
  const cx = (slot.x + 0.5) * CELL;
  const cz = (slot.y + 0.5) * CELL;
  const half = FIXTURE_WIDTH / 2;
  switch (slot.side) {
    case "n":
      return {
        x0: cx - half,
        x1: cx + half,
        z0: slot.y * CELL,
        z1: slot.y * CELL + FIXTURE_DEPTH,
      };
    case "s":
      return {
        x0: cx - half,
        x1: cx + half,
        z0: (slot.y + 1) * CELL - FIXTURE_DEPTH,
        z1: (slot.y + 1) * CELL,
      };
    case "w":
      return {
        x0: slot.x * CELL,
        x1: slot.x * CELL + FIXTURE_DEPTH,
        z0: cz - half,
        z1: cz + half,
      };
    case "e":
      return {
        x0: (slot.x + 1) * CELL - FIXTURE_DEPTH,
        x1: (slot.x + 1) * CELL,
        z0: cz - half,
        z1: cz + half,
      };
  }
}

/**
 * The boxes the player collides with. Doors and portals are left out: they
 * are flat on their wall, and walking up to one is how it gets used in
 * milestone 2. The placard is flat too.
 */
export function blockersFor(room: RoomSpec): Box[] {
  return room.fixtures
    .filter((f) => f.kind === "terminal" || f.kind === "machine")
    .map((f) => footprint(f.slot));
}

function hits(x: number, z: number, blockers: readonly Box[]) {
  return blockers.some(
    (b) =>
      x > b.x0 - PLAYER_RADIUS &&
      x < b.x1 + PLAYER_RADIUS &&
      z > b.z0 - PLAYER_RADIUS &&
      z < b.z1 + PLAYER_RADIUS,
  );
}

/** One tick of movement. */
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
  const vx = p.vx * FRICTION + wx * MAX_SPEED * (1 - FRICTION);
  const vz = p.vz * FRICTION + wz * MAX_SPEED * (1 - FRICTION);

  const minX = PLAYER_RADIUS;
  const maxX = room.width * CELL - PLAYER_RADIUS;
  const minZ = PLAYER_RADIUS;
  const maxZ = room.depth * CELL - PLAYER_RADIUS;

  let x = Math.max(minX, Math.min(maxX, p.x + vx * DT));
  if (hits(x, p.z, blockers)) x = p.x;
  let z = Math.max(minZ, Math.min(maxZ, p.z + vz * DT));
  if (hits(x, z, blockers)) z = p.z;

  const speed = Math.hypot(x - p.x, z - p.z) / DT;
  return {
    x,
    z,
    vx: x === p.x ? 0 : vx,
    vz: z === p.z ? 0 : vz,
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
