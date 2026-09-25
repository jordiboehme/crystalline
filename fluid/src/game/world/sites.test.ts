import { describe, expect, it } from "vitest";

import { CANNED_BRIDGE, CANNED_HUB, galleryRoom } from "./canned";
import { decorFootprint, footprint, footprintOf } from "./footprints";
import { generateRoom } from "./generate";
import { ARRIVAL_DISTANCE, wallPoint } from "./interact";
import { STEP, doorwayColumns, isFloor, wallRuns, wallSlots } from "./layout";
import { PLAYER_RADIUS } from "./move";
import {
  LANE_DEPTH,
  LANE_WIDTH,
  SHEET_LANE_DEPTH,
  SHEET_LANE_WIDTH,
} from "./props";
import propsSource from "./props.ts?raw";
import {
  dressingSites,
  edgeKey,
  fitsFloor,
  interiorBand,
  isLargeHall,
  overlaps,
  turnForSide,
  wallAnchor,
  type RoomBase,
} from "./sites";
import sitesSource from "./sites.ts?raw";
import type { Box, PlaceInput, PlaceReference, WallSlot } from "./types";
import { CELL } from "./units";

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

function tags(n: number) {
  return Array.from({ length: n }, (_, i) => `tag-${String(i)}`);
}

const bridge = generateRoom(CANNED_BRIDGE);
const hub = generateRoom(CANNED_HUB);
const empty = generateRoom(place({}));
const building = generateRoom(
  place({ status: "draft", type: "runbook", tags: tags(4) }),
);
const gallery = galleryRoom();
const ROOMS: [string, RoomBase][] = [
  ["bridge", bridge],
  ["hub", hub],
  ["empty", empty],
  ["building", building],
  ["gallery", gallery],
];

function entranceOf(room: RoomBase): WallSlot {
  return { ...room.entrance, side: "s" };
}

/** Every doorway cell: a floor cell in a doorway column. */
function doorwayCells(room: RoomBase) {
  const out = new Set<string>();
  for (const c of doorwayColumns(room))
    for (let y = 0; y < room.depth; y++)
      if (isFloor(room.grid, c, y)) out.add(`${String(c)},${String(y)}`);
  return out;
}

function nextTo(cells: Set<string>, x: number, y: number) {
  return Object.values(STEP).some(([dx, dy]) =>
    cells.has(`${String(x + dx)},${String(y + dy)}`),
  );
}

function isWallEdge(room: RoomBase, e: WallSlot) {
  const [dx, dy] = STEP[e.side];
  return (
    isFloor(room.grid, e.x, e.y) && !isFloor(room.grid, e.x + dx, e.y + dy)
  );
}

function bayCount(room: RoomBase) {
  return room.bays.length;
}

describe("the layout a room states", () => {
  it("names the doorway columns its corridor and bays open through, and the entrance at the spawn", () => {
    for (const [name, room] of ROOMS) {
      const want = new Set<number>();
      if (room.corridor !== null) want.add(room.corridor.x1);
      for (const b of room.bays) want.add(b.x0 - 1);
      expect(doorwayColumns(room), name).toEqual(want);
      expect(room.entrance, name).toEqual({ x: room.spawn.x, y: room.spawn.y });
    }
    expect(hub.corridor).not.toBeNull();
    expect(hub.bays.length).toBeGreaterThan(0);
  });
});

describe("wallAnchor and turnForSide", () => {
  it("anchors a prop at the middle of its wall, turned to face into the room", () => {
    expect(wallAnchor({ x: 3, y: 4, side: "n" })).toEqual({
      x: 3.5,
      y: 4,
      turn: 2,
    });
    expect(wallAnchor({ x: 3, y: 4, side: "s" })).toEqual({
      x: 3.5,
      y: 5,
      turn: 0,
    });
    expect(wallAnchor({ x: 3, y: 4, side: "w" })).toEqual({
      x: 3,
      y: 4.5,
      turn: 1,
    });
    expect(wallAnchor({ x: 3, y: 4, side: "e" })).toEqual({
      x: 4,
      y: 4.5,
      turn: 3,
    });
  });

  it("maps s 0, w 1, n 2, e 3", () => {
    expect(turnForSide("s")).toBe(0);
    expect(turnForSide("w")).toBe(1);
    expect(turnForSide("n")).toBe(2);
    expect(turnForSide("e")).toBe(3);
  });

  it("puts the anchor on the wall point interact.ts measures from", () => {
    for (const side of ["n", "e", "s", "w"] as const) {
      const e = { x: 5, y: 2, side };
      const a = wallAnchor(e);
      const w = wallPoint(e);
      expect(a.x * CELL).toBeCloseTo(w.x);
      expect(a.y * CELL).toBeCloseTo(w.z);
    }
  });
});

