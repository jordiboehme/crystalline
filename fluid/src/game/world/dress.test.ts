import { describe, expect, it } from "vitest";

import {
  CANNED_BRIDGE,
  CANNED_HUB,
  CANNED_WORKSHOP,
  galleryRoom,
} from "./canned";
import {
  PROP_ORDER,
  WALL_GAP,
  capProps,
  dressCandidates,
  dressRoom,
  type Candidate,
} from "./dress";
import dressSource from "./dress.ts?raw";
import {
  FOOTPRINTS,
  decorFootprint,
  footprint,
  footprintOf,
  propFootprint,
} from "./footprints";
import { generateRoom } from "./generate";
import generateSource from "./generate.ts?raw";
import { ARRIVAL_DISTANCE, wallPoint } from "./interact";
import { doorwayColumns, isFloor, wallSlots } from "./layout";
import { PLAYER_RADIUS, blockersFor } from "./move";
import { EXTRAS, PALETTES, PROP_CATALOGUE, PROP_CAP } from "./props";
import {
  dressingSites,
  edgeKey,
  fitsFloor,
  overlaps,
  turnForSide,
  wallAnchor,
  type DressingSites,
  type RoomBase,
} from "./sites";
import type {
  Archetype,
  Box,
  Condition,
  PlaceInput,
  PlaceReference,
  Prop,
  PropAnchor,
  RoomSpec,
  Side,
  WallSlot,
} from "./types";
import { CELL } from "./units";

const ARCHETYPE_TYPES = {
  bridge: "manifest",
  council: "decision",
  engineering: "runbook",
  archive: "reference",
  lab: "guide",
} satisfies Record<Archetype, string>;

const STATUS = {
  clean: "stable",
  construction: "draft",
  dim: "deprecated",
  derelict: "archived",
} satisfies Record<Condition, string>;

interface Dressed {
  name: string;
  archetype: Archetype;
  condition: Condition;
  room: RoomSpec;
}

/** The room of `place` in every archetype and every condition. */
function matrix(place: PlaceInput): Dressed[] {
  const out: Dressed[] = [];
  for (const [archetype, type] of Object.entries(ARCHETYPE_TYPES))
    for (const [condition, status] of Object.entries(STATUS)) {
      const room = generateRoom({ ...place, type, status });
      expect(room.archetype).toBe(archetype);
      expect(room.condition).toBe(condition);
      out.push({
        name: `${place.permalink} ${archetype} ${condition}`,
        archetype: archetype as Archetype,
        condition: condition as Condition,
        room,
      });
    }
  return out;
}

const HUBS = matrix(CANNED_HUB);
const WORKSHOPS = matrix(CANNED_WORKSHOP);
const ALL = [...WORKSHOPS, ...HUBS];
const BRIDGES = matrix(CANNED_BRIDGE);
const workshop = generateRoom(CANNED_WORKSHOP);

