import { describe, expect, it } from "vitest";

import { createRng } from "../core/seed";
import { CANNED_BRIDGE, CANNED_HUB } from "./canned";
import {
  FIXTURE_DEPTH,
  FIXTURE_WIDTH,
  FOOTPRINTS,
  decorFootprint,
  footprintOf,
  propFootprint,
} from "./footprints";
import { generateRoom } from "./generate";
import { BAY, isFloor } from "./layout";
import {
  MAX_PITCH,
  PLAYER_RADIUS,
  blockersFor,
  headBob,
  lookDelta,
  spawnPlayer,
  stepPlayer,
  type Intent,
  type Player,
} from "./move";
import type { Box, Decor, Prop, RoomSpec } from "./types";
import { CELL } from "./units";

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
    // fixtures via `footprintOf`, not from `blockers`, so this stays a live
    // check of `stepPlayer`'s collision even if `blockersFor` broke.
    const terminalFixture = room.fixtures.find((f) => f.kind === "terminal");
    const machineFixture = room.fixtures.find((f) => f.kind === "machine");
    if (!terminalFixture || !machineFixture) {
      throw new Error(
        "the canned room has no terminal or machine to walk into",
      );
    }

    const terminal = footprintOf(terminalFixture);
    if (terminal === null) throw new Error("a terminal has a footprint");
    const terminalZ = (terminal.z0 + terminal.z1) / 2;
    let west = walkUntil(
      spawnPlayer(room),
      { ...idle, forward: 1 },
      (p) => p.z <= terminalZ,
    );
    west = run(west, { ...idle, strafe: -1 }, 400);
    expect(west.x).toBeGreaterThanOrEqual(terminal.x1 + PLAYER_RADIUS - 1e-6);

    const machine = footprintOf(machineFixture);
    if (machine === null) throw new Error("a machine has a footprint");
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

/** One tick's travel at full speed, in metres: how short of contact a stop can land. */
const STEP = 7 / 35 + 1e-6;

/** A player standing still at a point, facing north. */
function at(x: number, z: number): Player {
  return { x, z, vx: 0, vz: 0, yaw: 0, pitch: 0, bob: 0 };
}

function runIn(r: RoomSpec, p: Player, intent: Intent, ticks: number) {
  const bs = blockersFor(r);
  let q = p;
  for (let i = 0; i < ticks; i++) q = stepPlayer(q, intent, r, bs);
  return q;
}

/** True when the player's circle overlaps a void cell or the outside. */
function touchesVoid(r: RoomSpec, p: Player) {
  const R = PLAYER_RADIUS - 1e-6;
  for (
    let y = Math.floor((p.z - R) / CELL);
    y <= Math.floor((p.z + R) / CELL);
    y++
  ) {
    for (
      let x = Math.floor((p.x - R) / CELL);
      x <= Math.floor((p.x + R) / CELL);
      x++
    ) {
      if (isFloor(r.grid, x, y)) continue;
      const nx = Math.max(x * CELL, Math.min(p.x, (x + 1) * CELL));
      const nz = Math.max(y * CELL, Math.min(p.z, (y + 1) * CELL));
      if (Math.hypot(p.x - nx, p.z - nz) < R) return true;
    }
  }
  return false;
}

/** True when the player's circle overlaps one of the blockers. */
function touchesBlocker(bs: readonly Box[], p: Player) {
  const R = PLAYER_RADIUS - 1e-6;
  return bs.some((b) => {
    const nx = Math.max(b.x0, Math.min(p.x, b.x1));
    const nz = Math.max(b.z0, Math.min(p.z, b.z1));
    return Math.hypot(p.x - nx, p.z - nz) < R;
  });
}

const hub = generateRoom(CANNED_HUB);

/** A world direction on the diagonal: -1 or 1 along x and along z. */
type Diagonal = readonly [-1 | 1, -1 | 1];

/**
 * Holds the diagonal intent that walks the unturned player along `dir` (x
 * is strafe, z is back) for `ticks`, starting 0.45 m short of `corner` on
 * that diagonal and `offset` metres to one side of it. Fails on any tick
 * that ends overlapping void or a blocker; returns where the player ended.
 */