describe("edgeKey and overlaps", () => {
  it("keys an edge x,y,side", () => {
    expect(edgeKey({ x: 2, y: 7, side: "w" })).toBe("2,7,w");
  });

  it("does not count touching edges as overlap", () => {
    const a: Box = { x0: 0, z0: 0, x1: 1, z1: 1 };
    expect(overlaps(a, { x0: 1, z0: 0, x1: 2, z1: 1 })).toBe(false);
    expect(overlaps(a, { x0: 0, z0: 1, x1: 1, z1: 2 })).toBe(false);
    expect(overlaps(a, { x0: 0.5, z0: 0.5, x1: 2, z1: 2 })).toBe(true);
    expect(overlaps(a, { x0: 0.2, z0: 0.2, x1: 0.8, z1: 0.8 })).toBe(true);
  });
});

describe("free wall edges", () => {
  it("leaves out fixture edges, the entrance and its neighbours, and doorway columns", () => {
    for (const [name, room] of ROOMS) {
      const sites = dressingSites(room);
      const cols = doorwayColumns(room);
      const entrance = entranceOf(room);
      const banned = new Set(room.fixtures.map((f) => edgeKey(f.slot)));
      for (let dx = -1; dx <= 1; dx++)
        banned.add(edgeKey({ ...entrance, x: entrance.x + dx }));
      expect(sites.free.size, name).toBeGreaterThan(0);
      for (const k of sites.free) {
        expect(banned.has(k), `${name} ${k}`).toBe(false);
        const x = Number(k.split(",")[0]);
        expect(cols.has(x), `${name} ${k}`).toBe(false);
      }
    }
  });

  it("is every wall edge not in noRun, and noRun holds every fixture edge", () => {
    for (const [name, room] of ROOMS) {
      const sites = dressingSites(room);
      const edges = wallRuns(room.grid).flat().map(edgeKey);
      const expected = edges.filter((k) => !sites.noRun.has(k));
      expect([...sites.free].sort(), name).toEqual(expected.sort());
      for (const k of sites.fixtureEdges) expect(sites.noRun.has(k)).toBe(true);
      expect(sites.fixtureEdges.size, name).toBe(
        new Set(room.fixtures.map((f) => edgeKey(f.slot))).size,
      );
    }
  });

  it("keeps every slot no fixture took free, outside the entrance and the doorways", () => {
    // The entrance is a slot no fixture takes but is excluded by rule, and
    // so are its neighbours and the slots of doorway-column cells.
    for (const [name, room] of ROOMS) {
      const sites = dressingSites(room);
      const cols = doorwayColumns(room);
      const entrance = entranceOf(room);
      const used = new Set(room.fixtures.map((f) => edgeKey(f.slot)));
      const unused = wallSlots(room.grid).filter(
        (s) =>
          !used.has(edgeKey(s)) &&
          !cols.has(s.x) &&
          !(
            s.side === "s" &&
            s.y === entrance.y &&
            Math.abs(s.x - entrance.x) <= 1
          ),
      );
      expect(unused.length, name).toBeGreaterThan(0);
      for (const s of unused)
        expect(sites.free.has(edgeKey(s)), name).toBe(true);
    }
  });

  it("orders its runs as wallRuns does", () => {
    expect(dressingSites(hub).runs).toEqual(wallRuns(hub.grid));
  });
});

