/**
 * Test helpers: the flood fill of the reachability tests, the points a
 * player stands on to use a way or a fixture, and the odd places the
 * dressing and hero tests share; imported only by tests.
 *
 * `reach` and `reachedTargets` flood a 0.2 m grid from the spawn, so a test
 * can compare what a room reaches with its props and heroes against what it
 * reaches without them. `arrivalPoint` and `targetsOf` name where the player
 * stands to use each door, hatch, portal, lift, exit, terminal and machine.
 * `DEGENERATE_PLACES` and `OVER_CAP` are the odd places (an empty one, a
 * hall full of fixtures, a narrow deep hall, a hall at the large threshold
 * and the fullest room) that `dress.test.ts` and `heroes.test.ts` both hold
 * their invariants on.
 */

import { FOOTPRINTS } from "./footprints";
import { ARRIVAL_DISTANCE, wallPoint } from "./interact";
import { isFloor } from "./layout";
import { PLAYER_RADIUS } from "./move";
import { edgeKey } from "./sites";
import type {
  Box,
  PlaceInput,
  PlaceReference,
  RoomSpec,
  WallSlot,
} from "./types";
import { CELL } from "./units";

/** The distance from a point to a box, 0 inside it. */
export function distanceTo(x: number, z: number, b: Box) {
  const nx = Math.max(b.x0, Math.min(x, b.x1));
  const nz = Math.max(b.z0, Math.min(z, b.z1));
  return Math.hypot(x - nx, z - nz);
}

/** The point in front of a door, hatch, portal, lift or exit the player arrives at. */
export function arrivalPoint(slot: WallSlot) {
  const w = wallPoint(slot);
  return {
    x: w.x + w.inward[0] * ARRIVAL_DISTANCE,
    z: w.z + w.inward[1] * ARRIVAL_DISTANCE,
  };
}

/** The flood-fill grid step, in metres. */
export const FILL = 0.2;

/**
 * The points of a 0.2 m grid the player can reach from the spawn: a point
 * is free when the player's circle there overlaps no void cell (anything
 * outside the grid is void) and no blocker. Each solid is rasterised once,
 * inflated by the player's radius, rather than tested per point. Returns a
 * test of whether a point lies within one grid step of a reached one.
 */
export function reach(room: RoomSpec, blockers: readonly Box[]) {
  const nx = Math.round((room.width * CELL) / FILL) + 1;
  const nz = Math.round((room.depth * CELL) / FILL) + 1;
  const solid = new Uint8Array(nx * nz);
  const mark = (b: Box) => {
    const i0 = Math.max(0, Math.floor((b.x0 - PLAYER_RADIUS) / FILL));
    const i1 = Math.min(nx - 1, Math.ceil((b.x1 + PLAYER_RADIUS) / FILL));
    const j0 = Math.max(0, Math.floor((b.z0 - PLAYER_RADIUS) / FILL));
    const j1 = Math.min(nz - 1, Math.ceil((b.z1 + PLAYER_RADIUS) / FILL));
    for (let j = j0; j <= j1; j++)
      for (let i = i0; i <= i1; i++)
        if (distanceTo(i * FILL, j * FILL, b) < PLAYER_RADIUS)
          solid[j * nx + i] = 1;
  };
  for (let y = -1; y <= room.depth; y++)
    for (let x = -1; x <= room.width; x++)
      if (!isFloor(room.grid, x, y))
        mark({
          x0: x * CELL,
          x1: (x + 1) * CELL,
          z0: y * CELL,
          z1: (y + 1) * CELL,
        });
  for (const b of blockers) mark(b);
  const reached = new Uint8Array(nx * nz);
  const si = Math.round(((room.spawn.x + 0.5) * CELL) / FILL);
  const sj = Math.round(((room.spawn.y + 0.5) * CELL) / FILL);
  const start = sj * nx + si;
  if (solid[start] === 1) throw new Error("the spawn point is not free");
  const queue = [start];
  reached[start] = 1;
  while (queue.length > 0) {
    const k = queue.pop() as number;
    const i = k % nx;
    const j = (k - i) / nx;
    for (const [di, dj] of [
      [1, 0],
      [-1, 0],
      [0, 1],
      [0, -1],
    ] as const) {
      const a = i + di;
      const b = j + dj;
      if (a < 0 || b < 0 || a >= nx || b >= nz) continue;
      const m = b * nx + a;
      if (reached[m] === 1 || solid[m] === 1) continue;
      reached[m] = 1;
      queue.push(m);
    }
  }
  return (x: number, z: number) => {
    const ci = Math.round(x / FILL);
    const cj = Math.round(z / FILL);
    for (let j = cj - 2; j <= cj + 2; j++)
      for (let i = ci - 2; i <= ci + 2; i++) {
        if (i < 0 || j < 0 || i >= nx || j >= nz) continue;
        if (reached[j * nx + i] !== 1) continue;
        if (Math.hypot(i * FILL - x, j * FILL - z) <= FILL + 1e-9) return true;
      }
    return false;
  };
}

/**
 * Where the player stands to use each way and wall fixture: a door's,
 * hatch's, portal's, lift's or exit's arrival point, and a terminal's or
 * machine's use point, a player's radius and 0.1 m in front of its
 * footprint.
 */
