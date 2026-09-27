import { describe, expect, it } from "vitest";

import { seedFor } from "../core/seed";
import {
  CANNED_BRIDGE,
  CANNED_HUB,
  CANNED_WORKSHOP,
  galleryRoom,
} from "./canned";
import { measureDensity, type Density } from "./density";
import curiosSource from "./curios.ts?raw";
import densitySource from "./density.ts?raw";
import {
  PROP_ORDER,
  SCREEN_GAP,
  WALL_GAP,
  NO_RARE,
  capProps,
  deskSides,
  dressCandidates,
  dressRoom,
  markedVariant,
  rareDraws,
  type Candidate,
  type RareDraws,
} from "./dress";
import dressSource from "./dress.ts?raw";
import {
  FOOTPRINTS,
  MAX_FLOOR_PROP,
  decorFootprint,
  footprint,
  footprintOf,
  heroBlocker,
  heroFootprint,
  pipeRunBox,
  propFootprint,
} from "./footprints";
import { generateRoom } from "./generate";
import generateSource from "./generate.ts?raw";
import { HERO_CLEAR, heroReserve, placeHeroes } from "./heroes";
import heroesSource from "./heroes.ts?raw";
import { REACH, wallPoint } from "./interact";
import { lampBoxes } from "./lamps";
import { doorwayColumns, isFloor, wallSlots } from "./layout";
import { PLAYER_RADIUS, blockersFor } from "./move";
import {
  CLUSTER_BLOCK,
  CLUSTER_CLEAR,
  CLUSTER_INNER,
  CLUSTER_MAX,
  CLUSTER_MIN,
  DESK_MACHINES,
  EXTRAS,
  FILLER,
  LANE_DEPTH,
  LANE_WIDTH,
  MARK_SHARE,
  PALETTES,
  PROP_CATALOGUE,
  PROP_CAP,
  PROP_KINDS,
  RARE_PROP_KINDS,
  RARE_SHARES,
  SPAN_CELLS,
  TOWER_DESK_GAP,
  USE_LANE_DEPTH,
} from "./props";
import {
  DEGENERATE_PLACES,
  OVER_CAP,
  arrivalPoint,
  distanceTo,
  inbound,
  place,
  reachedTargets,
  rel,
  sections,
} from "./reachChecks";
import {
  NO_RESERVE,
  dressingSites,
  edgeKey,
  fitsFloor,
  interiorBand,
  isLargeHall,
  overlaps,
  spanBox,
  turnForSide,
  wallAnchor,
  type DressingSites,
  type Reserved,
  type RoomBase,
} from "./sites";
import type {
  Archetype,
  Box,
  Condition,
  Fixture,
  Hero,
  PlaceInput,
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

const overCap = generateRoom(OVER_CAP);

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

/** The floor props' boxes and the blocking heroes' boxes of a room. */
const solidsOf = (r: RoomSpec): Box[] => [
  ...floorProps(r).map(boxOf),
  ...r.heroes.map(heroBlocker).filter((b) => b !== null),
];

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
 * fits the floor and overlaps no lane, no fixture, decor or scaffold box,
 * no box its heroes reserve (`heroReserve`) and no other floor prop; no
 * cell under it is a doorway cell, next to one (ruling 10) or in the
 * corridor; no two floor props share a cell; and no floor prop on a
 * wall-side spot stands on a cell whose wall edge carries a keep-clear
 * wall prop (D2 as amended: corner-zone spots ignore wall props).
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
  const reserved = heroReserve(room.heroes).boxes;
  for (const [i, box] of boxes.entries()) {
    const label = `${name} ${JSON.stringify(box)}`;
    expect(fitsFloor(room, box), label).toBe(true);
    for (const r of reserved) expect(overlaps(box, r), label).toBe(false);
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
  const kinds = new Map(
    wallProps(room).map((p) => [edgeKey(edgeOf(p)), p.kind] as const),
  );
  const wallSide = new Set(
    sites.wallSide.map((s) => `${String(s.cx)},${String(s.cy)}`),
  );
  for (const p of floorProps(room)) {
    const cx = Math.floor(p.x);
    const cy = Math.floor(p.y);
    if (!wallSide.has(`${String(cx)},${String(cy)}`)) continue;
    for (const side of ["n", "e", "s", "w"] as const) {
      const kind = kinds.get(edgeKey({ x: cx, y: cy, side }));
      expect(
        kind !== undefined && PROP_CATALOGUE[kind].keepClear,
        `${name} ${p.kind} at ${String(cx)},${String(cy)} before ${String(kind)}`,
      ).toBe(false);
    }
  }
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
  }, 20_000);

  it("leave the player's circle at every arrival point clear", () => {
    let checked = 0;
    for (const { name, room } of [...ALL, ...REACH_EXTRA]) {
      const boxes = solidsOf(room);
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

  it("keeps a terminal's and a machine's use range and the player's circle there inside its lane", () => {
    // focusOf offers a fixture within REACH of its wall point, so the whole
    // usable depth along the lane's axis lies inside it (E6).
    expect(REACH).toBeLessThanOrEqual(USE_LANE_DEPTH);
    const outs = [
      FOOTPRINTS.terminal.out,
      ...Object.values(FOOTPRINTS.machine).map((m) => m.out),
    ];
    for (const out of outs)
      expect(out + PLAYER_RADIUS + 0.1 + PLAYER_RADIUS).toBeLessThanOrEqual(
        USE_LANE_DEPTH,
      );
  });

  it("leave the player's circle at every terminal and machine use point clear", () => {
    let checked = 0;
    for (const { name, room } of [...ALL, ...BRIDGES, ...REACH_EXTRA]) {
      const boxes = solidsOf(room);
      for (const f of room.fixtures) {
        if (f.kind !== "terminal" && f.kind !== "machine") continue;
        const size =
          f.kind === "terminal"
            ? FOOTPRINTS.terminal
            : FOOTPRINTS.machine[f.machine];
        const w = wallPoint(f.slot);
        const d = size.out + PLAYER_RADIUS + 0.1;
        const x = w.x + w.inward[0] * d;
        const z = w.z + w.inward[1] * d;
        for (const b of boxes)
          expect(
            distanceTo(x, z, b),
            `${name} ${f.kind} ${edgeKey(f.slot)}`,
          ).toBeGreaterThanOrEqual(PLAYER_RADIUS);
        checked++;
      }
    }
    expect(checked).toBeGreaterThan(200);
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

  it("backs a wall-side prop WALL_GAP off a bare wall and SCREEN_GAP off a screenable wall prop, on all four walls", () => {
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
    const screenSides = new Set<Side>();
    let bare = 0;
    let screening = 0;
    for (const room of rooms) {
      const spots = new Map(
        dressingSites(room).wallSide.map((s) => [`${s.cx},${s.cy}`, s]),
      );
      const kinds = new Map(
        wallProps(room).map((p) => [edgeKey(edgeOf(p)), p.kind] as const),
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
        const kind = kinds.get(edgeKey({ x: cx, y: cy, side }));
        if (p.kind === "marked-crate") {
          // The mark shifts a wall-side crate back along its turn by half
          // the depth it loses, so its back stands at the same gap off its
          // wall the crate it replaced did, not merely at or past it (2.6d
          // C12 fix). Its box still lies inside the crate's.
          expect(gap).toBeCloseTo(
            kind === undefined ? WALL_GAP : SCREEN_GAP,
            2,
          );
          if (kind !== undefined)
            expect(PROP_CATALOGUE[kind].keepClear).toBe(false);
          continue;
        }
        if (kind !== undefined) {
          expect(gap).toBeCloseTo(SCREEN_GAP, 2);
          expect(PROP_CATALOGUE[kind].keepClear).toBe(false);
          expect(PROP_CATALOGUE[p.kind].wallBacked).toBe(false);
          screenSides.add(side);
          screening++;
        } else {
          expect(gap).toBeCloseTo(WALL_GAP, 2);
          bare++;
        }
      }
    }
    expect([...sides].sort()).toEqual(["e", "n", "s", "w"]);
    expect(bare).toBeGreaterThan(0);
    expect(screening).toBeGreaterThanOrEqual(20);
    expect([...screenSides].sort()).toEqual(["e", "n", "s", "w"]);
  });

  it("keeps floor props out of a reserved box", () => {
    const props = floorProps(workshop);
    const first = props[0];
    if (first === undefined) throw new Error("the workshop has floor props");
    const box = boxOf(first);
    const again = dressRoom(workshop, {
      boxes: [box],
      edges: new Set<string>(),
    });
    const floors = again.filter((p) => p.anchor === "floor");
    expect(floors.length).toBeGreaterThan(0);
    for (const p of floors) expect(overlaps(boxOf(p), box)).toBe(false);
    expect(dressRoom(workshop)).toEqual(workshop.props);
  });
});

/** The first cell of the span segment `p` hangs over, and its axis. */
function spanCellOf(p: Prop): {
  axis: "x" | "y";
  cell: { x: number; y: number };
} {
  return p.turn === 0
    ? { axis: "x", cell: { x: p.x - SPAN_CELLS / 2, y: p.y - 0.5 } }
    : { axis: "y", cell: { x: p.x - 0.5, y: p.y - SPAN_CELLS / 2 } };
}

describe("reserved boxes and edges", () => {
  it("dresses every room exactly as before when nothing is reserved", () => {
    for (const { name, room } of [...ALL, ...BRIDGES]) {
      expect(dressRoom(room), name).toEqual(room.props);
      expect(dressRoom(room, NO_RESERVE), name).toEqual(room.props);
    }
  }, 20_000);

  it("drops a span line crossing a reserved box before the pick, and still hangs another", () => {
    const spans = workshop.props.filter((p) => PROP_CATALOGUE[p.kind].span);
    const first = spans[0];
    if (first === undefined) throw new Error("the workshop hangs a span");
    expect(dressingSites(workshop).spanLines.length).toBeGreaterThan(1);
    const { axis, cell } = spanCellOf(first);
    const box = spanBox(axis, cell);
    const again = dressRoom(workshop, {
      boxes: [box],
      edges: new Set<string>(),
    });
    const segments = again.filter((p) => PROP_CATALOGUE[p.kind].span);
    expect(segments.length).toBeGreaterThan(0);
    for (const s of segments) {
      const c = spanCellOf(s);
      expect(overlaps(spanBox(c.axis, c.cell), box)).toBe(false);
    }
  });

  it("keeps wall props, runs, loops and loose cables off reserved edges, mandatory ones too", () => {
    let runsBefore = 0;
    for (const { name, room } of WORKSHOPS) {
      const sites = dressingSites(room);
      // Only free edges, as a hero only ever takes those: the entrance edge
      // (the beacon's) and its neighbours are in noRun, never reserved.
      const edges = new Set(
        sites.longWalls
          .flat()
          .map(edgeKey)
          .filter((k) => sites.free.has(k)),
      );
      const reserved: Reserved = { boxes: [], edges };
      runsBefore += room.props.filter(isRun).length;
      const again = dressRoom(room, reserved);
      for (const p of again) {
        if (p.anchor === "floor" || PROP_CATALOGUE[p.kind].span) continue;
        expect(edges.has(edgeKey(edgeOf(p))), `${name} ${p.kind}`).toBe(false);
      }
      // Runs and their loops only ever hang on the long walls.
      expect(again.filter(isRun), name).toEqual([]);
      expect(
        again.filter((p) => p.kind === "cable-loop"),
        name,
      ).toEqual([]);
    }
    expect(runsBefore).toBeGreaterThan(0);
  });

  it("keeps the dim room's loose cables off reserved edges and still hangs them elsewhere", () => {
    const dim = WORKSHOPS.find((w) => w.condition === "dim");
    if (dim === undefined) throw new Error("no dim workshop");
    const loose = dim.room.props.filter((p) => p.kind === "loose-cable");
    expect(loose.length).toBeGreaterThan(0);
    const edges = new Set(loose.map((p) => edgeKey(edgeOf(p))));
    const again = dressRoom(dim.room, { boxes: [], edges });
    const moved = again.filter((p) => p.kind === "loose-cable");
    expect(moved.length).toBe(loose.length);
    for (const p of moved) expect(edges.has(edgeKey(edgeOf(p)))).toBe(false);
  });

  it("keeps floor props, wall props and spans off a hero's reserve, read from the room itself", () => {
    const h: Hero = {
      kind: "helper-robot",
      variant: 0,
      x: workshop.hall.x0 + 6.5,
      y: workshop.hall.y0 + 6,
      turn: 0,
      seed: 1,
    };
    const room = { ...workshop, heroes: [h] };
    const reserve = heroReserve([h]);
    const props = dressRoom(room);
    expect(props).toEqual(dressRoom(workshop, reserve));
    for (const p of props.filter((q) => q.anchor === "floor"))
      for (const b of reserve.boxes) expect(overlaps(boxOf(p), b)).toBe(false);
  });
});

/**
 * 60 workshop rooms in every archetype, the conditions cycling: the rooms
 * the density measure's floors were forecast on (the plan's Baselines).
 */
const SEEDS: RoomSpec[] = Array.from({ length: 60 }, (_, i) => i).flatMap((i) =>
  Object.values(ARCHETYPE_TYPES).map((type) =>
    generateRoom({
      ...CANNED_WORKSHOP,
      type,
      status: Object.values(STATUS)[i % 4] ?? "stable",
      permalink: `w-${String(i)}`,
    }),
  ),
);
const sum = (rooms: RoomSpec[], f: (d: Density) => number) =>
  rooms.reduce((a, r) => a + f(measureDensity(r)), 0);

/**
 * Every 16th of `SEEDS`, 19 rooms: `SEEDS` cycles the five types within a
 * permalink and the four statuses across permalinks, so a step of 16 walks
 * every archetype and every condition, where a step of 15 would give the
 * bridge type only. With `OVER_CAP` they are the extra rooms the flood fill
 * and the arrival circles are held on.
 */
const REACH_SEEDS = SEEDS.flatMap((room, k) =>
  k % 16 === 0 ? [{ name: `seed ${String(k)}`, room }] : [],
);
const REACH_EXTRA = [{ name: "over cap", room: overCap }, ...REACH_SEEDS];

describe("reachability (Review Focus 1)", () => {
  it("loses a target when a bay doorway is blocked by hand", () => {
    const hub = HUBS[0]?.room;
    if (hub === undefined) throw new Error("no hub");
    const bare = { ...hub, props: [], heroes: [] };
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

  const expectSameTargets = (name: string, room: RoomSpec) => {
    const bare = { ...room, props: [], heroes: [] };
    const without = reachedTargets(bare, blockersFor(bare));
    expect(without.size, name).toBeGreaterThan(0);
    const withProps = reachedTargets(room, blockersFor(room));
    for (const t of without)
      expect(withProps.has(t), `${name} ${t}`).toBe(true);
  };

  for (const { name, room } of [...ALL, ...BRIDGES])
    it(`reaches with props every target ${name} reaches without them`, () => {
      expectSameTargets(name, room);
    }, 20_000);

  // The fullest room and the seeds, where the mid-hall clusters stand among
  // the most fixtures and decor: a cluster narrows a way but never seals it.
  for (const { name, room } of REACH_EXTRA)
    it(`reaches with props every target ${name} reaches without them`, () => {
      expectSameTargets(name, room);
    }, 30_000);
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
      // A zone spot ignores wall props, keep-clear ones included (D2 as
      // amended), so only the lanes and taken boxes decide.
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

describe("density measure", () => {
  it("covers at least 0.12 of free edges with wide wall props, and at most 0.6", () => {
    // Baseline 0.037 (locker banks, and the padded panel once it was
    // widened), forecast 0.17.
    const share =
      sum(SEEDS, (d) => d.wideEdges) / sum(SEEDS, (d) => d.freeEdges);
    expect(share).toBeGreaterThanOrEqual(0.12);
    expect(share).toBeLessThanOrEqual(0.6);
  });

  it("puts at least 0.15 floor props per wall-side spot, and at most 0.35", () => {
    // 0.128 before iteration 2, forecast 0.175 (E7).
    const per =
      sum(SEEDS, (d) => d.wallSideProps) / sum(SEEDS, (d) => d.wallSideSpots);
    expect(per).toBeGreaterThanOrEqual(0.15);
    expect(per).toBeLessThanOrEqual(0.35);
  });

  it("makes at least 0.25 of the floor props tall", () => {
    // 0.060 on the 13x12 seeds before Task 2, 0.188 after it; forecast 0.327
    // once the clusters draw tall stacks (E9).
    expect(
      sum(SEEDS, (d) => d.tallProps) / sum(SEEDS, (d) => d.floorProps),
    ).toBeGreaterThanOrEqual(0.25);
  });

  it("keeps every large dressed room at or under 22 floor props per 100 floor cells", () => {
    // The densest room measured with a cluster in every block is at 19.87,
    // the workshop (permalink pipe-shop) engineering derelict (E9); the
    // ceiling was 16 before the clusters grew.
    const rooms = [
      ...ALL,
      ...BRIDGES,
      ...SEEDS.map((room, k) => ({ name: `seed ${String(k)}`, room })),
      ...REACH_EXTRA,
    ];
    let large = 0;
    for (const { name, room } of rooms) {
      const d = measureDensity(room);
      if (!d.large) continue;
      expect(d.floorPer100, name).toBeLessThanOrEqual(22);
      large++;
    }
    // Every workshop, hub, seed and the over-cap room is large; no bridge is.
    expect(large).toBe(ALL.length + SEEDS.length + REACH_EXTRA.length);
  });

  it("raises large workshops to at least 13 floor props per 100 floor cells", () => {
    // Baseline 4.70 before iteration 1, 7.39 after it, 8.81 with the 13x12
    // workshop; 15.97 with a cluster in every block (E9), 15.58 with the
    // heroes the seeds draw (190 of 300 rooms carry one, 2.6a).
    const per100 =
      (sum(SEEDS, (d) => d.floorProps) * 100) / sum(SEEDS, (d) => d.floorCells);
    expect(per100).toBeGreaterThanOrEqual(13);
  });

  it("raises the canned hub to at least 13 floor props per 100 floor cells in every archetype", () => {
    // Baseline 2.1 to 2.4, 7.7 to 8.9 before the clusters grew; 15.77 to
    // 17.19 (E9), 15.63 to 17.05 with the hoverboard the hub's own seed
    // draws (2.6c); reseeded with the heroes they draw, the clean hub's
    // lowest is 14.77 (2.6a).
    for (const { name, condition, room } of HUBS)
      if (condition === "clean")
        expect(measureDensity(room).floorPer100, name).toBeGreaterThanOrEqual(
          13,
        );
  });
});

/** A box grown by `m` metres on every side, as `dress.ts` grows a cluster ring. */
const grow = (b: Box, m: number): Box => ({
  x0: b.x0 - m,
  x1: b.x1 + m,
  z0: b.z0 - m,
  z1: b.z1 + m,
});

describe("mid-hall clusters", () => {
  const LARGE = [
    ...ALL,
    ...SEEDS.slice(0, 40).map((room, i) => ({
      name: `seed ${String(i)}`,
      room,
    })),
  ].filter(({ room }) => isLargeHall(room.hall));

  it("keep every member inside a block's inner cells, 1.0 m clear of everything outside its cluster", () => {
    let members = 0;
    for (const { name, room } of LARGE) {
      const band = interiorBand(room.hall);
      if (band === null) throw new Error("large hall without a band");
      const blockOf = (p: Prop) => {
        const bx =
          band.x0 +
          CLUSTER_BLOCK *
            Math.floor((Math.floor(p.x) - band.x0) / CLUSTER_BLOCK);
        const by =
          band.y0 +
          CLUSTER_BLOCK *
            Math.floor((Math.floor(p.y) - band.y0) / CLUSTER_BLOCK);
        return `${String(bx)},${String(by)}`;
      };
      const inBand = floorProps(room).filter(
        (p) =>
          p.x >= band.x0 && p.x < band.x1 && p.y >= band.y0 && p.y < band.y1,
      );
      const inner = new Set(
        dressingSites(room).clusterBlocks.flatMap((b) =>
          b.cells.map((c) => `${String(c.cx)},${String(c.cy)}`),
        ),
      );
      const taken = takenBoxes(room);
      const floor = floorProps(room);
      for (const p of inBand) {
        expect(
          inner.has(`${String(Math.floor(p.x))},${String(Math.floor(p.y))}`),
          name,
        ).toBe(true);
        const ring = grow(boxOf(p), CLUSTER_CLEAR - 1e-9);
        for (const t of taken)
          expect(overlaps(ring, t), `${name} ${p.kind}`).toBe(false);
        for (const q of floor)
          if (q !== p && blockOf(q) !== blockOf(p))
            expect(overlaps(ring, boxOf(q)), `${name} ${p.kind}`).toBe(false);
        members++;
      }
      const sizes = new Map<string, number>();
      for (const p of inBand)
        sizes.set(blockOf(p), (sizes.get(blockOf(p)) ?? 0) + 1);
      for (const n of sizes.values())
        expect(n, name).toBeLessThanOrEqual(CLUSTER_MAX);
      expect(
        sizes.size === 0 ? 0 : Math.max(...sizes.values()),
        name,
      ).toBeGreaterThanOrEqual(CLUSTER_MIN);
    }
    // 2815 members measured over these rooms.
    expect(members).toBeGreaterThan(2000);
  }, 20_000);

  it("keeps two clusters at least 2 m + twice the widest footprint's inset apart", () => {
    // E4: inner cells of neighbouring blocks are one empty cell apart, and a
    // member stays (CELL - MAX_FLOOR_PROP) / 2 inside its cell.
    const gap = CELL + (CELL - MAX_FLOOR_PROP);
    expect(gap).toBeGreaterThan(2 * CLUSTER_CLEAR);
  });

  it("uses only the palette's cluster kinds in the band", () => {
    for (const { name, room } of LARGE) {
      const band = interiorBand(room.hall);
      if (band === null) continue;
      const kinds = new Set<string>(
        PALETTES[room.archetype].cluster.map(([k]) => k),
      );
      for (const p of floorProps(room))
        if (p.x >= band.x0 && p.x < band.x1 && p.y >= band.y0 && p.y < band.y1)
          // The mark relabels one accepted crate or crate stack, a cluster
          // member among them, as a marked crate (2.6d C12).
          expect(
            kinds.has(p.kind) ||
              (p.kind === "marked-crate" &&
                (kinds.has("crate") || kinds.has("crate-stack"))),
            `${name} ${p.kind}`,
          ).toBe(true);
    }
  });

  it("puts none in a hall that is not large", () => {
    for (const { name, room } of BRIDGES)
      expect(measureDensity(room).bandProps, name).toBe(0);
  });
});

describe("ceiling spans", () => {
  it("hang one straight line of the palette's span in some large halls only", () => {
    let eligible = 0;
    let withSpan = 0;
    for (const room of [
      ...SEEDS,
      ...HUBS.map((h) => h.room),
      ...BRIDGES.map((b) => b.room),
    ]) {
      const spans = room.props.filter((p) => PROP_CATALOGUE[p.kind].span);
      const kind = PALETTES[room.archetype].ceilingSpan;
      if (kind === null || !isLargeHall(room.hall)) {
        expect(spans).toEqual([]);
        continue;
      }
      eligible++;
      if (spans.length === 0) continue;
      withSpan++;
      for (const p of spans) expect(p.kind).toBe(kind);
      expect(new Set(spans.map((p) => p.variant)).size).toBe(1);
      expect(new Set(spans.map((p) => p.turn)).size).toBe(1);
      const turn = spans[0]?.turn;
      // One line: every segment on the same row (turn 0) or column.
      expect(new Set(spans.map((p) => (turn === 0 ? p.y : p.x))).size).toBe(1);
    }
    expect(eligible).toBeGreaterThan(50);
    expect(withSpan / eligible).toBeGreaterThanOrEqual(0.5);
    expect(withSpan / eligible).toBeLessThanOrEqual(0.8);
  });

  it("keeps every span box clear of the ceiling band along the walls, lamps, decor, pipe runs and scaffolding", () => {
    let checked = 0;
    for (const room of [...SEEDS.slice(0, 60), ...HUBS.map((h) => h.room)]) {
      const h = room.hall;
      const solid = [
        ...room.decor.map(decorFootprint).filter((b) => b !== null),
        ...room.decor.flatMap((d) => {
          const b = pipeRunBox(d, room.hall);
          return b === null ? [] : [b];
        }),
        ...room.scaffold,
        ...lampBoxes(room),
      ];
      for (const p of room.props.filter((q) => PROP_CATALOGUE[q.kind].span)) {
        checked++;
        const axis = p.turn === 0 ? "x" : "y";
        const first =
          axis === "x"
            ? { x: p.x - SPAN_CELLS / 2, y: p.y - 0.5 }
            : { x: p.x - 0.5, y: p.y - SPAN_CELLS / 2 };
        const b = spanBox(axis, first);
        // CEILING_OUT (1.2 m) along every wall, plus 0.5 m.
        const off = Math.min(
          b.x0 - h.x0 * CELL,
          h.x1 * CELL - b.x1,
          b.z0 - h.y0 * CELL,
          h.y1 * CELL - b.z1,
        );
        expect(off).toBeGreaterThanOrEqual(1.7);
        for (const s of solid) expect(overlaps(b, s)).toBe(false);
      }
    }
    expect(checked).toBeGreaterThan(0);
  });

  it("hangs a span in the demo workshop in every archetype but the council", () => {
    for (const { name, archetype, room } of WORKSHOPS) {
      const segments = measureDensity(room).spanSegments;
      if (archetype === "council") expect(segments, name).toBe(0);
      else expect(segments, name).toBeGreaterThan(0);
    }
  });
});

describe("mandatory props", () => {
  it("gives the workshop the hall it was made for", () => {
    expect(workshop.hall).toEqual({ x0: 0, y0: 0, x1: 13, y1: 12 });
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
    expect(count("machine")).toBe(5);
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

  it("keeps the canned hub's candidates at or under PROP_CAP in every archetype and condition", () => {
    // D8: the cap never drops a hub's ceiling tier. 211 to 246 with the
    // hoverboard the hub's own seed draws (2.6c); over about 1000 reseeds
    // that draw a hero, a hero only ever removed candidates (at most 10),
    // never added one (2.6a).
    for (const { name, room } of HUBS)
      expect(dressCandidates(room).length, name).toBeLessThanOrEqual(PROP_CAP);
  });

  it("caps a room that really overflows by the tier rules", () => {
    const room = overCap;
    expect(room.hall).toEqual({ x0: 26, y0: 0, x1: 50, y1: 24 });
    expect(room.bays).toHaveLength(4);
    expect(room.corridor).not.toBeNull();
    const candidates = dressCandidates(room);
    expect(candidates.length).toBeGreaterThan(PROP_CAP);
    const line = candidates.filter(
      (c) => PROP_CATALOGUE[c.prop.kind].span,
    ).length;
    expect(room.props.length).toBeLessThanOrEqual(PROP_CAP);
    // OVER_CAP draws a span line (11 segments), and the cap keeps it whole
    // or drops it whole (E10); dropping the whole line can cost up to
    // line - 1 props, so the length is pinned within that of PROP_CAP.
    expect(line).toBeGreaterThan(1);
    const spansKept = room.props.filter(
      (p) => PROP_CATALOGUE[p.kind].span,
    ).length;
    expect([0, line]).toContain(spansKept);
    expect(room.props.length).toBeGreaterThan(PROP_CAP - line);
    const kept = capProps(candidates, PROP_CAP);
    expect(room.props).toEqual([...kept].sort(PROP_ORDER));
    expectTiers("over cap", candidates, kept);
    // No wall candidate is dropped: the lower tiers hold enough to cover
    // the overflow on their own.
    expect(
      candidates.filter((c) => c.prop.anchor !== "wall").length,
    ).toBeGreaterThanOrEqual(candidates.length - PROP_CAP);
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

  it("keeps a span line whole or drops it whole, however tight the cap", () => {
    let dropped = 0;
    let checked = 0;
    for (const { name, room } of HUBS) {
      const candidates = dressCandidates(room);
      const line = candidates.filter(
        (c) => PROP_CATALOGUE[c.prop.kind].span,
      ).length;
      if (line === 0) continue;
      const ceiling = candidates.filter(
        (c) => c.prop.anchor === "ceiling",
      ).length;
      for (
        let cap = candidates.length - ceiling;
        cap < candidates.length;
        cap++
      ) {
        const kept = capProps(candidates, cap);
        expect(kept.length, `${name} cap ${String(cap)}`).toBeLessThanOrEqual(
          cap,
        );
        expect(kept.length, `${name} cap ${String(cap)}`).toBeGreaterThan(
          cap - line,
        );
        const spans = kept.filter((p) => PROP_CATALOGUE[p.kind].span).length;
        expect([0, line], `${name} cap ${String(cap)}`).toContain(spans);
        if (spans === 0) dropped++;
        checked++;
      }
    }
    expect(checked).toBeGreaterThan(100);
    expect(dropped).toBeGreaterThan(0);
  });
});

/** A room, the same room with one more door (a relation to "z"), and that door. */
interface LocalityCase {
  name: string;
  before: RoomSpec;
  after: RoomSpec;
  door: Fixture;
}

function localityCase(name: string, p: PlaceInput): LocalityCase {
  const before = generateRoom(p);
  const after = generateRoom({ ...p, relations: [...p.relations, rel("z")] });
  const door = after.fixtures.find(
    (f) => f.kind === "door" && f.address?.permalink === "z",
  );
  if (door === undefined) throw new Error(`${name}: no new door`);
  return { name, before, after, door };
}

/** A 13-wide hall that is not large enough to lose its door's locality to the cap. */
const SMALL_LOCALITY = localityCase(
  "runbook",
  place({
    type: "runbook",
    status: "stable",
    relations: [rel("a"), rel("b")],
    inbound: inbound(5),
    inboundTotal: 5,
    content: sections(2),
    tags: ["t-1", "t-2"],
  }),
);

/** A 13 by 10 large hall in four archetypes (see the large-hall locality test). */
const LARGE_LOCALITY = ["manifest", "runbook", "reference", "guide"].map(
  (type) =>
    localityCase(
      type,
      place({
        type,
        status: "stable",
        relations: [rel("a"), rel("b")],
        inbound: inbound(5),
        inboundTotal: 5,
        content: sections(4),
        tags: ["t-1", "t-2"],
      }),
    ),
);

/**
 * The locality cases under 10 room seeds each, the same seed on both
 * sides, so some of them carry heroes: what the hero locality test walks.
 */
const LOCALITY_CASES: LocalityCase[] = [
  SMALL_LOCALITY,
  ...LARGE_LOCALITY,
].flatMap((c) =>
  Array.from({ length: 10 }, (_, i) => {
    const seed = seedFor("hero-locality", i);
    return {
      ...c,
      name: `${c.name} seed ${String(i)}`,
      before: { ...c.before, seed },
      after: { ...c.after, seed },
    };
  }),
);

/**
 * A room dressed with no heroes: the dressing's locality is pinned apart
 * from the heroes', which a new door can move when its lane crosses one.
 */
function withoutHeroes(room: RoomSpec): RoomSpec {
  const bare = { ...room, heroes: [] };
  return { ...bare, props: dressRoom(bare) };
}

describe("locality", () => {
  // Locality holds only below the cap: near PROP_CAP a new fixture can
  // change which props are dropped anywhere in the room, so both rooms here
  // are asserted to stay under it.
  it("changes only props near a new door's slot", () => {
    const a = withoutHeroes(SMALL_LOCALITY.before);
    const b = withoutHeroes(SMALL_LOCALITY.after);
    expect(a.hall).toEqual(b.hall);
    expect(a.hall.x1 - a.hall.x0).toBe(13);
    const door = SMALL_LOCALITY.door;
    const slot = door.slot;
    expect(a.fixtures.some((f) => edgeKey(f.slot) === edgeKey(slot))).toBe(
      false,
    );
    expect(wallSlots(a.grid).map(edgeKey)).toContain(edgeKey(slot));
    expect(dressCandidates(a).length).toBeLessThanOrEqual(PROP_CAP);
    expect(dressCandidates(b).length).toBeLessThanOrEqual(PROP_CAP);

    const cellOf = (p: Prop) => {
      if (p.anchor === "floor") return [Math.floor(p.x), Math.floor(p.y)];
      // A span maps to its segment's first cell, not to a wall edge.
      if (PROP_CATALOGUE[p.kind].span)
        return [
          Math.floor(p.x - (p.turn === 0 ? SPAN_CELLS / 2 : 0.5)),
          Math.floor(p.y - (p.turn === 0 ? 0.5 : SPAN_CELLS / 2)),
        ];
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

  it("changes only props near a new door in a large hall, leaving its clusters and span line alone", () => {
    // A 13 by 10 hall: 4 sections, 2 relations, 5 inbound references and 2
    // tags, and a third relation adds one door at (5,0,n) without moving the
    // hall or any other fixture. The canned rooms do not serve here: a
    // relation grows the workshop's hall, and reshuffles the hub's slots.
    // Probe note (planner, 2026-09-25, not re-run here): with a cluster in
    // every block, the 3x3 inner squares and the engineering span, over 288
    // clean pairs (4 span archetypes, 3 to 6 sections, 1 to 3 relations, 3
    // to 5 inbound, 0 or 2 tags) every changed prop lay within 1 cell of the
    // new slot, and no cluster member or span prop changed. This test pins
    // the radius on the four cases below. Under the other conditions the
    // extras move further, since each goes to the first spot in floor-seed
    // order that accepts it (ruling 12), so this case is clean.
    const RADIUS = 1;
    let members = 0;
    for (const c of LARGE_LOCALITY) {
      const a = withoutHeroes(c.before);
      const b = withoutHeroes(c.after);
      const name = `${c.name} (${a.archetype})`;
      expect(a.hall, name).toEqual(b.hall);
      expect(a.hall, name).toEqual({ x0: 0, y0: 0, x1: 13, y1: 10 });
      expect(isLargeHall(a.hall), name).toBe(true);
      const door = c.door;
      const slot = door.slot;
      const fk = (f: unknown) => JSON.stringify(f);
      const bf = new Set(b.fixtures.map(fk));
      for (const f of a.fixtures) expect(bf.has(fk(f)), name).toBe(true);
      expect(b.fixtures, name).toHaveLength(a.fixtures.length + 1);
      expect(dressCandidates(a).length, name).toBeLessThan(PROP_CAP);
      expect(dressCandidates(b).length, name).toBeLessThan(PROP_CAP);

      const sites = dressingSites(a);
      const blockOf = (p: Prop) =>
        p.anchor === "floor"
          ? sites.clusterBlocks.find((bl) =>
              bl.cells.some(
                (c) => c.cx === Math.floor(p.x) && c.cy === Math.floor(p.y),
              ),
            )
          : undefined;
      const inClusters = (r: RoomSpec) =>
        r.props.filter((p) => blockOf(p) !== undefined).length;
      expect(inClusters(a), name).toBeGreaterThan(0);
      expect(inClusters(b), name).toBeGreaterThan(0);
      members += inClusters(a);

      const spans = (r: RoomSpec) =>
        r.props.filter((p) => PROP_CATALOGUE[p.kind].span);
      expect(spans(a).length, name).toBeGreaterThan(0);
      expect(spans(b), name).toEqual(spans(a));
      expect(dressingSites(b).spanLines, name).toEqual(sites.spanLines);

      // What a block can reach: its inner CLUSTER_INNER by CLUSTER_INNER
      // cells grown by the ring. A member may change only when that reaches
      // the new door or its lane.
      const near = [
        footprint(slot, { along: LANE_WIDTH, out: LANE_DEPTH }),
        footprintOf(door),
      ].filter((x) => x !== null);
      const reaches = (p: Prop) => {
        const bl = blockOf(p);
        if (bl === undefined) return false;
        const inner = grow(
          {
            x0: (bl.x + 1) * CELL,
            x1: (bl.x + 1 + CLUSTER_INNER) * CELL,
            z0: (bl.y + 1) * CELL,
            z1: (bl.y + 1 + CLUSTER_INNER) * CELL,
          },
          CLUSTER_CLEAR,
        );
        return near.some((n) => overlaps(inner, n));
      };
      const cellOf = (p: Prop) => {
        if (p.anchor === "floor") return [Math.floor(p.x), Math.floor(p.y)];
        const e = edgeOf(p);
        return [e.x, e.y];
      };
      const sa = new Set(a.props.map(fk));
      const sb = new Set(b.props.map(fk));
      const diff = [
        ...[...sa].filter((k) => !sb.has(k)),
        ...[...sb].filter((k) => !sa.has(k)),
      ];
      expect(diff.length, name).toBeGreaterThan(0);
      for (const k of diff) {
        const p = JSON.parse(k) as Prop;
        const [x, y] = cellOf(p) as [number, number];
        const d = Math.max(Math.abs(x - slot.x), Math.abs(y - slot.y));
        expect(d <= RADIUS || reaches(p), `${name} ${k}`).toBe(true);
      }
    }
    expect(members).toBeGreaterThan(0);
  });

  it("keeps the heroes where they were when a door is added, unless the door's lane crosses one", () => {
    let kept = 0;
    for (const { name, before, after, door } of LOCALITY_CASES) {
      const lane = footprint(door.slot, { along: LANE_WIDTH, out: LANE_DEPTH });
      const heroes = placeHeroes(before);
      const moved = heroes.some((h) => {
        const b = heroFootprint(h);
        return overlaps(
          {
            x0: b.x0 - HERO_CLEAR,
            x1: b.x1 + HERO_CLEAR,
            z0: b.z0 - HERO_CLEAR,
            z1: b.z1 + HERO_CLEAR,
          },
          lane,
        );
      });
      if (moved) continue;
      expect(placeHeroes(after), name).toEqual(heroes);
      if (heroes.length > 0) kept++;
    }
    expect(kept).toBeGreaterThan(0);
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
        const room = generateRoom({ ...DEGENERATE_PLACES.empty, type, status });
        expect(room.hall).toEqual({ x0: 0, y0: 0, x1: 5, y1: 6 });
        const name = `empty ${type} ${status}`;
        walls += expectWallInvariants(name, room);
        expectFloorInvariants(name, room);
        expect(measureDensity(room).bandProps, name).toBe(0);
        expect(measureDensity(room).spanSegments, name).toBe(0);
      }
    expect(walls).toBeGreaterThan(0);
  });

  it("gives a hall full of fixtures only wall props off the slots, keeping every invariant", () => {
    const full = DEGENERATE_PLACES.full;
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
    expect(measureDensity(room).spanSegments).toBe(0);
  });

  it("dresses a narrow, deep hall, keeping every invariant", () => {
    for (const status of Object.values(STATUS)) {
      const room = generateRoom({ ...DEGENERATE_PLACES.narrow, status });
      expect(room.hall.x1 - room.hall.x0).toBe(5);
      expect(room.hall.y1 - room.hall.y0).toBe(12);
      expect(expectWallInvariants(`narrow ${status}`, room)).toBeGreaterThan(0);
      expectFloorInvariants(`narrow ${status}`, room);
      expect(measureDensity(room).bandProps).toBe(0);
      expect(measureDensity(room).spanSegments).toBe(0);
    }
  });

  it("dresses a hall exactly at the large threshold, one cluster block, keeping every invariant", () => {
    // A guide with 3 sections and 3 inbound references (inboundTotal 3), no
    // tags and no relations: a 9 by 8 hall, the smallest large hall (D5).
    // The generator makes odd widths and even depths, so 8 by 9 never comes
    // out of it. Its band is 5 by 4 cells, which holds one block.
    for (const status of Object.values(STATUS)) {
      const room = generateRoom({ ...DEGENERATE_PLACES.threshold, status });
      const name = `threshold ${status}`;
      expect(room.hall.x1 - room.hall.x0, name).toBe(9);
      expect(room.hall.y1 - room.hall.y0, name).toBe(8);
      expect(isLargeHall(room.hall), name).toBe(true);
      expect(dressingSites(room).clusterBlocks, name).toHaveLength(1);
      expect(expectWallInvariants(name, room)).toBeGreaterThan(0);
      expectFloorInvariants(name, room);
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
  it("keeps dress.ts, density.ts, heroes.ts and curios.ts away from move, generate, interact, malfunction and render", () => {
    for (const source of [
      dressSource,
      densitySource,
      heroesSource,
      curiosSource,
    ])
      expect(source).not.toMatch(
        /\b(?:from|import)\s*\(?\s*["'](?:\.\/(?:move|generate|interact|malfunction)|\.\.\/render(?:\/[^"']*)?)["']/,
      );
  });

  it("keeps generate.ts away from move, interact, malfunction and render", () => {
    expect(generateSource).not.toMatch(
      /\b(?:from|import)\s*\(?\s*["'](?:\.\/(?:move|interact|malfunction)|\.\.\/render(?:\/[^"']*)?)["']/,
    );
  });
});

/** Whether `count` of `n` draws lies within 4 standard deviations of a share `p`. */
const within = (count: number, n: number, p: number) =>
  Math.abs(count - n * p) <= 4 * Math.sqrt(n * p * (1 - p));

describe("rare props (2.6d C9 to C13)", () => {
  const SWEEP = [...ALL, ...BRIDGES];
  const cellOf = (p: Prop) =>
    p.anchor === "floor"
      ? [Math.floor(p.x), Math.floor(p.y)]
      : [edgeOf(p).x, edgeOf(p).y];

  it("keeps every rare kind out of every palette, the filler and the extras (2.6d C10)", () => {
    // Mutation caught: a rare kind added to a palette, where it would
    // reshuffle every weighted pick of every room.
    const rare = new Set<string>(RARE_PROP_KINDS);
    expect(PROP_KINDS.filter((k) => PROP_CATALOGUE[k].rare).sort()).toEqual(
      [...RARE_PROP_KINDS].sort(),
    );
    for (const [a, p] of Object.entries(PALETTES))
      for (const [k] of [...p.wall, ...p.floor, ...p.cluster])
        expect(rare.has(k), `${a} ${k}`).toBe(false);
    for (const [k] of FILLER) expect(rare.has(k), k).toBe(false);
    for (const rules of Object.values(EXTRAS))
      for (const r of rules) expect(rare.has(r.kind), r.kind).toBe(false);
  });

  it("draws each rare kind at its archetype's share (2.6d C9)", () => {
    const n = 4000;
    for (const [a, type] of Object.entries(ARCHETYPE_TYPES)) {
      const arch = a as Archetype;
      const room = generateRoom({ ...CANNED_WORKSHOP, type });
      const c = { poster: 0, canisters: 0, tower: 0, panel: 0, mark: 0 };
      for (let i = 0; i < n; i++) {
        const d = rareDraws({ ...room, seed: seedFor("rare-rate", a, i) });
        if (d.poster) c.poster++;
        if (d.canisters) c.canisters++;
        if (d.tower) c.tower++;
        if (d.panel) c.panel++;
        if (d.mark.take) c.mark++;
      }
      expect(
        within(c.poster, n, RARE_SHARES["saucer-poster"][arch]),
        `${a} poster`,
      ).toBe(true);
      expect(
        within(c.canisters, n, RARE_SHARES["ooze-canisters"][arch]),
        `${a} canisters`,
      ).toBe(true);
      expect(
        within(c.tower, n, RARE_SHARES["designer-tower"][arch]),
        `${a} tower`,
      ).toBe(true);
      expect(
        within(c.panel, n, RARE_SHARES["gravity-console"][arch]),
        `${a} console`,
      ).toBe(true);
      expect(within(c.mark, n, MARK_SHARE[arch]), `${a} mark`).toBe(true);
    }
  });

  it("hangs a forced poster only on a free edge no prop, fixture or hero holds, with nothing standing in front (2.6d C11, Review Focus 3)", () => {
    // Mutation caught: the poster step not checking `used` (it lands on a
    // sign's edge), not keep-clear (a floor prop stands before it), or
    // taking a corner-zone cell's edge.
    // The canned rooms share three seeds between them, so the poster's
    // first edge by seed is nearly the same in every archetype and
    // condition of one place; the reseeded workshops (`SEEDS`) give it
    // sixty more seeds, which is what reaches a corner-zone cell's edge.
    const rooms = [
      ...SWEEP,
      ...SEEDS.map((room, k) => ({
        name: `seed ${String(k)}`,
        condition: room.condition,
        room,
      })),
    ];
    let hung = 0;
    for (const { name, condition, room } of rooms) {
      const plain = dressRoom(room, NO_RESERVE, NO_RARE);
      const props = dressRoom(room, NO_RESERVE, { ...NO_RARE, poster: true });
      const posters = props.filter((p) => p.kind === "saucer-poster");
      expect(posters.length, name).toBeLessThanOrEqual(1);
      const p = posters[0];
      if (p === undefined) {
        expect(props, name).toEqual(plain);
        continue;
      }
      hung++;
      const e = edgeOf(p);
      const k = edgeKey(e);
      const sites = dressingSites(room);
      expect(sites.free.has(k), name).toBe(true);
      expect(heroReserve(room.heroes).edges.has(k), name).toBe(false);
      expect(
        room.fixtures.some((f) => edgeKey(f.slot) === k),
        name,
      ).toBe(false);
      expect(
        sites.zones.some((z) =>
          z.spots.some((s) => s.cx === e.x && s.cy === e.y),
        ),
        name,
      ).toBe(false);
      const sameEdge = props.filter(
        (q) =>
          q !== p &&
          q.anchor === "wall" &&
          !PROP_CATALOGUE[q.kind].run &&
          edgeKey(edgeOf(q)) === k,
      );
      expect(sameEdge, name).toEqual([]);
      expect(
        props.some(
          (q) =>
            q.anchor === "floor" &&
            Math.floor(q.x) === e.x &&
            Math.floor(q.y) === e.y,
        ),
        name,
      ).toBe(false);
      // Everything else is as it was, except on its own edge and in its
      // cell. Only in a clean room: elsewhere a condition extra whose first
      // spot the poster's keep-clear closed moves on to its next spot
      // anywhere in the room (ruling 12, 2.6d C10).
      if (condition !== "clean") continue;
      const key = (q: Prop) => JSON.stringify(q);
      const before = new Set(plain.map(key));
      const after = new Set(props.map(key));
      for (const q of [
        ...plain.filter((x) => !after.has(key(x))),
        ...props.filter((x) => !before.has(key(x))),
      ])
        expect(cellOf(q), `${name} ${q.kind}`).toEqual([e.x, e.y]);
    }
    // Most rooms keep an open edge after their mandatory props. If this
    // bound fails, report the count before changing it.
    expect(hung).toBeGreaterThan(rooms.length / 2);
  });

  it("marks at most one eligible crate, inside the footprint it replaces, and changes nothing else (2.6d C12, Review Focus 4)", () => {
    // Mutation caught: the small crate (variant 0) marked, whose 0.8 m box
    // the 1.0 m mark overhangs, or a second crate marked.
    let marked = 0;
    let shifted = 0;
    for (const { name, room } of SWEEP) {
      const plain = dressRoom(room, NO_RESERVE, NO_RARE);
      const eligible = plain.filter((p) => markedVariant(p) !== null);
      for (const roll of [0, 0.5, 0.999]) {
        const props = dressRoom(room, NO_RESERVE, {
          ...NO_RARE,
          mark: { take: true, roll },
        });
        expect(props.length, name).toBe(plain.length);
        expect(blockersFor({ ...room, props }).length, name).toBe(
          blockersFor({ ...room, props: plain }).length,
        );
        const bySeed = new Map(plain.map((p) => [p.seed, p]));
        const changed = props.filter(
          (p) => JSON.stringify(p) !== JSON.stringify(bySeed.get(p.seed)),
        );
        expect(changed.length, name).toBe(eligible.length > 0 ? 1 : 0);
        const m = changed[0];
        if (m === undefined) continue;
        marked++;
        const was = bySeed.get(m.seed);
        if (was === undefined)
          throw new Error(`${name}: no prop under the mark`);
        expect(m.kind, name).toBe("marked-crate");
        expect(m.variant, name).toBe(markedVariant(was));
        expect(m.turn, name).toBe(was.turn);
        expect(m.seed, name).toBe(was.seed);
        expect(m.anchor, name).toBe(was.anchor);
        const inner = propFootprint(m);
        const outer = propFootprint(was);
        if (inner === null || outer === null)
          throw new Error("floor props have boxes");
        // A wall-side mark shifts its centre back along its turn by half
        // the depth it loses there, so its own back still meets the wall
        // at the gap the crate it replaced stood at, instead of drifting
        // away from it (2.6d C12 fix). The WALL_GAP test already pins
        // that every wall-side floor prop's turn matches its spot's wall
        // (`turnForSide`), so this same cell/side lookup can only find a
        // real wall: a cluster or corner-zone crate's cell is excluded
        // from `wallSide` by construction and never matches here, so it
        // falls to the exact-position check below. A mark whose depth
        // does not shrink on the wall's axis (a crate-stack v1, already
        // as deep as the marked stack) shifts nothing either.
        const spot = dressingSites(room).wallSide.find(
          (s) => s.cx === Math.floor(was.x) && s.cy === Math.floor(was.y),
        );
        const side = spot?.wall ?? null;
        const shrankZ = outer.z1 - outer.z0 > inner.z1 - inner.z0 + 1e-9;
        const shrankX = outer.x1 - outer.x0 > inner.x1 - inner.x0 + 1e-9;
        if (side === "n" && shrankZ) {
          shifted++;
          expect(m.x, name).toBe(was.x);
          expect(inner.z0, name).toBeCloseTo(outer.z0, 6);
        } else if (side === "s" && shrankZ) {
          shifted++;
          expect(m.x, name).toBe(was.x);
          expect(inner.z1, name).toBeCloseTo(outer.z1, 6);
        } else if (side === "w" && shrankX) {
          shifted++;
          expect(m.y, name).toBe(was.y);
          expect(inner.x0, name).toBeCloseTo(outer.x0, 6);
        } else if (side === "e" && shrankX) {
          shifted++;
          expect(m.y, name).toBe(was.y);
          expect(inner.x1, name).toBeCloseTo(outer.x1, 6);
        } else {
          expect({ ...m, kind: was.kind, variant: was.variant }, name).toEqual(
            was,
          );
        }
        expect(
          inner.x0 >= outer.x0 - 1e-9 &&
            inner.x1 <= outer.x1 + 1e-9 &&
            inner.z0 >= outer.z0 - 1e-9 &&
            inner.z1 <= outer.z1 + 1e-9,
          name,
        ).toBe(true);
        expect(was.kind === "crate" ? [1, 2] : [0, 1], name).toContain(
          was.variant,
        );
      }
    }
    expect(marked).toBeGreaterThan(0);
    expect(shifted).toBeGreaterThan(0);
  });

  it("places each rare floor prop only where it fits, and the tower only beside a desk on its wall (2.6d C13, Review Focus 5)", () => {
    // Mutation caught: the tower step not reading `deskSides` (a tower by a
    // bare wall or in a corner), or a rare prop placed without `place`.
    const rooms = [
      ...SWEEP,
      ...Object.entries(DEGENERATE_PLACES).map(([name, p]) => ({
        name,
        room: generateRoom(p),
      })),
    ];
    const isDesk = (f: Fixture) =>
      f.kind === "terminal" ||
      (f.kind === "machine" &&
        (DESK_MACHINES as readonly string[]).includes(f.machine));
    const forced: RareDraws = {
      ...NO_RARE,
      canisters: true,
      tower: true,
      panel: true,
    };
    let towers = 0;
    let canisters = 0;
    let consoles = 0;
    for (const { name, room } of rooms) {
      const props = dressRoom(room, NO_RESERVE, forced);
      expectFloorInvariants(name, { ...room, props });
      canisters += props.filter((p) => p.kind === "ooze-canisters").length;
      consoles += props.filter((p) => p.kind === "gravity-console").length;
      for (const t of props.filter((p) => p.kind === "designer-tower")) {
        towers++;
        const side = SIDE_OF_TURN[t.turn];
        const cx = Math.floor(t.x);
        const cy = Math.floor(t.y);
        const along = side === "n" || side === "s";
        const desk = room.fixtures.find(
          (f) =>
            isDesk(f) &&
            f.slot.side === side &&
            (along
              ? f.slot.y === cy && Math.abs(f.slot.x - cx) === 1
              : f.slot.x === cx && Math.abs(f.slot.y - cy) === 1),
        );
        expect(
          desk,
          `${name} tower at ${String(cx)},${String(cy)}`,
        ).toBeDefined();
      }
      const deskless = {
        ...room,
        fixtures: room.fixtures.filter((f) => !isDesk(f)),
      };
      expect(() => dressRoom(deskless, NO_RESERVE, forced), name).not.toThrow();
      expect(
        dressRoom(deskless, NO_RESERVE, forced).some(
          (p) => p.kind === "designer-tower",
        ),
        name,
      ).toBe(false);
    }
    // Each forced kind lands somewhere, so dropping any one of the three
    // steps fails here.
    expect(towers).toBeGreaterThan(0);
    expect(canisters).toBeGreaterThan(0);
    expect(consoles).toBeGreaterThan(0);
    // The canned bridge's desk cells are all taken: the tower falls back.
    const bridge = generateRoom(CANNED_BRIDGE);
    expect(
      dressRoom(bridge, NO_RESERVE, forced).some(
        (p) => p.kind === "designer-tower",
      ),
    ).toBe(false);
  }, 30_000);
  it("pushes the tower to its desk's side, its near side TOWER_DESK_GAP from that border, the lower desk between two (2.6d C13)", () => {
    // Mutation caught: the push flipped (the tower's far side on the desk's
    // border), or the tie between two desks read from the fixture list.
    const isDesk = (f: Fixture) =>
      f.kind === "terminal" ||
      (f.kind === "machine" &&
        (DESK_MACHINES as readonly string[]).includes(f.machine));
    // The near side's gap to the border on the desk's side, in metres.
    const nearGap = (room: RoomBase, t: Prop) => {
      const side = SIDE_OF_TURN[t.turn];
      const cx = Math.floor(t.x);
      const cy = Math.floor(t.y);
      const along = side === "n" || side === "s";
      const at = along ? cx : cy;
      const desks = room.fixtures
        .filter(
          (f) =>
            isDesk(f) &&
            f.slot.side === side &&
            (along ? f.slot.y === cy : f.slot.x === cx) &&
            Math.abs((along ? f.slot.x : f.slot.y) - at) === 1,
        )
        .map((f) => (along ? f.slot.x : f.slot.y))
        .sort((a, b) => a - b);
      const desk = desks[0];
      if (desk === undefined) throw new Error("a tower stands beside a desk");
      const box = boxOf(t);
      const [lo, hi] = along ? [box.x0, box.x1] : [box.z0, box.z1];
      return {
        between: desks.length === 2,
        gap: desk < at ? lo - at * CELL : (at + 1) * CELL - hi,
      };
    };
    const forced: RareDraws = { ...NO_RARE, tower: true };
    let towers = 0;
    for (const { name, room } of [
      ...ALL,
      ...BRIDGES,
      ...SEEDS.map((r, k) => ({ name: `seed ${String(k)}`, room: r })),
    ])
      for (const t of dressRoom(room, NO_RESERVE, forced).filter(
        (p) => p.kind === "designer-tower",
      )) {
        towers++;
        // round3 rounds the anchor to a thousandth of a cell, 1 mm.
        expect(
          Math.abs(nearGap(room, t).gap - TOWER_DESK_GAP),
          name,
        ).toBeLessThanOrEqual(1.01e-3);
      }
    expect(towers).toBeGreaterThan(0);

    // Between two desks: the gallery's west wall holds terminals at y 12
    // and 14 with cell (0, 13) between them. With the bay benches taken
    // out and cell (0, 11) reserved, (0, 13) is the tower's only place. It
    // goes to the lower desk (y 12) in either fixture order.
    const gallery = galleryRoom();
    const walls = gallery.fixtures.filter(
      (f) => f.kind !== "machine" || !isDesk(f),
    );
    const north: Reserved = {
      boxes: [{ x0: 0, x1: CELL, z0: 11 * CELL, z1: 12 * CELL }],
      edges: new Set<string>(),
    };
    for (const fixtures of [walls, [...walls].reverse()]) {
      const room = { ...gallery, fixtures };
      expect(deskSides(room).get("0,13,w")).toBe(-1);
      const tower = dressRoom(room, north, forced).find(
        (p) => p.kind === "designer-tower",
      );
      if (tower === undefined) throw new Error("the tower lands at (0, 13)");
      expect([Math.floor(tower.x), Math.floor(tower.y)]).toEqual([0, 13]);
      const { between, gap } = nearGap(room, tower);
      expect(between).toBe(true);
      expect(Math.abs(gap - TOWER_DESK_GAP)).toBeLessThanOrEqual(1.01e-3);
    }
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