describe("lanes", () => {
  const sites = dressingSites(hub);
  const wayKinds = new Set(["door", "hatch", "portal", "terminal", "machine"]);

  it("gives the hub one lane per fixture, the entrance and each doorway column", () => {
    const cols = doorwayColumns(hub).size;
    expect(cols).toBe(1 + bayCount(hub));
    expect(bayCount(hub)).toBeGreaterThan(0);
    expect(sites.lanes).toHaveLength(hub.fixtures.length + 1 + cols);
    for (const f of hub.fixtures) {
      if (!wayKinds.has(f.kind)) continue;
      expect(sites.lanes).toContainEqual(
        footprint(f.slot, { along: LANE_WIDTH, out: LANE_DEPTH }),
      );
    }
  });

  it("gives every poster and the placard a 1.2 m by 1.5 m viewing lane", () => {
    expect(SHEET_LANE_WIDTH).toBe(1.2);
    expect(SHEET_LANE_DEPTH).toBe(1.5);
    let checked = 0;
    for (const room of [hub, bridge]) {
      const lanes = dressingSites(room).lanes;
      for (const f of room.fixtures) {
        if (f.kind !== "poster" && f.kind !== "placard") continue;
        expect(lanes, `${f.kind} ${edgeKey(f.slot)}`).toContainEqual(
          footprint(f.slot, { along: 1.2, out: 1.5 }),
        );
        checked++;
      }
    }
    expect(hub.fixtures.some((f) => f.kind === "placard")).toBe(true);
    expect(hub.fixtures.some((f) => f.kind === "poster")).toBe(true);
    expect(checked).toBeGreaterThan(2);
  });

  it("holds every arrival point of a door, hatch or portal with the player's radius to spare", () => {
    let checked = 0;
    for (const f of hub.fixtures) {
      if (f.kind !== "door" && f.kind !== "hatch" && f.kind !== "portal")
        continue;
      const lane = footprint(f.slot, { along: LANE_WIDTH, out: LANE_DEPTH });
      const w = wallPoint(f.slot);
      const px = w.x + w.inward[0] * ARRIVAL_DISTANCE;
      const pz = w.z + w.inward[1] * ARRIVAL_DISTANCE;
      expect(px - PLAYER_RADIUS).toBeGreaterThanOrEqual(lane.x0);
      expect(px + PLAYER_RADIUS).toBeLessThanOrEqual(lane.x1);
      expect(pz - PLAYER_RADIUS).toBeGreaterThanOrEqual(lane.z0);
      expect(pz + PLAYER_RADIUS).toBeLessThanOrEqual(lane.z1);
      checked++;
    }
    expect(checked).toBeGreaterThan(40);
  });

  it("runs the entrance lane from the entrance wall to the hall's centre row", () => {
    const e = entranceOf(hub);
    const cx = (e.x + 0.5) * CELL;
    expect(sites.lanes).toContainEqual({
      x0: cx - LANE_WIDTH / 2,
      x1: cx + LANE_WIDTH / 2,
      z0: ((hub.hall.y0 + hub.hall.y1) / 2) * CELL,
      z1: (e.y + 1) * CELL,
    });
  });

  it("covers each doorway column and a cell either side over its open rows", () => {
    for (const c of doorwayColumns(hub)) {
      const rows = Array.from({ length: hub.depth }, (_, y) => y).filter((y) =>
        isFloor(hub.grid, c, y),
      );
      expect(sites.lanes).toContainEqual({
        x0: (c - 1) * CELL,
        x1: (c + 2) * CELL,
        z0: Math.min(...rows) * CELL,
        z1: (Math.max(...rows) + 1) * CELL,
      });
    }
  });
});

describe("taken", () => {
  it("holds the fixture and decor footprints and the scaffold frames", () => {
    expect(building.scaffold.length).toBeGreaterThan(0);
    for (const [name, room] of ROOMS) {
      const expected = [
        ...room.fixtures.map(footprintOf),
        ...room.decor.map(decorFootprint),
        ...room.scaffold,
      ].filter((b) => b !== null);
      expect(dressingSites(room).taken, name).toEqual(expected);
    }
  });
});

describe("corner zones", () => {
  it("gives the hall and each bay four zones, clear of the doorways", () => {
    for (const [name, room] of ROOMS) {
      const { zones } = dressingSites(room);
      const cols = doorwayColumns(room);
      const doors = doorwayCells(room);
      expect(zones, name).toHaveLength(4 * (1 + bayCount(room)));
      expect(new Set(zones.map((z) => z.key)).size, name).toBe(zones.length);
      for (const z of zones) {
        expect(z.spots.length, name).toBeLessThanOrEqual(4);
        for (const s of z.spots) {
          expect(s.wall).toBeNull();
          expect(s.zone).toBe(z.key);
          expect(isFloor(room.grid, s.cx, s.cy)).toBe(true);
          expect(cols.has(s.cx)).toBe(false);
          expect(nextTo(doors, s.cx, s.cy), `${name} ${z.key}`).toBe(false);
        }
      }
    }
  });

  it("keeps the four zones of the minimum hall apart", () => {
    expect(empty.hall).toEqual({ x0: 0, y0: 0, x1: 5, y1: 6 });
    const { zones } = dressingSites(empty);
    expect(zones.map((z) => z.key)).toEqual(["0,0", "3,0", "0,4", "3,4"]);
    const cells = zones.flatMap((z) => z.spots.map((s) => `${s.cx},${s.cy}`));
    expect(cells).toHaveLength(16);
    expect(new Set(cells).size).toBe(16);
  });

  it("leaves out a hall corner cell beside a bay's doorway", () => {
    expect(bayCount(hub)).toBeGreaterThan(0);
    const doors = doorwayCells(hub);
    const cut = dressingSites(hub).zones.filter((z) => z.spots.length < 4);
    expect(cut.length).toBeGreaterThan(0);
    for (const z of cut) {
      const [zx, zy] = z.key.split(",").map(Number) as [number, number];
      const cells = [
        [zx, zy],
        [zx + 1, zy],
        [zx, zy + 1],
        [zx + 1, zy + 1],
      ] as const;
      expect(cells.some(([x, y]) => nextTo(doors, x, y))).toBe(true);
    }
  });
});