export function targetsOf(room: RoomSpec) {
  const out: { label: string; x: number; z: number }[] = [];
  for (const f of room.fixtures) {
    const label = `${f.kind} ${edgeKey(f.slot)}`;
    if (
      f.kind === "door" ||
      f.kind === "hatch" ||
      f.kind === "portal" ||
      f.kind === "lift" ||
      f.kind === "exit"
    ) {
      out.push({ label, ...arrivalPoint(f.slot) });
    } else if (f.kind === "terminal" || f.kind === "machine") {
      const size =
        f.kind === "terminal"
          ? FOOTPRINTS.terminal
          : FOOTPRINTS.machine[f.machine];
      const w = wallPoint(f.slot);
      const d = size.out + PLAYER_RADIUS + 0.1;
      out.push({
        label,
        x: w.x + w.inward[0] * d,
        z: w.z + w.inward[1] * d,
      });
    }
  }
  return out;
}

/** The labels of the targets (`targetsOf`) reached from the spawn with `blockers`. */
export function reachedTargets(room: RoomSpec, blockers: readonly Box[]) {
  const can = reach(room, blockers);
  return new Set(
    targetsOf(room)
      .filter((t) => can(t.x, t.z))
      .map((t) => t.label),
  );
}

/**
 * The fullest room a probe of the generator found: a manifest under
 * construction with 30 relations, 30 sections, 60 tags and 30 inbound
 * references (24 listed). A 24 by 24 hall, four bays and a corridor. Halls
 * stop growing there, so no generated room has many more candidates; this
 * one has 317 against the cap of 280. Its seed draws no hero.
 */
export const OVER_CAP: PlaceInput = {
  domain: "t",
  permalink: "p30-30-60-30",
  title: "R",
  type: "manifest",
  status: "draft",
  salience: null,
  validFrom: null,
  validTo: null,
  tags: Array.from({ length: 60 }, (_, k) => `t${String(k)}`),
  content: Array.from({ length: 30 }, (_, i) => `## P${String(i)}\nx`).join(
    "\n",
  ),
  relations: Array.from({ length: 30 }, (_, k) => {
    const t = `r${String(k).padStart(2, "0")}`;
    return {
      relType: "r",
      target: { domain: null, target: t },
      resolved: true,
      address: { domain: "t", permalink: t },
      targetTitle: t,
      targetSalience: 3,
    };
  }),
  links: [],
  inbound: Array.from({ length: 24 }, (_, i) => ({
    address: { domain: "t", permalink: `i${String(i)}` },
    title: `i${String(i)}`,
    relType: "l",
  })),
  inboundTotal: 30,
  observations: [],
};

/** A place with nothing in it, to be filled by `over`. */
export function place(over: Partial<PlaceInput>): PlaceInput {
  return {
    domain: "test",
    permalink: "room",
    title: "Room",
    type: null,
    status: null,
    salience: null,
    validFrom: null,
    validTo: null,
    tags: [],
    content: "",
    relations: [],
    links: [],
    inbound: [],
    inboundTotal: 0,
    observations: [],
    ...over,
  };
}

/** A located relation to `target`. */
export function rel(target: string): PlaceReference {
  return {
    relType: "relates_to",
    target: { domain: null, target },
    resolved: true,
    address: { domain: "test", permalink: target },
    targetTitle: target,
    targetSalience: 3,
  };
}

/** `n` level-two sections of one line each, as an engram's content. */
export function sections(n: number) {
  return Array.from({ length: n }, (_, i) => `## Part ${String(i)}\nline`).join(
    "\n",
  );
}

/** `n` inbound references. */
export function inbound(n: number) {
  return Array.from({ length: n }, (_, i) => ({
    address: { domain: "test", permalink: `in-${String(i)}` },
    title: `In ${String(i)}`,
    relType: "links_to",
  }));
}

/**
 * The odd places of the degenerate-room tests (Review Focus 1 and 5), each
 * as one place; a test spreads a `type` and a `status` over it to walk the
 * archetypes and conditions:
 *
 * - `empty`: nothing at all, a 5 by 6 hall with no interior band;
 * - `full`: a runbook under construction whose hall's every wall slot holds
 *   a fixture (3 relations, 2 inbound, 2 sections, 2 tags, 2 observations),
 *   no bays;
 * - `narrow`: a guide with five tags, a narrow deep 5 by 12 hall;
 * - `threshold`: a guide with 3 sections and 3 inbound references, a 9 by
 *   8 hall, the smallest large hall (D5), whose band holds one cluster
 *   block.
 */
export const DEGENERATE_PLACES = {
  empty: place({}),
  full: place({
    type: "runbook",
    status: "draft",
    relations: [rel("a"), rel("b"), rel("c")],
    inbound: inbound(2),
    inboundTotal: 2,
    content: sections(2),
    tags: ["t-1", "t-2"],
    observations: [
      { category: "one", content: "x" },
      { category: "two", content: "y" },
    ],
  }),
  narrow: place({ type: "guide", tags: ["a", "b", "c", "d", "e"] }),
  threshold: place({
    type: "guide",
    content: sections(3),
    inbound: inbound(3),
    inboundTotal: 3,
  }),
} satisfies Record<string, PlaceInput>;