function diagonalInto(
  r: RoomSpec,
  corner: { x: number; z: number },
  dir: Diagonal,
  offset: number,
  ticks = 60,
) {
  const bs = blockersFor(r);
  const [dx, dz] = dir;
  const k = 0.45 / Math.SQRT2;
  const side = offset / Math.SQRT2;
  let p = at(corner.x - dx * k - dz * side, corner.z - dz * k + dx * side);
  expect(touchesVoid(r, p)).toBe(false);
  expect(touchesBlocker(bs, p)).toBe(false);
  const intent: Intent = { ...idle, strafe: dx, forward: -dz };
  for (let i = 0; i < ticks; i++) {
    p = stepPlayer(p, intent, r, bs);
    expect(touchesVoid(r, p)).toBe(false);
    expect(touchesBlocker(bs, p)).toBe(false);
  }
  return p;
}

const distance = (p: Player, c: { x: number; z: number }) =>
  Math.hypot(p.x - c.x, p.z - c.z);

describe("walking on the grid", () => {
  it("enters the backlink corridor through its doorway, not beside it", () => {
    const hall = hub.hall;
    const doorway = hall.x0 - 1;
    const rows = hub.grid
      .map((_, y) => y)
      .filter((y) => isFloor(hub.grid, doorway, y));
    expect(rows.length).toBe(2);
    const [first] = rows as [number, number];

    // Down the middle of the two corridor rows: through the doorway and on.
    const through = runIn(
      hub,
      at((hall.x0 + 1) * CELL, (first + 1) * CELL),
      { ...idle, strafe: -1 },
      400,
    );
    expect(through.x).toBeLessThan(doorway * CELL);

    // One row north of the doorway: the void column stops the player.
    const beside = runIn(
      hub,
      at((hall.x0 + 1) * CELL, (first - 0.5) * CELL),
      { ...idle, strafe: -1 },
      400,
    );
    expect(beside.x).toBeGreaterThanOrEqual(
      hall.x0 * CELL + PLAYER_RADIUS - 1e-6,
    );
    expect(beside.x).toBeLessThanOrEqual(hall.x0 * CELL + PLAYER_RADIUS + STEP);
  });

  it("walks into a bay through its doorway and stops at the wall beside it", () => {
    const hall = hub.hall;
    // The first bay's doorway is the hall's east neighbour column, rows 3 and 4.
    expect(isFloor(hub.grid, hall.x1, 3)).toBe(true);
    expect(isFloor(hub.grid, hall.x1, 4)).toBe(true);
    expect(isFloor(hub.grid, hall.x1, 2)).toBe(false);

    const through = runIn(
      hub,
      at((hall.x1 - 1) * CELL, 4 * CELL),
      { ...idle, strafe: 1 },
      400,
    );
    expect(through.x).toBeGreaterThan((hall.x1 + 1) * CELL);

    const beside = runIn(
      hub,
      at((hall.x1 - 1) * CELL, 2.5 * CELL),
      { ...idle, strafe: 1 },
      400,
    );
    expect(beside.x).toBeLessThanOrEqual(hall.x1 * CELL - PLAYER_RADIUS + 1e-6);
    expect(beside.x).toBeGreaterThanOrEqual(
      hall.x1 * CELL - PLAYER_RADIUS - STEP,
    );
  });

  it("stops at a terminal's own footprint, not the old fixed box", () => {
    const terminal = room.fixtures.find((f) => f.kind === "terminal");
    if (terminal === undefined) throw new Error("the bridge has terminals");
    expect(terminal.slot.side).toBe("w");
    const wall = terminal.slot.x * CELL;
    const cz = (terminal.slot.y + 0.5) * CELL;

    // Head on: the desk's 0.9 m depth plus the radius.
    const head = runIn(room, at(wall + 3, cz), { ...idle, strafe: -1 }, 400);
    expect(head.x).toBeGreaterThanOrEqual(wall + 0.9 + PLAYER_RADIUS - 1e-6);
    expect(head.x).toBeLessThanOrEqual(wall + 0.9 + PLAYER_RADIUS + STEP);

    // A lane 1.2 m off its centre clears the 1.4 m desk and reaches the wall,
    // where the old 2 m box would have stopped the player 0.9 m short.
    const lane = runIn(
      room,
      at(wall + 3, cz + 1.2),
      { ...idle, strafe: -1 },
      400,
    );
    expect(lane.x).toBeLessThanOrEqual(wall + PLAYER_RADIUS + STEP);
  });

  it("sizes machines per kind", () => {
    const slot = { x: 4, y: 3, side: "e" as const };
    const box = (machine: "server-rack" | "workbench") =>
      footprintOf({
        kind: "machine",
        slot,
        machine,
        tag: "t",
        hue: 0,
        seed: 1,
      }) as Box;
    const rack = box("server-rack");
    const bench = box("workbench");
    expect(rack.z1 - rack.z0).toBeCloseTo(0.8);
    expect(rack.x1 - rack.x0).toBeCloseTo(1.0);
    expect(rack.z1 - rack.z0).toBeLessThan(FIXTURE_WIDTH);
    expect(rack.z1 - rack.z0).toBeLessThan(bench.z1 - bench.z0);
    expect(bench.z1 - bench.z0).toBeCloseTo(1.8);
    expect(bench.x1 - bench.x0).toBeCloseTo(0.9);
    // Against the east wall: flush with the cell's east edge.
    expect(rack.x1).toBeCloseTo(5 * CELL);
    expect(FOOTPRINTS.machine["comms-array"]).toEqual({ along: 1.2, out: 0.6 });
    expect(FIXTURE_DEPTH).toBe(0.9);
  });

  it("leaves flush fixtures out of the blockers", () => {
    for (const f of hub.fixtures) {
      const flush =
        f.kind === "door" ||
        f.kind === "portal" ||
        f.kind === "hatch" ||
        f.kind === "poster" ||
        f.kind === "placard";
      expect(footprintOf(f) === null).toBe(flush);
    }
  });

  it("turns decor footprints with the piece", () => {
    const d = { kind: "lab-island" as const, x: 5, y: 6, seed: 1 };
    const north = decorFootprint({ ...d, turn: 0 }) as Box;
    const east = decorFootprint({ ...d, turn: 1 }) as Box;
    expect(north).toEqual({ x0: 8.5, x1: 11.5, z0: 11.3, z1: 12.7 });
    expect(east).toEqual({ x0: 9.3, x1: 10.7, z0: 10.5, z1: 13.5 });
    expect(FOOTPRINTS.decor["shelf-row"]).toEqual({ width: 4, depth: 0.8 });
  });

  it("is blocked by a crate and not by a wall prop or a ceiling prop", () => {
    // The bridge with nothing in it but three props on the middle column:
    // a crate in the hall, a vent on the north wall, a beacon above it.
    const crate: Prop = {
      kind: "crate",
      variant: 1,
      anchor: "floor",
      x: 3.5,
      y: 2.5,
      turn: 0,
      seed: 1,
    };
    const vent: Prop = {
      kind: "vent-grille",
      variant: 0,
      anchor: "wall",
      x: 3.5,
      y: 0,
      turn: 2,
      seed: 2,
    };
    const beacon: Prop = {
      ...vent,
      kind: "beacon",
      anchor: "ceiling",
      seed: 3,
    };
    const bare: RoomSpec = { ...room, fixtures: [], decor: [], props: [] };
    const dressed: RoomSpec = { ...bare, props: [crate, vent, beacon] };
    const box = propFootprint(crate);
    if (box === null) throw new Error("a crate stands on the floor");
    expect(blockersFor(dressed)).toEqual([box]);
    expect(propFootprint(vent)).toBeNull();
    expect(propFootprint(beacon)).toBeNull();
    // Walking north from the spawn stops at the crate...
    const x = 3.5 * CELL;
    const stopped = runIn(
      dressed,
      at(x, 5.5 * CELL),
      { ...idle, forward: 1 },
      200,
    );
    expect(stopped.z).toBeGreaterThanOrEqual(box.z1 + PLAYER_RADIUS - 1e-6);
    expect(stopped.z).toBeLessThanOrEqual(box.z1 + PLAYER_RADIUS + STEP);
    // ...and from north of it the player reaches the wall under the vent.
    const wall = runIn(
      dressed,
      at(x, 1.5 * CELL),
      { ...idle, forward: 1 },
      200,
    );
    expect(wall.z).toBeCloseTo(PLAYER_RADIUS, 3);
    expect(
      runIn(bare, at(x, 5.5 * CELL), { ...idle, forward: 1 }, 200).z,
    ).toBeCloseTo(PLAYER_RADIUS, 3);
  });

  it("is blocked by a council chair and walks under a pipe run", () => {
    const council = generateRoom({ ...CANNED_HUB, type: "decision" });
    const chair = council.decor.find((d) => d.kind === "council-chair");
    if (chair === undefined) throw new Error("a council has chairs");
    const box = decorFootprint(chair);
    if (box === null) throw new Error("a chair blocks");
    expect(blockersFor(council)).toContainEqual(box);
    // The first chair sits north of the table: walk south into it.
    const cx = (box.x0 + box.x1) / 2;
    const stopped = runIn(
      council,
      at(cx, box.z0 - 3),
      { ...idle, forward: -1 },
      400,
    );
    expect(stopped.z).toBeLessThanOrEqual(box.z0 - PLAYER_RADIUS + 1e-6);
    expect(stopped.z).toBeGreaterThanOrEqual(box.z0 - PLAYER_RADIUS - STEP);

    const engineering = generateRoom({ ...CANNED_HUB, type: "runbook" });
    const pipe = engineering.decor.find((d) => d.kind === "pipe-run");
    if (pipe === undefined) throw new Error("engineering has pipe runs");
    expect(pipe.turn).toBe(0);
    expect(decorFootprint(pipe)).toBeNull();
    // Beside the generator, south across the pipe run's line.
    const px = pipe.x * CELL + 3;
    const pz = pipe.y * CELL;
    const under = runIn(
      engineering,
      at(px, pz - 2),
      { ...idle, forward: -1 },
      30,
    );
    expect(under.z).toBeGreaterThan(pz + 1);
  });

  it("puts up to two scaffold frames in the hall's interior band of a room under construction", () => {
    const built = generateRoom({ ...CANNED_HUB, status: "draft" });
    expect(built.condition).toBe("construction");
    const frames = built.scaffold;
    // Its shelf rows may take a frame (see the next test), never both.
    expect(frames.length).toBeGreaterThanOrEqual(1);
    expect(frames.length).toBeLessThanOrEqual(2);
    const h = built.hall;
    for (const f of frames) {
      expect(f.x1 - f.x0).toBeCloseTo(1.4);
      expect(f.z1 - f.z0).toBeCloseTo(1.4);
      expect(f.x0).toBeGreaterThanOrEqual((h.x0 + 2) * CELL);
      expect(f.x1).toBeLessThanOrEqual((h.x1 - 2) * CELL);
      expect(f.z0).toBeGreaterThanOrEqual((h.y0 + 2) * CELL);
      expect(f.z1).toBeLessThanOrEqual((h.y1 - 2) * CELL);
      expect(blockersFor(built)).toContainEqual(f);
    }
    // Generated once, the same every time the place is entered.
    expect(generateRoom({ ...CANNED_HUB, status: "draft" }).scaffold).toEqual(
      frames,
    );
    expect(hub.scaffold).toEqual([]);
    // The smallest hall has no decor, so its first frame always stands; its
    // band is so small that the second one mostly overlaps the first.
    const small = generateRoom({ ...CANNED_BRIDGE, status: "draft" });
    expect(small.scaffold.length).toBeGreaterThanOrEqual(1);
  });

  it("skips a scaffold frame that would stand in the decor or the other frame", () => {
    const share = (a: Box, b: Box) =>
      a.x0 < b.x1 && b.x0 < a.x1 && a.z0 < b.z1 && b.z0 < a.z1;
    let skipped = 0;
    for (const type of [
      "manifest",
      "decision",
      "runbook",
      "reference",
      "guide",
    ]) {
      for (let i = 0; i < 12; i++) {
        const r = generateRoom({
          ...CANNED_HUB,
          type,
          status: "draft",
          permalink: `site-${i}`,
        });
        const frames = r.scaffold;
        const decor = r.decor
          .map(decorFootprint)
          .filter((b): b is Box => b !== null);
        for (const f of frames) {
          for (const d of decor) expect(share(f, d)).toBe(false);
        }
        const [a, b] = frames;
        if (a !== undefined && b !== undefined) expect(share(a, b)).toBe(false);
        skipped += 2 - frames.length;
      }
    }
    // Sixty building sites among the shelves and tables: some frame had to go.
    expect(skipped).toBeGreaterThan(0);
  });

  it("never ends inside a void cell, whatever it is asked", () => {
    for (const place of [
      CANNED_HUB,
      { ...CANNED_HUB, status: "draft" },
      { ...CANNED_BRIDGE, status: "draft" },
    ]) {
      const r = generateRoom(place);
      const bs = blockersFor(r);
      const rng = createRng(2026);
      let p = spawnPlayer(r);
      expect(touchesVoid(r, p)).toBe(false);
      for (let i = 0; i < 2000; i++) {
        const intent: Intent = {
          forward: rng.int(-1, 1),
          strafe: rng.int(-1, 1),
          turn: rng.int(-1, 1),
          lookDx: rng.range(-40, 40),
          lookDy: 0,
        };
        const ticks = rng.int(1, 12);
        for (let t = 0; t < ticks; t++) {
          p = stepPlayer(p, intent, r, bs);
          expect(touchesVoid(r, p)).toBe(false);
          expect(touchesBlocker(bs, p)).toBe(false);
          expect(
            isFloor(r.grid, Math.floor(p.x / CELL), Math.floor(p.z / CELL)),
          ).toBe(true);
        }
      }
    }
  });
});