describe("wall-side spots", () => {
  it("lie outside every zone, on a real wall edge that carries no fixture", () => {
    for (const [name, room] of ROOMS) {
      const sites = dressingSites(room);
      const zoneCells = new Set<string>();
      for (const z of sites.zones) {
        const [zx, zy] = z.key.split(",").map(Number) as [number, number];
        for (let y = zy; y < zy + 2; y++)
          for (let x = zx; x < zx + 2; x++) zoneCells.add(`${x},${y}`);
      }
      const doors = doorwayCells(room);
      const cells = new Set<string>();
      expect(sites.wallSide.length, name).toBeGreaterThan(0);
      for (const s of sites.wallSide) {
        expect(s.zone).toBeNull();
        if (s.wall === null) throw new Error(`${name}: no wall`);
        expect(zoneCells.has(`${s.cx},${s.cy}`), name).toBe(false);
        expect(nextTo(doors, s.cx, s.cy), name).toBe(false);
        const edge = { x: s.cx, y: s.cy, side: s.wall };
        expect(isWallEdge(room, edge), `${name} ${edgeKey(edge)}`).toBe(true);
        expect(sites.fixtureEdges.has(edgeKey(edge)), name).toBe(false);
        cells.add(`${s.cx},${s.cy}`);
      }
      expect(cells.size, name).toBe(sites.wallSide.length);
    }
  });

  it("stay in the hall and the bays, never in the corridor", () => {
    const { wallSide } = dressingSites(hub);
    for (const s of wallSide) expect(s.cx).toBeGreaterThanOrEqual(hub.hall.x0);
  });
});

describe("long walls", () => {
  it("are the north and south walls of a 13-wide, 10-deep hall", () => {
    const room = generateRoom(
      place({
        relations: ["a", "b", "c", "d", "e", "f"].map(rel),
        tags: tags(4),
      }),
    );
    const hall = room.hall;
    expect(hall.x1 - hall.x0).toBe(13);
    expect(hall.y1 - hall.y0).toBe(10);
    const [north, south] = dressingSites(room).longWalls;
    expect(north.map((e) => e.side)).toEqual(Array(13).fill("n"));
    expect(north.map((e) => e.x)).toEqual(
      Array.from({ length: 13 }, (_, i) => hall.x0 + i),
    );
    expect(north.every((e) => e.y === hall.y0)).toBe(true);
    expect(south.map((e) => e.side)).toEqual(Array(13).fill("s"));
    expect(south.map((e) => e.x)).toEqual(
      Array.from({ length: 13 }, (_, i) => hall.x1 - 1 - i),
    );
    expect(south.every((e) => e.y === hall.y1 - 1)).toBe(true);
  });

  it("are the west and east walls of a 5-wide, 12-deep hall", () => {
    const room = generateRoom(place({ tags: tags(5) }));
    const hall = room.hall;
    expect(hall.x1 - hall.x0).toBe(5);
    expect(hall.y1 - hall.y0).toBe(12);
    const [west, east] = dressingSites(room).longWalls;
    expect(west.map((e) => e.side)).toEqual(Array(12).fill("w"));
    expect(west.map((e) => e.y)).toEqual(
      Array.from({ length: 12 }, (_, i) => hall.y1 - 1 - i),
    );
    expect(west.every((e) => e.x === hall.x0)).toBe(true);
    expect(east.map((e) => e.side)).toEqual(Array(12).fill("e"));
    expect(east.map((e) => e.y)).toEqual(
      Array.from({ length: 12 }, (_, i) => hall.y0 + i),
    );
    expect(east.every((e) => e.x === hall.x1 - 1)).toBe(true);
  });

  it("stay inside the hall when a corridor and bays join it", () => {
    const hall = hub.hall;
    for (const wall of dressingSites(hub).longWalls) {
      expect(wall.length).toBeGreaterThan(0);
      for (const e of wall) {
        expect(e.x).toBeGreaterThanOrEqual(hall.x0);
        expect(e.x).toBeLessThan(hall.x1);
        expect(e.y).toBeGreaterThanOrEqual(hall.y0);
        expect(e.y).toBeLessThan(hall.y1);
      }
    }
  });
});