/** A place with nothing in it, to be filled by `over`. */
function place(over: Partial<PlaceInput>): PlaceInput {
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
function rel(target: string): PlaceReference {
  return {
    relType: "relates_to",
    target: { domain: null, target },
    resolved: true,
    address: { domain: "test", permalink: target },
    targetTitle: target,
    targetSalience: 3,
  };
}

function sections(n: number) {
  return Array.from({ length: n }, (_, i) => `## Part ${String(i)}\nline`).join(
    "\n",
  );
}

function inbound(n: number) {
  return Array.from({ length: n }, (_, i) => ({
    address: { domain: "test", permalink: `in-${String(i)}` },
    title: `In ${String(i)}`,
    relType: "links_to",
  }));
}

const SIDE_OF_TURN: readonly Side[] = ["s", "w", "n", "e"];

/** The wall edge a wall or ceiling prop is anchored on (ruling 1). */
function edgeOf(p: Prop): WallSlot {
  const side = SIDE_OF_TURN[p.turn];
  if (side === undefined) throw new Error(`bad turn ${String(p.turn)}`);
  switch (side) {
    case "n":
      return { x: p.x - 0.5, y: p.y, side };
    case "s":
      return { x: p.x - 0.5, y: p.y - 1, side };
    case "w":
      return { x: p.x, y: p.y - 0.5, side };
    case "e":
      return { x: p.x - 1, y: p.y - 0.5, side };
  }
}

const isRun = (p: Prop) => PROP_CATALOGUE[p.kind].run;
const wallProps = (r: RoomSpec) =>
  r.props.filter((p) => p.anchor === "wall" && !isRun(p));
const floorProps = (r: RoomSpec) => r.props.filter((p) => p.anchor === "floor");
const boxOf = (p: Prop): Box => {
  const box = propFootprint(p);
  if (box === null) throw new Error(`${p.kind} has no footprint`);
  return box;
};

/** The distance from a point to a box, 0 inside it. */
function distanceTo(x: number, z: number, b: Box) {
  const nx = Math.max(b.x0, Math.min(x, b.x1));
  const nz = Math.max(b.z0, Math.min(z, b.z1));
  return Math.hypot(x - nx, z - nz);
}

/** The point in front of a door, hatch or portal the player arrives at. */
function arrivalPoint(slot: WallSlot) {
  const w = wallPoint(slot);
  return {
    x: w.x + w.inward[0] * ARRIVAL_DISTANCE,
    z: w.z + w.inward[1] * ARRIVAL_DISTANCE,
  };
}

/** Every box a floor prop must stay out of: fixtures, decor, scaffolding. */
function takenBoxes(room: RoomBase): Box[] {
  return [
    ...room.fixtures.map(footprintOf),
    ...room.decor.map(decorFootprint),
    ...room.scaffold,
  ].filter((b) => b !== null);
}

/** The next and previous edge of `e` in its wall run, when they exist. */
function neighbours(sites: DressingSites, e: WallSlot): WallSlot[] {
  const k = edgeKey(e);
  for (const run of sites.runs) {
    const i = run.findIndex((r) => edgeKey(r) === k);
    if (i < 0) continue;
    return [run[i + 1], run[i - 1]].filter((n) => n !== undefined);
  }
  throw new Error(`no run holds ${k}`);
}

describe("the shape of every prop", () => {
  it("has its kind's anchor, a variant of its kind, a quarter turn and three decimals", () => {
    let n = 0;
    for (const { name, room } of [
      ...ALL,
      { name: "bridge", room: generateRoom(CANNED_BRIDGE) },
    ]) {
      expect(room.props.length, name).toBeGreaterThan(0);
      for (const p of room.props) {
        const entry = PROP_CATALOGUE[p.kind];
        expect(p.anchor, name).toBe(entry.anchor);
        expect(Number.isInteger(p.variant)).toBe(true);
        expect(p.variant).toBeGreaterThanOrEqual(0);
        expect(p.variant).toBeLessThan(entry.variants);
        expect([0, 1, 2, 3]).toContain(p.turn);
        expect(Math.round(p.x * 1000) / 1000).toBe(p.x);
        expect(Math.round(p.y * 1000) / 1000).toBe(p.y);
        expect(Number.isSafeInteger(p.seed)).toBe(true);
        n++;
      }
    }
    expect(n).toBeGreaterThan(1000);
  });

  it("sorts its output wall, floor, ceiling, then by y, x, turn, kind and variant", () => {
    for (const { room } of ALL) {
      const sorted = [...room.props].sort(PROP_ORDER);
      expect(room.props).toEqual(sorted);
    }
    const tier = (a: PropAnchor) => ["wall", "floor", "ceiling"].indexOf(a);
    const props = workshop.props;
    for (let i = 1; i < props.length; i++) {
      const a = props[i - 1];
      const b = props[i];
      if (a === undefined || b === undefined) continue;
      expect(tier(a.anchor)).toBeLessThanOrEqual(tier(b.anchor));
      if (a.anchor === b.anchor) expect(a.y).toBeLessThanOrEqual(b.y);
    }
  });
});

/**
 * The wall-prop invariants, for any room: every wall prop (runs aside) sits
 * at the anchor of a free edge, turned to face into the room, one to an
 * edge, never on a fixture edge, the entrance edge or its neighbours, or a
 * doorway-column edge. Returns how many it checked.
 */
function expectWallInvariants(name: string, room: RoomSpec) {
  const sites = dressingSites(room);
  const cols = doorwayColumns(room);
  const fixtures = new Set(room.fixtures.map((f) => edgeKey(f.slot)));
  const e0 = room.entrance;
  const seen = new Set<string>();
  const props = wallProps(room);
  for (const p of props) {
    const e = edgeOf(p);
    const k = edgeKey(e);
    expect(wallAnchor(e), `${name} ${p.kind}`).toEqual({
      x: p.x,
      y: p.y,
      turn: p.turn,
    });
    expect(p.turn).toBe(turnForSide(e.side));
    expect(sites.free.has(k), `${name} ${k}`).toBe(true);
    expect(seen.has(k), `${name} ${k} twice`).toBe(false);
    seen.add(k);
    expect(fixtures.has(k), `${name} ${k}`).toBe(false);
    expect(
      e.side === "s" && e.y === e0.y && Math.abs(e.x - e0.x) <= 1,
      `${name} ${k}`,
    ).toBe(false);
    expect(cols.has(e.x), `${name} ${k}`).toBe(false);
  }
  return props.length;
}

/**
 * The floor-prop invariants, for any room: every floor prop's footprint
 * fits the floor and overlaps no lane, no fixture, decor or scaffold box
 * and no other floor prop; no cell under it is a doorway cell, next to one
 * (ruling 10) or in the corridor; and no two floor props share a cell.
 * Returns how many it checked.
 */
function expectFloorInvariants(name: string, room: RoomSpec) {
  const sites = dressingSites(room);
  const taken = takenBoxes(room);
  const cols = doorwayColumns(room);
  const doorway = (x: number, y: number) =>
    cols.has(x) && isFloor(room.grid, x, y);
  const nearDoorway = (x: number, y: number) =>
    doorway(x, y) ||
    doorway(x + 1, y) ||
    doorway(x - 1, y) ||
    doorway(x, y + 1) ||
    doorway(x, y - 1);
  const boxes = floorProps(room).map(boxOf);
  for (const [i, box] of boxes.entries()) {
    const label = `${name} ${JSON.stringify(box)}`;
    expect(fitsFloor(room, box), label).toBe(true);
    for (const lane of sites.lanes)
      expect(overlaps(box, lane), label).toBe(false);
    for (const t of taken) expect(overlaps(box, t), label).toBe(false);
    for (const other of boxes.slice(i + 1))
      expect(overlaps(box, other), label).toBe(false);
    const c = room.corridor;
    for (
      let y = Math.floor(box.z0 / CELL);
      y <= Math.floor((box.z1 - 1e-9) / CELL);
      y++
    )
      for (
        let x = Math.floor(box.x0 / CELL);
        x <= Math.floor((box.x1 - 1e-9) / CELL);
        x++
      ) {
        expect(cols.has(x), label).toBe(false);
        expect(nearDoorway(x, y), label).toBe(false);
        if (c !== null)
          expect(x >= c.x0 && x < c.x1 && y >= c.y0 && y < c.y1, label).toBe(
            false,
          );
      }
  }
  const cells = floorProps(room).map(
    (p) => `${String(Math.floor(p.x))},${String(Math.floor(p.y))}`,
  );
  expect(new Set(cells).size, name).toBe(cells.length);
  return boxes.length;
}

describe("wall props", () => {
  it("stand at the anchor of a free edge, one to an edge, never on a fixture, the entrance or a doorway", () => {
    for (const { name, room } of ALL)
      expect(expectWallInvariants(name, room), name).toBeGreaterThan(0);
  });

  it("put a sign plate beside every door and hatch, unless both neighbours are taken or excluded", () => {
    let signs = 0;
    for (const { name, room } of ALL) {
      const sites = dressingSites(room);
      const onEdge = new Map(
        wallProps(room).map((p) => [edgeKey(edgeOf(p)), p]),
      );
      for (const f of room.fixtures) {
        if (f.kind !== "door" && f.kind !== "hatch") continue;
        const near = neighbours(sites, f.slot);
        if (near.some((e) => onEdge.get(edgeKey(e))?.kind === "sign-plate")) {
          signs++;
          continue;
        }
        for (const e of near) {
          const k = edgeKey(e);
          expect(!sites.free.has(k) || onEdge.has(k), `${name} ${k}`).toBe(
            true,
          );
        }
      }
    }
    expect(signs).toBeGreaterThan(100);
  });
});

describe("runs", () => {
  it("sit only on long-wall edges no run may cover, and so never on a fixture edge", () => {
    let segments = 0;
    for (const { name, room } of ALL) {
      const sites = dressingSites(room);
      const long = new Set(sites.longWalls.flat().map(edgeKey));
      const fixtures = new Set(room.fixtures.map((f) => edgeKey(f.slot)));
      for (const p of room.props.filter(isRun)) {
        const e = edgeOf(p);
        const k = edgeKey(e);
        expect(wallAnchor(e)).toEqual({ x: p.x, y: p.y, turn: p.turn });
        expect(long.has(k), `${name} ${p.kind} ${k}`).toBe(true);
        expect(sites.noRun.has(k), `${name} ${p.kind} ${k}`).toBe(false);
        expect(fixtures.has(k), `${name} ${k}`).toBe(false);
        segments++;
      }
    }
    expect(segments).toBeGreaterThan(100);
  });

  it("gives every segment of one run the same variant", () => {
    for (const { name, room } of ALL) {
      for (const kind of PROP_KIND_RUNS) {
        const variants = new Set(
          room.props.filter((p) => p.kind === kind).map((p) => p.variant),
        );
        expect(variants.size, `${name} ${kind}`).toBeLessThanOrEqual(1);
      }
    }
  });

  it("appear only where the palette has one, of the palette's kind", () => {
    for (const { name, archetype, room } of WORKSHOPS) {
      const palette = PALETTES[archetype];
      const wall = room.props.filter((p) => isRun(p) && p.anchor === "wall");
      const ceiling = room.props.filter(
        (p) => isRun(p) && p.anchor === "ceiling",
      );
      if (palette.wallRun === null) expect(wall, name).toEqual([]);
      else {
        expect(wall.length, name).toBeGreaterThan(0);
        for (const p of wall) expect(p.kind).toBe(palette.wallRun);
      }
      if (palette.ceilingRun === null) expect(ceiling, name).toEqual([]);
      else {
        expect(ceiling.length, name).toBeGreaterThan(0);
        for (const p of ceiling) expect(p.kind).toBe(palette.ceilingRun);
      }
    }
  });
});

const PROP_KIND_RUNS = (
  Object.keys(PROP_CATALOGUE) as (keyof typeof PROP_CATALOGUE)[]
).filter((k) => PROP_CATALOGUE[k].run);

describe("floor props", () => {
  it("fit the floor and keep out of lanes, fixtures, decor, scaffolding, each other, doorways and the corridor", () => {
    let n = 0;
    for (const { name, room } of ALL) n += expectFloorInvariants(name, room);
    expect(n).toBeGreaterThan(100);
  });

  it("leave the player's circle at every arrival point clear", () => {
    let checked = 0;
    for (const { name, room } of ALL) {
      const boxes = floorProps(room).map(boxOf);
      for (const f of room.fixtures) {
        if (f.kind !== "door" && f.kind !== "hatch" && f.kind !== "portal")
          continue;
        const a = arrivalPoint(f.slot);
        for (const b of boxes)
          expect(distanceTo(a.x, a.z, b), name).toBeGreaterThanOrEqual(
            PLAYER_RADIUS,
          );
        checked++;
      }
    }
    expect(checked).toBeGreaterThan(500);
  });

  it("leave the viewing lane in front of every poster and the placard clear", () => {
    // The sheet's bottom edge hangs at SHEET_BOTTOM (1.2 m, `wall.ts`), lower
    // than most floor props stand, so no floor prop may stand within the
    // sheet's width and 1.5 m in front of it.
    const SHEET_BOTTOM = 1.2;
    const view = { along: SHEET_BOTTOM, out: 1.5 };
    let checked = 0;
    for (const { name, room } of [...ALL, ...BRIDGES]) {
      const boxes = floorProps(room).map(boxOf);
      for (const f of room.fixtures) {
        if (f.kind !== "poster" && f.kind !== "placard") continue;
        const lane = footprint(f.slot, view);
        for (const b of boxes)
          expect(
            overlaps(b, lane),
            `${name} ${f.kind} ${edgeKey(f.slot)}`,
          ).toBe(false);
        checked++;
      }
    }
    expect(checked).toBeGreaterThan(100);
  });

  it("backs a wall-side prop onto its wall, WALL_GAP off it and facing away, on all four walls", () => {
    const rooms = [
      ...ALL.map((d) => d.room),
      ...Array.from({ length: 40 }, (_, i) =>
        generateRoom(
          place({
            type: "runbook",
            permalink: `narrow-${String(i)}`,
            tags: ["a", "b", "c", "d", "e"],
          }),
        ),
      ),
    ];
    const sides = new Set<Side>();
    for (const room of rooms) {
      const spots = new Map(
        dressingSites(room).wallSide.map((s) => [`${s.cx},${s.cy}`, s]),
      );
      for (const p of floorProps(room)) {
        const cx = Math.floor(p.x);
        const cy = Math.floor(p.y);
        const spot = spots.get(`${String(cx)},${String(cy)}`);
        if (spot === undefined || spot.wall === null) continue;
        const side = spot.wall;
        sides.add(side);
        const box = boxOf(p);
        expect(p.turn).toBe(turnForSide(side));
        expect(fitsFloor(room, box)).toBe(true);
        const gap = {
          n: box.z0 - cy * CELL,
          s: (cy + 1) * CELL - box.z1,
          w: box.x0 - cx * CELL,
          e: (cx + 1) * CELL - box.x1,
        }[side];
        expect(gap).toBeCloseTo(WALL_GAP, 2);
      }
    }
    expect([...sides].sort()).toEqual(["e", "n", "s", "w"]);
  });

  it("keeps floor props out of a reserved box", () => {
    const props = floorProps(workshop);
    const first = props[0];
    if (first === undefined) throw new Error("the workshop has floor props");
    const reserved = boxOf(first);
    const again = dressRoom(workshop, [reserved]);
    const floors = again.filter((p) => p.anchor === "floor");
    expect(floors.length).toBeGreaterThan(0);
    for (const p of floors) expect(overlaps(boxOf(p), reserved)).toBe(false);
    expect(dressRoom(workshop)).toEqual(workshop.props);
  });
});

/** The flood-fill grid step, in metres. */
const FILL = 0.2;

/**
 * The points of a 0.2 m grid the player can reach from the spawn: a point
 * is free when the player's circle there overlaps no void cell (anything
 * outside the grid is void) and no blocker. Each solid is rasterised once,
 * inflated by the player's radius, rather than tested per point.
 */
function reach(room: RoomSpec, blockers: readonly Box[]) {
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

/** Where the player stands to use each way and wall fixture. */
function targetsOf(room: RoomSpec) {
  const out: { label: string; x: number; z: number }[] = [];
  for (const f of room.fixtures) {
    const label = `${f.kind} ${edgeKey(f.slot)}`;
    if (f.kind === "door" || f.kind === "hatch" || f.kind === "portal") {
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

/** The targets reached from the spawn with `blockers`. */
function reachedTargets(room: RoomSpec, blockers: readonly Box[]) {
  const can = reach(room, blockers);
  return new Set(
    targetsOf(room)
      .filter((t) => can(t.x, t.z))
      .map((t) => t.label),
  );
}

describe("reachability (Review Focus 1)", () => {
  it("loses a target when a bay doorway is blocked by hand", () => {
    const hub = HUBS[0]?.room;
    if (hub === undefined) throw new Error("no hub");
    const bare = { ...hub, props: [] };
    const open = reachedTargets(bare, blockersFor(bare));
    const c = hub.hall.x1;
    const wall: Box = {
      x0: c * CELL,
      x1: (c + 1) * CELL,
      z0: 0,
      z1: hub.depth * CELL,
    };
    const shut = reachedTargets(bare, [...blockersFor(bare), wall]);
    expect(open.size).toBeGreaterThan(shut.size);
  }, 20_000);

  for (const { name, room } of ALL) {
    it(`reaches with props every target ${name} reaches without them`, () => {
      const bare = { ...room, props: [] };
      const without = reachedTargets(bare, blockersFor(bare));
      expect(without.size, name).toBeGreaterThan(0);
      const withProps = reachedTargets(room, blockersFor(room));
      for (const t of without)
        expect(withProps.has(t), `${name} ${t}`).toBe(true);
    }, 20_000);
  }
});

/** The workshop's clean room in every archetype. */
const CLEAN_WORKSHOPS = WORKSHOPS.filter((w) => w.condition === "clean");

describe("density", () => {
  it("puts an optional prop on about two thirds of the free edges left after the mandatory ones", () => {
    let optional = 0;
    let left = 0;
    for (const { room } of CLEAN_WORKSHOPS) {
      const candidates = dressCandidates(room);
      expect(capProps(candidates, PROP_CAP)).toHaveLength(candidates.length);
      const walls = candidates.filter(
        (c) => c.prop.anchor === "wall" && !isRun(c.prop),
      );
      const mandatory = walls.filter((c) => c.mandatory).length;
      optional += walls.length - mandatory;
      left += dressingSites(room).free.size - mandatory;
    }
    // The room seed does not depend on the type, so the five rooms draw
    // the same edges; the share is still that of the rule.
    expect(left).toBeGreaterThanOrEqual(50);
    const share = optional / left;
    expect(share).toBeGreaterThanOrEqual(0.55);
    expect(share).toBeLessThanOrEqual(0.78);
  });

  it("keeps that share over workshops of many seeds", () => {
    let optional = 0;
    let left = 0;
    for (let i = 0; i < 24; i++) {
      const room = generateRoom({
        ...CANNED_WORKSHOP,
        status: "stable",
        permalink: `workshop-${String(i)}`,
      });
      const walls = dressCandidates(room).filter(
        (c) => c.prop.anchor === "wall" && !isRun(c.prop),
      );
      const mandatory = walls.filter((c) => c.mandatory).length;
      optional += walls.length - mandatory;
      left += dressingSites(room).free.size - mandatory;
    }
    expect(left).toBeGreaterThan(200);
    const share = optional / left;
    expect(share).toBeGreaterThanOrEqual(0.55);
    expect(share).toBeLessThanOrEqual(0.78);
  });

  it("gives a corner zone with two accepted spots one or two floor props, one on a bridge", () => {
    let zones = 0;
    for (const { name, archetype, room } of CLEAN_WORKSHOPS) {
      const sites = dressingSites(room);
      const palette = PALETTES[archetype];
      const kinds = palette.floor
        .map(([k]) => k)
        .filter((k) => !PROP_CATALOGUE[k].wallBacked);
      // A spot is accepted when every box a zone prop could take there is.
      const accepted = (cx: number, cy: number) =>
        kinds.every((kind) =>
          FOOTPRINTS.prop[kind].every((_, variant) =>
            [0, 1, 2, 3].every((turn) => {
              const box = boxOf({
                kind,
                variant,
                anchor: "floor",
                x: cx + 0.5,
                y: cy + 0.5,
                turn,
                seed: 0,
              });
              return (
                fitsFloor(room, box) &&
                !sites.lanes.some((l) => overlaps(box, l)) &&
                !sites.taken.some((t) => overlaps(box, t))
              );
            }),
          ),
        );
      for (const z of sites.zones) {
        const spots = z.spots.filter((s) => accepted(s.cx, s.cy));
        if (spots.length < 2) continue;
        const cells = new Set(z.spots.map((s) => `${s.cx},${s.cy}`));
        const n = floorProps(room).filter((p) =>
          cells.has(`${String(Math.floor(p.x))},${String(Math.floor(p.y))}`),
        ).length;
        if (archetype === "bridge") expect(n, `${name} ${z.key}`).toBe(1);
        else {
          expect(n, `${name} ${z.key}`).toBeGreaterThanOrEqual(1);
          expect(n, `${name} ${z.key}`).toBeLessThanOrEqual(palette.cornerMax);
        }
        zones++;
      }
    }
    expect(zones).toBeGreaterThanOrEqual(10);
  });

  it("hangs ceiling runs on one or two long walls, and none in a council", () => {
    for (const { name, archetype, room } of CLEAN_WORKSHOPS) {
      const sides = new Set(
        room.props
          .filter((p) => p.anchor === "ceiling" && isRun(p))
          .map((p) => edgeOf(p).side),
      );
      if (PALETTES[archetype].ceilingRun === null) {
        expect(archetype).toBe("council");
        expect(sides.size, name).toBe(0);
      } else {
        expect(sides.size, name).toBeGreaterThanOrEqual(1);
        expect(sides.size, name).toBeLessThanOrEqual(2);
      }
    }
  });
});

describe("mandatory props", () => {
  it("gives the workshop the hall it was made for", () => {
    expect(workshop.hall).toEqual({ x0: 0, y0: 0, x1: 13, y1: 10 });
    expect(workshop.bays).toEqual([]);
    expect(workshop.corridor).toBeNull();
    expect(workshop.dropped).toBe(0);
    expect(workshop.archetype).toBe("engineering");
    expect(workshop.condition).toBe("construction");
    expect(workshop.scaffold.length).toBeGreaterThan(0);
    const count = (kind: string) =>
      workshop.fixtures.filter((f) => f.kind === kind).length;
    expect(count("door")).toBe(6);
    expect(count("terminal")).toBe(3);
    expect(count("machine")).toBe(4);
    expect(count("hatch")).toBe(3);
    expect(count("poster")).toBe(2);
    expect(count("placard")).toBe(1);
  });

  it("puts a keycard reader beside every unsealed door, unless both neighbours are taken or excluded", () => {
    let readers = 0;
    for (const { name, room } of WORKSHOPS) {
      const sites = dressingSites(room);
      const onEdge = new Map(
        wallProps(room).map((p) => [edgeKey(edgeOf(p)), p]),
      );
      for (const f of room.fixtures) {
        if (f.kind !== "door" || f.address === null) continue;
        const near = neighbours(sites, f.slot);
        const reader = near.some(
          (e) => onEdge.get(edgeKey(e))?.kind === "keycard-reader",
        );
        if (reader) readers++;
        else
          for (const e of near) {
            const k = edgeKey(e);
            expect(!sites.free.has(k) || onEdge.has(k), `${name} ${k}`).toBe(
              true,
            );
          }
      }
    }
    expect(readers).toBeGreaterThanOrEqual(6 * WORKSHOPS.length);
  });

  it("puts an extinguisher near every sixth edge of a run, unless all three edges are spoken for", () => {
    let extinguishers = 0;
    let spokenFor = 0;
    for (const { name, room } of WORKSHOPS) {
      const sites = dressingSites(room);
      const onEdge = new Map(
        wallProps(room).map((p) => [edgeKey(edgeOf(p)), p]),
      );
      for (const run of sites.runs) {
        for (let i = 3; i < run.length; i += 6) {
          const edges = [run[i], run[i + 1], run[i - 1]];
          const has = edges.some(
            (e) =>
              e !== undefined &&
              onEdge.get(edgeKey(e))?.kind === "extinguisher",
          );
          if (has) {
            extinguishers++;
            continue;
          }
          spokenFor++;
          for (const e of edges) {
            if (e === undefined) continue;
            const k = edgeKey(e);
            const kind = onEdge.get(k)?.kind;
            expect(
              !sites.free.has(k) ||
                kind === "keycard-reader" ||
                kind === "sign-plate",
              `${name} ${k}`,
            ).toBe(true);
          }
        }
      }
    }
    expect(extinguishers).toBeGreaterThan(0);
    expect(spokenFor).toBeGreaterThan(0);
    // The north wall: six doors on the odd edges, their readers on the even.
    const north = wallProps(workshop).filter(
      (p) => edgeOf(p).side === "n" && edgeOf(p).y === 0,
    );
    expect(north.filter((p) => p.kind === "keycard-reader")).toHaveLength(6);
    expect(north.some((p) => p.kind === "extinguisher")).toBe(false);
  });
});

describe("the cap (Review Focus 4)", () => {
  const TIERS: readonly PropAnchor[] = ["ceiling", "floor", "wall"];

  /**
   * Checks what `capProps` kept of `candidates` against the tier rules:
   * a tier loses props only when every lower tier lost all of its own, and
   * within a tier a mandatory prop goes only when no optional one is left.
   * Returns how many were dropped.
   */
  function expectTiers(name: string, candidates: Candidate[], kept: Prop[]) {
    const left = new Map<string, number>();
    for (const p of kept) {
      const k = JSON.stringify(p);
      left.set(k, (left.get(k) ?? 0) + 1);
    }
    const dropped: Candidate[] = [];
    const survived: Candidate[] = [];
    for (const c of candidates) {
      const k = JSON.stringify(c.prop);
      const n = left.get(k) ?? 0;
      if (n > 0) {
        left.set(k, n - 1);
        survived.push(c);
      } else dropped.push(c);
    }
    expect(survived, name).toHaveLength(kept.length);
    for (const [t, tier] of TIERS.entries()) {
      if (!dropped.some((c) => c.prop.anchor === tier)) continue;
      for (const lower of TIERS.slice(0, t))
        expect(
          survived.some((c) => c.prop.anchor === lower),
          `${name}: ${lower} kept while ${tier} was dropped`,
        ).toBe(false);
      if (dropped.some((c) => c.prop.anchor === tier && c.mandatory))
        expect(
          survived.some((c) => c.prop.anchor === tier && !c.mandatory),
          `${name}: optional ${tier} kept while a mandatory one was dropped`,
        ).toBe(false);
    }
    return dropped.length;
  }

  it("keeps at most PROP_CAP props in every hub, the capped candidates in output order", () => {
    for (const { name, room } of HUBS) {
      expect(room.props.length, name).toBeLessThanOrEqual(PROP_CAP);
      const candidates = dressCandidates(room);
      const kept = capProps(candidates, PROP_CAP);
      expect(room.props, name).toEqual([...kept].sort(PROP_ORDER));
      expectTiers(name, candidates, kept);
    }
  });

  it("drops ceiling, then floor, then wall from a hub's own props, mandatory ones last", () => {
    // CANNED_HUB stays under PROP_CAP in every archetype (its long walls are
    // mostly doors, so its runs are short), so tighter caps put the real mix
    // of a hub through the same rules.
    let dropped = 0;
    let wallsLost = 0;
    for (const { name, room } of HUBS) {
      const candidates = dressCandidates(room);
      const walls = candidates.filter((c) => c.prop.anchor === "wall").length;
      for (const cap of [100, walls, walls - 20, 40]) {
        const kept = capProps(candidates, cap);
        expect(kept).toHaveLength(Math.min(cap, candidates.length));
        dropped += expectTiers(`${name} cap ${String(cap)}`, candidates, kept);
        if (cap < walls) {
          wallsLost++;
          const mandatory = candidates.filter(
            (c) => c.mandatory && c.prop.anchor === "wall",
          );
          if (cap >= mandatory.length)
            for (const m of mandatory) expect(kept).toContain(m.prop);
        }
      }
    }
    expect(dropped).toBeGreaterThan(0);
    expect(wallsLost).toBeGreaterThan(0);
  });

  /**
   * The fullest room a probe of the generator found: a manifest under
   * construction with 24 relations, 24 sections, 40 tags and 30 inbound
   * references (24 listed). A 24 by 24 hall, four bays and a corridor give
   * it 203 candidates, three over the cap.
   */
  const OVER_CAP: PlaceInput = {
    domain: "t",
    permalink: "p24-24-40-30",
    title: "R",
    type: "manifest",
    status: "draft",
    salience: null,
    validFrom: null,
    validTo: null,
    tags: Array.from({ length: 40 }, (_, k) => `t${String(k)}`),
    content: Array.from({ length: 24 }, (_, i) => `## P${String(i)}\nx`).join(
      "\n",
    ),
    relations: Array.from({ length: 24 }, (_, k) => {
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

  it("caps a room that really overflows, dropping only optional ceiling props", () => {
    const room = generateRoom(OVER_CAP);
    expect(room.hall).toEqual({ x0: 26, y0: 0, x1: 50, y1: 24 });
    expect(room.bays).toHaveLength(4);
    expect(room.corridor).not.toBeNull();
    const candidates = dressCandidates(room);
    expect(candidates).toHaveLength(PROP_CAP + 3);
    expect(room.props).toHaveLength(PROP_CAP);
    const kept = capProps(candidates, PROP_CAP);
    expect(room.props).toEqual([...kept].sort(PROP_ORDER));
    expect(expectTiers("over cap", candidates, kept)).toBe(3);
    const left = new Set(kept.map((p) => JSON.stringify(p)));
    const dropped = candidates.filter((c) => !left.has(JSON.stringify(c.prop)));
    expect(dropped).toHaveLength(3);
    for (const c of dropped) {
      expect(c.prop.anchor).toBe("ceiling");
      expect(c.mandatory).toBe(false);
    }
    expect(room.props.filter((p) => p.kind === "beacon")).toHaveLength(1);
  });

  const prop = (anchor: PropAnchor, seed: number, x = 0): Prop => ({
    kind:
      anchor === "wall"
        ? "vent-grille"
        : anchor === "floor"
          ? "crate"
          : "beacon",
    variant: 0,
    anchor,
    x,
    y: 0,
    turn: 0,
    seed,
  });

  it("drops ceiling before floor before wall", () => {
    const list: Candidate[] = [
      { prop: prop("wall", 9), mandatory: false },
      { prop: prop("floor", 1), mandatory: false },
      { prop: prop("ceiling", 2), mandatory: true },
      { prop: prop("wall", 8), mandatory: false },
    ];
    expect(
      capProps(list, 3)
        .map((p) => p.anchor)
        .sort(),
    ).toEqual(["floor", "wall", "wall"]);
    expect(capProps(list, 2).map((p) => p.anchor)).toEqual(["wall", "wall"]);
    expect(capProps(list, 1).map((p) => p.seed)).toEqual([8]);
  });

  it("drops optional before mandatory, highest seed first", () => {
    const list: Candidate[] = [
      { prop: prop("wall", 1), mandatory: false },
      { prop: prop("wall", 50), mandatory: true },
      { prop: prop("wall", 2), mandatory: false },
      { prop: prop("wall", 40), mandatory: true },
    ];
    expect(capProps(list, 3).map((p) => p.seed)).toEqual([40, 50, 1]);
    expect(capProps(list, 2).map((p) => p.seed)).toEqual([40, 50]);
  });

  it("gives the same props whatever the input order", () => {
    const list: Candidate[] = [];
    for (let i = 0; i < 30; i++)
      list.push({
        prop: prop(TIERS[i % 3] as PropAnchor, (i * 7) % 11, i),
        mandatory: i % 4 === 0,
      });
    const want = capProps(list, 17);
    for (let s = 1; s < 6; s++) {
      const shuffled = [...list].sort(
        (a, b) => ((a.prop.x * 31 * s) % 17) - ((b.prop.x * 31 * s) % 17),
      );
      expect(capProps(shuffled, 17)).toEqual(want);
    }
  });

  it("drops nothing under the cap", () => {
    const list: Candidate[] = [
      { prop: prop("ceiling", 3), mandatory: false },
      { prop: prop("floor", 2), mandatory: false },
    ];
    expect(capProps(list, 2)).toEqual(list.map((c) => c.prop));
    expect(capProps(list, 5)).toEqual(list.map((c) => c.prop));
  });
});

describe("locality", () => {
  // Locality holds only below the cap: near PROP_CAP a new fixture can
  // change which props are dropped anywhere in the room, so both rooms here
  // are asserted to stay under it.
  it("changes only props near a new door's slot", () => {
    const P = place({
      type: "runbook",
      status: "stable",
      relations: [rel("a"), rel("b")],
      inbound: inbound(5),
      inboundTotal: 5,
      content: sections(2),
      tags: ["t-1", "t-2"],
    });
    const P2 = { ...P, relations: [...P.relations, rel("z")] };
    const a = generateRoom(P);
    const b = generateRoom(P2);
    expect(a.hall).toEqual(b.hall);
    expect(a.hall.x1 - a.hall.x0).toBe(13);
    const door = b.fixtures.find(
      (f) => f.kind === "door" && f.address?.permalink === "z",
    );
    if (door === undefined) throw new Error("no new door");
    const slot = door.slot;
    expect(a.fixtures.some((f) => edgeKey(f.slot) === edgeKey(slot))).toBe(
      false,
    );
    expect(wallSlots(a.grid).map(edgeKey)).toContain(edgeKey(slot));
    expect(dressCandidates(a).length).toBeLessThanOrEqual(PROP_CAP);
    expect(dressCandidates(b).length).toBeLessThanOrEqual(PROP_CAP);

    const cellOf = (p: Prop) => {
      if (p.anchor === "floor") return [Math.floor(p.x), Math.floor(p.y)];
      const e = edgeOf(p);
      return [e.x, e.y];
    };
    const sa = new Set(a.props.map((p) => JSON.stringify(p)));
    const sb = new Set(b.props.map((p) => JSON.stringify(p)));
    const diff = [
      ...[...sa].filter((k) => !sb.has(k)),
      ...[...sb].filter((k) => !sa.has(k)),
    ];
    expect(diff.length).toBeGreaterThan(0);
    for (const k of diff) {
      const [x, y] = cellOf(JSON.parse(k) as Prop) as [number, number];
      expect(
        Math.max(Math.abs(x - slot.x), Math.abs(y - slot.y)),
        k,
      ).toBeLessThanOrEqual(3);
    }
  });
});

describe("condition extras", () => {
  const extraKinds = (c: Condition) =>
    new Set<string>(EXTRAS[c].map((r) => r.kind));

  it("appear only in their condition, at least one in each, none when clean", () => {
    for (const { name, condition, room } of WORKSHOPS) {
      const extras = room.props.filter((p) => PROP_CATALOGUE[p.kind].extra);
      const own = extraKinds(condition);
      for (const p of extras)
        expect(own.has(p.kind), `${name} ${p.kind}`).toBe(true);
      if (condition === "clean") expect(extras, name).toEqual([]);
      else expect(extras.length, name).toBeGreaterThan(0);
    }
  });
});

describe("degenerate rooms (Review Focus 5)", () => {
  it("dresses an empty place without throwing, keeping every invariant", () => {
    let walls = 0;
    for (const status of Object.values(STATUS))
      for (const type of Object.values(ARCHETYPE_TYPES)) {
        const room = generateRoom(place({ type, status }));
        expect(room.hall).toEqual({ x0: 0, y0: 0, x1: 5, y1: 6 });
        const name = `empty ${type} ${status}`;
        walls += expectWallInvariants(name, room);
        expectFloorInvariants(name, room);
      }
    expect(walls).toBeGreaterThan(0);
  });

  it("gives a hall full of fixtures only wall props off the slots, keeping every invariant", () => {
    const full = place({
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
    });
    const room = generateRoom(full);
    expect(room.bays).toEqual([]);
    expect(room.dropped).toBe(0);
    const used = new Set(room.fixtures.map((f) => edgeKey(f.slot)));
    const entrance = edgeKey({ ...room.entrance, side: "s" });
    const slots = wallSlots(room.grid).map(edgeKey);
    for (const k of slots)
      expect(used.has(k) || k === entrance, `slot ${k}`).toBe(true);
    const slotSet = new Set(slots);
    expect(expectWallInvariants("full", room)).toBeGreaterThan(0);
    for (const p of wallProps(room))
      expect(slotSet.has(edgeKey(edgeOf(p)))).toBe(false);
    expectFloorInvariants("full", room);
  });

  it("dresses a narrow, deep hall, keeping every invariant", () => {
    for (const status of Object.values(STATUS)) {
      const room = generateRoom(
        place({
          type: "guide",
          status,
          tags: ["a", "b", "c", "d", "e"],
        }),
      );
      expect(room.hall.x1 - room.hall.x0).toBe(5);
      expect(room.hall.y1 - room.hall.y0).toBe(12);
      expect(expectWallInvariants(`narrow ${status}`, room)).toBeGreaterThan(0);
      expectFloorInvariants(`narrow ${status}`, room);
    }
  });

  it("dresses the gallery by hand, keeping every invariant", () => {
    const gallery = galleryRoom();
    expect(gallery.props.length).toBeGreaterThan(0);
    expectWallInvariants("gallery", gallery);
    expectFloorInvariants("gallery", gallery);
    expect(blockersFor(gallery)).toHaveLength(
      takenBoxes(gallery).length + floorProps(gallery).length,
    );
  });
});

describe("the generator side's imports (ruling 20)", () => {
  it("keeps dress.ts away from move, generate and interact", () => {
    expect(dressSource).not.toMatch(
      /from\s+["']\.\/(move|generate|interact)["']/,
    );
  });

  it("keeps generate.ts away from move and interact", () => {
    expect(generateSource).not.toMatch(/from\s+["']\.\/(move|interact)["']/);
  });
});

describe("determinism", () => {
  it("dresses the same room every time, whatever order the lists come in", () => {
    for (const status of Object.values(STATUS)) {
      const p = { ...CANNED_WORKSHOP, status };
      const room = generateRoom(p);
      expect(JSON.stringify(generateRoom(p))).toBe(JSON.stringify(room));
      const shuffled = generateRoom({
        ...p,
        tags: [...p.tags].reverse(),
        relations: [...p.relations].reverse(),
      });
      expect(shuffled.props).toEqual(room.props);
    }
  });
});