describe("sliding around corners", () => {
  // Each case walks diagonally at a convex corner from the one quadrant
  // that is open floor, both a little to either side of the corner and dead
  // on it, and must get past it instead of freezing there.
  const offsets = [-0.06, 0, 0.06];

  it("slides past each jamb of a bay doorway", () => {
    const X = hub.hall.x1 * CELL;
    const jambs: [{ x: number; z: number }, Diagonal][] = [
      [{ x: X, z: 3 * CELL }, [1, -1]],
      [{ x: X, z: 5 * CELL }, [1, 1]],
      [{ x: X + CELL, z: 3 * CELL }, [-1, -1]],
      [{ x: X + CELL, z: 5 * CELL }, [-1, 1]],
    ];
    for (const [corner, dir] of jambs) {
      for (const offset of offsets) {
        const p = diagonalInto(hub, corner, dir, offset);
        expect(distance(p, corner)).toBeGreaterThan(1);
      }
    }
  });

  it("slides past both room-side corners of a terminal", () => {
    // Undressed: a wall-side floor prop may stand between the bridge's
    // terminals since the wall-side chance went up (E7).
    const bare: RoomSpec = { ...room, props: [] };
    const terminal = bare.fixtures.find((f) => f.kind === "terminal");
    if (terminal === undefined) throw new Error("the bridge has terminals");
    const box = footprintOf(terminal) as Box;
    const corners: [{ x: number; z: number }, Diagonal][] = [
      [{ x: box.x1, z: box.z0 }, [-1, 1]],
      [{ x: box.x1, z: box.z1 }, [-1, -1]],
    ];
    for (const [corner, dir] of corners) {
      for (const offset of offsets) {
        const p = diagonalInto(bare, corner, dir, offset, 40);
        // Past the corner, or slid along the desk into the pocket between
        // it and the wall; frozen at the corner is neither.
        const inPocket =
          Math.abs(p.x - (terminal.slot.x * CELL + PLAYER_RADIUS)) < 1e-6;
        expect(inPocket || distance(p, corner) > 1).toBe(true);
      }
    }
  });

  it("slides past every corner of a council chair", () => {
    const council = generateRoom({ ...CANNED_HUB, type: "decision" });
    const chairs = council.decor.filter((d) => d.kind === "council-chair");
    // The chairs due north and due south of the table.
    const north = decorFootprint(chairs[0] as Decor) as Box;
    const south = decorFootprint(chairs[3] as Decor) as Box;
    const corners: [{ x: number; z: number }, Diagonal][] = [
      [{ x: north.x0, z: north.z0 }, [1, 1]],
      [{ x: north.x1, z: north.z0 }, [-1, 1]],
      [{ x: south.x0, z: south.z1 }, [1, -1]],
      [{ x: south.x1, z: south.z1 }, [-1, -1]],
    ];
    for (const [corner, dir] of corners) {
      for (const offset of offsets) {
        const p = diagonalInto(council, corner, dir, offset, 40);
        expect(distance(p, corner)).toBeGreaterThan(1);
      }
    }
  });

  it("walks diagonally into a corner of void cells and settles in it", () => {
    // The first bay's south-east corner: void on both sides (the next bay's
    // doorway column east, the rows below the bay south), no fixture near.
    const corner = { x: (hub.hall.x1 + 1 + BAY) * CELL, z: BAY * CELL };
    expect(isFloor(hub.grid, hub.hall.x1 + BAY, BAY - 1)).toBe(true);
    expect(isFloor(hub.grid, hub.hall.x1 + BAY + 1, BAY - 1)).toBe(false);
    expect(isFloor(hub.grid, hub.hall.x1 + BAY, BAY)).toBe(false);
    const p = diagonalInto(
      hub,
      { x: corner.x - 3, z: corner.z - 3 },
      [1, 1],
      0.3,
      80,
    );
    expect(p.x).toBeCloseTo(corner.x - PLAYER_RADIUS, 3);
    expect(p.z).toBeCloseTo(corner.z - PLAYER_RADIUS, 3);
    expect(p.vx).toBeCloseTo(0, 6);
    expect(p.vz).toBeCloseTo(0, 6);
    expect(headBob(p)).toBeCloseTo(0, 6);
  });

  it("stops the stride at a wall on the grid's edge", () => {
    // The bridge's west wall is the grid's west edge: between its terminals.
    // Undressed: a wall-side floor prop may stand there since the wall-side
    // chance went up (E7).
    const bridge: RoomSpec = { ...room, props: [] };
    expect(bridge.hall.x0).toBe(0);
    const west = runIn(bridge, at(3, 7), { ...idle, strafe: -1 }, 60);
    expect(west.x).toBeCloseTo(PLAYER_RADIUS, 6);
    expect(west.vx).toBeCloseTo(0, 9);
    expect(west.vz).toBeCloseTo(0, 9);
    expect(headBob(west)).toBeCloseTo(0, 9);

    // The hub's north wall is the grid's north edge.
    const cx = ((hub.hall.x0 + hub.hall.x1) / 2) * CELL;
    const north = runIn(hub, at(cx, 3), { ...idle, forward: 1 }, 60);
    expect(north.z).toBeCloseTo(PLAYER_RADIUS, 6);
    expect(north.vx).toBeCloseTo(0, 9);
    expect(north.vz).toBeCloseTo(0, 9);
    expect(headBob(north)).toBeCloseTo(0, 9);

    // Sliding along it diagonally keeps only the speed along the wall.
    const slide = runIn(hub, north, { ...idle, forward: 1, strafe: 1 }, 20);
    expect(slide.z).toBeCloseTo(PLAYER_RADIUS, 6);
    expect(slide.vz).toBeCloseTo(0, 9);
    expect(slide.vx).toBeCloseTo(7 / Math.SQRT2, 2);
    expect(slide.x - north.x).toBeGreaterThan(2);
  });

  it("stops head on at a wall's edge plus the radius", () => {
    // Undressed: a floor prop may stand against this wall since GAME_VERSION 3.
    const bare: RoomSpec = { ...hub, props: [] };
    const wall = hub.hall.x0 * CELL;
    const p = runIn(bare, at(wall + 3, 3), { ...idle, strafe: -1 }, 60);
    expect(p.x).toBeCloseTo(wall + PLAYER_RADIUS, 6);
    expect(p.vx).toBeCloseTo(0, 6);
  });
});

describe("lookDelta", () => {
  it("passes the vertical look through, or negates it when inverted", () => {
    expect(lookDelta(12, false)).toBe(12);
    expect(lookDelta(-3, false)).toBe(-3);
    expect(lookDelta(12, true)).toBe(-12);
    expect(lookDelta(-3, true)).toBe(3);
  });

  it("turns the pitch the other way when inverted", () => {
    const p = spawnPlayer(room);
    const down = run(p, { ...idle, lookDy: lookDelta(40, false) }, 1);
    const up = run(p, { ...idle, lookDy: lookDelta(40, true) }, 1);
    expect(down.pitch).toBeLessThan(0);
    expect(up.pitch).toBeGreaterThan(0);
  });
});