describe("fitsFloor", () => {
  const cellBox = (x: number, y: number, pad = 0.3): Box => ({
    x0: x * CELL + pad,
    x1: (x + 1) * CELL - pad,
    z0: y * CELL + pad,
    z1: (y + 1) * CELL - pad,
  });

  it("accepts a box inside one hall cell, or flush with its borders", () => {
    const { x0, y0 } = hub.hall;
    expect(fitsFloor(hub, cellBox(x0, y0))).toBe(true);
    expect(fitsFloor(hub, cellBox(x0, y0, 0))).toBe(true);
    expect(fitsFloor(hub, cellBox(x0 + 3, y0 + 3))).toBe(true);
  });

  it("accepts a box inside a bay", () => {
    const bay = [...doorwayColumns(hub)].find((c) => c >= hub.hall.x1);
    if (bay === undefined) throw new Error("no bay");
    expect(fitsFloor(hub, cellBox(bay + 2, 2))).toBe(true);
  });

  it("refuses a box that reaches into void", () => {
    const { x0, y0 } = hub.hall;
    const box = cellBox(x0, y0);
    expect(fitsFloor(hub, { ...box, z0: box.z0 - 0.5 })).toBe(false);
    expect(fitsFloor(empty, { ...cellBox(4, 2), x1: 5 * CELL + 0.1 })).toBe(
      false,
    );
  });

  it("refuses a box in a doorway column or the corridor", () => {
    const col = hub.hall.x0 - 1;
    const row = hub.hall.y1 - 1;
    expect(isFloor(hub.grid, col, row)).toBe(true);
    expect(fitsFloor(hub, cellBox(col, row))).toBe(false);
    // Straddling the doorway column and the hall.
    expect(
      fitsFloor(hub, { ...cellBox(hub.hall.x0, row), x0: col * CELL + 1.5 }),
    ).toBe(false);
    expect(isFloor(hub.grid, 1, row)).toBe(true);
    expect(fitsFloor(hub, cellBox(1, row))).toBe(false);
    for (const c of doorwayColumns(hub)) {
      if (c < hub.hall.x1) continue;
      expect(fitsFloor(hub, cellBox(c, 3))).toBe(false);
    }
  });

  it("refuses an empty box", () => {
    expect(fitsFloor(hub, { x0: 1, x1: 1, z0: 1, z1: 2 })).toBe(false);
  });
});

describe("the interior band and large halls", () => {
  const hall = (w: number, d: number) => ({ x0: 0, y0: 0, x1: w, y1: d });
  it("shrinks the hall by BAND_MARGIN on every side, or gives null", () => {
    expect(interiorBand(hall(13, 10))).toEqual({ x0: 2, y0: 2, x1: 11, y1: 8 });
    expect(interiorBand(hall(4, 10))).toBeNull();
    expect(interiorBand({ x0: 26, y0: 0, x1: 50, y1: 24 })).toEqual({
      x0: 28,
      y0: 2,
      x1: 48,
      y1: 22,
    });
  });
  it("calls a hall large from 9 by 8, either way round", () => {
    expect(isLargeHall(hall(9, 8))).toBe(true);
    expect(isLargeHall(hall(8, 9))).toBe(true);
    expect(isLargeHall(hall(8, 8))).toBe(false);
    expect(isLargeHall(hall(9, 7))).toBe(false);
    expect(isLargeHall(hall(13, 10))).toBe(true);
    for (const [w, d] of [
      [7, 6],
      [5, 6],
      [5, 12],
    ] as const)
      expect(isLargeHall(hall(w, d))).toBe(false);
  });
});

describe("the generator side's imports (ruling 20)", () => {
  // `interact.ts` imports `./generate`, and `CELL` comes from `./units`,
  // never from `./generate`, so none of the three may be reached from here.
  it("keeps sites.ts and props.ts away from move, generate and interact", () => {
    for (const source of [sitesSource, propsSource]) {
      expect(source).not.toMatch(/from\s+["']\.\/(move|generate|interact)["']/);
    }
  });
});
