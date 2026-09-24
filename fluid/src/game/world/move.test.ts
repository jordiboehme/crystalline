import { describe, expect, it } from "vitest";

import { CANNED_BRIDGE } from "./canned";
import { CELL, generateRoom } from "./generate";
import {
  MAX_PITCH,
  PLAYER_RADIUS,
  blockersFor,
  footprint,
  spawnPlayer,
  stepPlayer,
  type Intent,
  type Player,
} from "./move";

const room = generateRoom(CANNED_BRIDGE);
const blockers = blockersFor(room);
const idle: Intent = { forward: 0, strafe: 0, turn: 0, lookDx: 0, lookDy: 0 };

function run(p: Player, intent: Intent, ticks: number) {
  let q = p;
  for (let i = 0; i < ticks; i++) q = stepPlayer(q, intent, room, blockers);
  return q;
}

/** Steps until `stop` is true or 2000 ticks pass, whichever comes first. */
function walkUntil(p: Player, intent: Intent, stop: (q: Player) => boolean) {
  let q = p;
  for (let i = 0; i < 2000 && !stop(q); i++) {
    q = stepPlayer(q, intent, room, blockers);
  }
  return q;
}

describe("spawnPlayer", () => {
  it("stands in the middle of the entrance cell, facing north", () => {
    const p = spawnPlayer(room);
    expect(p.x).toBeCloseTo((room.spawn.x + 0.5) * CELL);
    expect(p.z).toBeCloseTo((room.spawn.y + 0.5) * CELL);
    expect(p.yaw).toBe(0);
  });
});

describe("stepPlayer", () => {
  it("walks north when forward is held", () => {
    const p = spawnPlayer(room);
    const q = run(p, { ...idle, forward: 1 }, 10);
    expect(q.z).toBeLessThan(p.z);
    expect(q.x).toBeCloseTo(p.x, 3);
  });

  it("gets up to speed quickly and stops quickly (little inertia)", () => {
    const p = spawnPlayer(room);
    const moving = run(p, { ...idle, forward: 1 }, 6);
    expect(Math.hypot(moving.vx, moving.vz)).toBeGreaterThan(5);
    const stopped = run(moving, idle, 6);
    expect(Math.hypot(stopped.vx, stopped.vz)).toBeLessThan(0.5);
  });

  it("never leaves the room however long it walks", () => {
    let p = spawnPlayer(room);
    for (const dir of [
      { forward: 1 },
      { strafe: 1 },
      { forward: -1 },
      { strafe: -1 },
    ]) {
      p = run(p, { ...idle, ...dir }, 400);
      expect(p.x).toBeGreaterThanOrEqual(PLAYER_RADIUS - 1e-6);
      expect(p.x).toBeLessThanOrEqual(room.width * CELL - PLAYER_RADIUS + 1e-6);
      expect(p.z).toBeGreaterThanOrEqual(PLAYER_RADIUS - 1e-6);
      expect(p.z).toBeLessThanOrEqual(room.depth * CELL - PLAYER_RADIUS + 1e-6);
    }
  });

  it("does not walk through a fixture", () => {
    // A straight walk up the spawn column never meets a fixture (every
    // terminal and machine sits off to the side), so this drives the player
    // sideways into one of each kind instead: north to a fixture's depth,
    // then into its wall. The target box comes straight from the room's own
    // fixtures via `footprint`, not from `blockers`, so this stays a live
    // check of `stepPlayer`'s collision even if `blockersFor` broke.
    const terminalFixture = room.fixtures.find((f) => f.kind === "terminal");
    const machineFixture = room.fixtures.find((f) => f.kind === "machine");
    if (!terminalFixture || !machineFixture) {
      throw new Error(
        "the canned room has no terminal or machine to walk into",
      );
    }

    const terminal = footprint(terminalFixture.slot);
    const terminalZ = (terminal.z0 + terminal.z1) / 2;
    let west = walkUntil(
      spawnPlayer(room),
      { ...idle, forward: 1 },
      (p) => p.z <= terminalZ,
    );
    west = run(west, { ...idle, strafe: -1 }, 400);
    expect(west.x).toBeGreaterThanOrEqual(terminal.x1 + PLAYER_RADIUS - 1e-6);

    const machine = footprint(machineFixture.slot);
    const machineZ = (machine.z0 + machine.z1) / 2;
    let east = walkUntil(
      spawnPlayer(room),
      { ...idle, forward: 1 },
      (p) => p.z <= machineZ,
    );
    east = run(east, { ...idle, strafe: 1 }, 400);
    expect(east.x).toBeLessThanOrEqual(machine.x0 - PLAYER_RADIUS + 1e-6);
  });

  it("slides along a wall instead of sticking to it", () => {
    let p = spawnPlayer(room);
    p = run(p, { ...idle, strafe: -1 }, 400);
    const before = p.z;
    p = run(p, { ...idle, strafe: -1, forward: 1 }, 10);
    expect(p.z).toBeLessThan(before);
  });

  it("clamps pitch to 30 degrees either way", () => {
    const p = spawnPlayer(room);
    expect(run(p, { ...idle, lookDy: -10000 }, 1).pitch).toBeCloseTo(MAX_PITCH);
    expect(run(p, { ...idle, lookDy: 10000 }, 1).pitch).toBeCloseTo(-MAX_PITCH);
  });

  it("turns left with the left arrow and right with the mouse moving right", () => {
    const p = spawnPlayer(room);
    expect(run(p, { ...idle, turn: 1 }, 5).yaw).toBeGreaterThan(0);
    expect(run(p, { ...idle, lookDx: 50 }, 1).yaw).toBeLessThan(0);
  });

  it("does not move faster on a diagonal", () => {
    const p = spawnPlayer(room);
    const straight = run(p, { ...idle, forward: 1 }, 3);
    const diagonal = run(p, { ...idle, forward: 1, strafe: 1 }, 3);
    expect(Math.hypot(diagonal.vx, diagonal.vz)).toBeCloseTo(
      Math.hypot(straight.vx, straight.vz),
      3,
    );
  });
});
