import { describe, expect, it } from "vitest";

import {
  BAY,
  CORRIDOR_WIDTH,
  MAX_BAYS,
  createSlotPool,
  doorwayColumns,
  isFloor,
  planLayout,
  wallRuns,
  wallSlots,
  type Layout,
  type LayoutNeed,
} from "./layout";
import { galleryRoom } from "./canned";
import type { Side, WallSlot } from "./types";

const none: LayoutNeed = {
  north: 0,
  west: 0,
  east: 0,
  south: 0,
  any: 0,
  hatches: 0,
};

const small: LayoutNeed = {
  ...none,
  north: 3,
  west: 2,
  east: 2,
  south: 2,
  hatches: 2,
};

function key(s: WallSlot) {
  return `${s.x},${s.y},${s.side}`;
}

const STEP: Record<Side, [number, number]> = {
  n: [0, -1],
  e: [1, 0],
  s: [0, 1],
  w: [-1, 0],
};

/** The cell a slot's wall faces. */
function beyond(s: WallSlot): [number, number] {
  const [dx, dy] = STEP[s.side];
  return [s.x + dx, s.y + dy];
}

function coords(slots: WallSlot[], side: Side, axis: "x" | "y") {
  return slots.filter((s) => s.side === side).map((s) => s[axis]);
}

/** Every slot the pool hands out until it runs dry. */
function drain(layout: Layout) {
  const pool = createSlotPool(layout);
  const out: WallSlot[] = [];
  for (let s = pool.take("any"); s !== null; s = pool.take("any")) out.push(s);
  return out;
}

describe("planLayout", () => {
  it("gives a small need a plain 7x6 hall with no bays and no corridor", () => {
    const layout = planLayout(small);
    expect(layout.width).toBe(7);
    expect(layout.depth).toBe(6);
    expect(layout.hall).toEqual({ x0: 0, y0: 0, x1: 7, y1: 6 });
    expect(layout.bays).toEqual([]);
    expect(layout.corridor).toBeNull();
    expect(layout.grid).toEqual(Array.from({ length: 6 }, () => "......."));
    expect(layout.entrance).toEqual({ x: 3, y: 5, side: "s" });
    expect(layout.placard).toEqual({ x: 2, y: 5, side: "s" });
  });

  it("keeps every row as wide as the grid", () => {
    for (const need of [
      small,
      { ...small, hatches: 12, south: 0 },
      { ...small, north: 40, east: 40, west: 40, any: 60 },
    ]) {
      const layout = planLayout(need);
      expect(layout.grid).toHaveLength(layout.depth);
      for (const row of layout.grid) expect(row).toHaveLength(layout.width);
      expect(layout.grid.join("")).toMatch(/^[. ]+$/);
    }
  });

  it("puts a backlink corridor west of the hall past eight hatches", () => {
    const layout = planLayout({ ...small, south: 0, hatches: 12 });
    const corridor = layout.corridor;
    if (corridor === null) throw new Error("no corridor");
    const length = 2 * Math.ceil(12 / 2) + 1;
    expect(corridor).toEqual({
      x0: 0,
      y0: layout.hall.y1 - CORRIDOR_WIDTH,
      x1: length,
      y1: layout.hall.y1,
    });
    const ox = layout.hall.x0;
    expect(ox).toBe(length + 1);
    // The doorway column: void except the two corridor rows.
    for (let y = 0; y < layout.depth; y++) {
      const open = y >= corridor.y0 && y < corridor.y1;
      expect(isFloor(layout.grid, ox - 1, y)).toBe(open);
    }
    // The corridor itself is floor, and void above it.
    for (let x = corridor.x0; x < corridor.x1; x++) {
      expect(isFloor(layout.grid, x, corridor.y0)).toBe(true);
      expect(isFloor(layout.grid, x, corridor.y1 - 1)).toBe(true);
      expect(isFloor(layout.grid, x, corridor.y0 - 1)).toBe(false);
    }
  });

  it("has no corridor at exactly eight hatches", () => {
    expect(planLayout({ ...small, south: 8, hatches: 8 }).corridor).toBeNull();
  });

  it("grows 8x8 bays, at most four, each through a two-cell doorway", () => {
    const layout = planLayout({
      ...none,
      north: 100,
      west: 100,
      east: 100,
      any: 100,
    });
    expect(layout.bays.length).toBeGreaterThan(0);
    expect(layout.bays.length).toBeLessThanOrEqual(MAX_BAYS);
    let prevEast = layout.hall.x1;
    for (const bay of layout.bays) {
      expect(bay.x1 - bay.x0).toBe(BAY);
      expect(bay.y1 - bay.y0).toBe(BAY);
      expect(bay.y0).toBe(0);
      // One void column between it and what lies west, open on rows 3 and 4.
      expect(bay.x0).toBe(prevEast + 1);
      const door = prevEast;
      for (let y = 0; y < layout.depth; y++) {
        expect(isFloor(layout.grid, door, y)).toBe(y === 3 || y === 4);
      }
      for (let y = 0; y < BAY; y++)
        for (let x = bay.x0; x < bay.x1; x++)
          expect(isFloor(layout.grid, x, y)).toBe(true);
      prevEast = bay.x1;
    }
    expect(layout.width).toBe(prevEast);
  });

  it("adds only as many bays as it takes to fit", () => {
    const hallOnly = planLayout({ ...none, north: 11, east: 11, west: 11 });
    expect(hallOnly.bays).toEqual([]);
    const oneMore = planLayout({
      ...none,
      north: 11,
      east: 11,
      west: 11,
      any: 20,
    });
    expect(oneMore.bays.length).toBeGreaterThan(0);
    const slots = drain(oneMore);
    expect(slots.length).toBeGreaterThanOrEqual(11 * 3 + 20);
    // One bay fewer would not have held them: the same hall without bays
    // (`any` does not size the hall) has fewer slots than the need.
    expect(oneMore.bays).toHaveLength(1);
    expect(drain(hallOnly).length).toBeLessThan(11 * 3 + 20);
  });

  it("widens the hall so eight hatches fit on its south wall", () => {
    const layout = planLayout({ ...none, south: 8, hatches: 8 });
    const pool = createSlotPool(layout);
    for (let i = 0; i < 8; i++) {
      const slot = pool.take("south");
      expect(slot?.side).toBe("s");
      expect(slot?.y).toBe(layout.hall.y1 - 1);
    }
  });

  it("caps the hall at 24 by 24 cells", () => {
    const layout = planLayout({ ...none, north: 80, east: 80, west: 80 });
    expect(layout.hall.x1 - layout.hall.x0).toBe(24);
    expect(layout.hall.y1 - layout.hall.y0).toBe(24);
  });
});

describe("isFloor", () => {
  it("is false outside the grid", () => {
    const { grid } = planLayout(small);
    expect(isFloor(grid, 0, 0)).toBe(true);
    expect(isFloor(grid, -1, 0)).toBe(false);
    expect(isFloor(grid, 0, -1)).toBe(false);
    expect(isFloor(grid, 7, 0)).toBe(false);
    expect(isFloor(grid, 0, 6)).toBe(false);
  });
});

describe("wallSlots", () => {
  it("returns milestone 1's positions in a plain 7x6 hall", () => {
    const slots = wallSlots(planLayout(small).grid);
    expect(coords(slots, "n", "x")).toEqual([1, 3, 5]);
    expect(coords(slots, "e", "y")).toEqual([1, 3, 5]);
    expect(coords(slots, "s", "x")).toEqual([5, 3, 1]);
    expect(coords(slots, "w", "y")).toEqual([4, 2, 0]);
    expect(slots.filter((s) => s.side === "n").every((s) => s.y === 0)).toBe(
      true,
    );
    expect(slots.filter((s) => s.side === "e").every((s) => s.x === 6)).toBe(
      true,
    );
  });

  it("only puts slots on floor cells facing void, never twice on one wall", () => {
    for (const need of [
      small,
      { ...small, hatches: 13, south: 0 },
      { ...none, north: 60, east: 60, west: 60, any: 60, hatches: 24 },
    ]) {
      const { grid } = planLayout(need);
      const slots = wallSlots(grid);
      const keys = slots.map(key);
      expect(new Set(keys).size).toBe(keys.length);
      for (const s of slots) {
        expect(isFloor(grid, s.x, s.y)).toBe(true);
        const [bx, by] = beyond(s);
        expect(isFloor(grid, bx, by)).toBe(false);
      }
    }
  });

  it("keeps the hall's south slots when a corridor joins its south wall", () => {
    const plain = planLayout({ ...small, south: 0, hatches: 8 });
    const joined = planLayout({ ...small, south: 0, hatches: 9 });
    const hallSouth = (layout: Layout) =>
      wallSlots(layout.grid)
        .filter(
          (s) =>
            s.side === "s" &&
            s.y === layout.hall.y1 - 1 &&
            s.x >= layout.hall.x0 &&
            s.x < layout.hall.x1,
        )
        .map((s) => s.x - layout.hall.x0);
    expect(hallSouth(joined)).toEqual(hallSouth(plain));
  });
});

describe("createSlotPool", () => {
  it("never hands out the entrance, the placard or a slot twice", () => {
    for (const need of [
      small,
      { ...small, hatches: 20, south: 0 },
      { ...none, north: 60, east: 60, west: 60, any: 60 },
    ]) {
      const layout = planLayout(need);
      const slots = drain(layout);
      const keys = slots.map(key);
      expect(new Set(keys).size).toBe(keys.length);
      expect(keys).not.toContain(key(layout.entrance));
      expect(keys).not.toContain(key(layout.placard));
    }
  });

  it("serves a wall's own slots first, then falls through to the rest of the hall", () => {
    const layout = planLayout(small);
    const pool = createSlotPool(layout);
    const north = [pool.take("north"), pool.take("north"), pool.take("north")];
    expect(north.map((s) => s && key(s))).toEqual(["1,0,n", "3,0,n", "5,0,n"]);
    // North is used up: `any` goes on round the hall, east first.
    expect(pool.take("north")).toEqual({ x: 6, y: 1, side: "e" });
    expect(pool.take("west")).toEqual({ x: 0, y: 4, side: "w" });
    expect(pool.take("south")).toEqual({ x: 5, y: 5, side: "s" });
    // South x = 3 is the entrance; x = 1 is the last south slot.
    expect(pool.take("south")).toEqual({ x: 1, y: 5, side: "s" });
    expect(pool.take("south")).toEqual({ x: 6, y: 3, side: "e" });
  });

  it("returns null when every slot is taken", () => {
    const layout = planLayout(small);
    const pool = createSlotPool(layout);
    const total = drain(layout).length;
    for (let i = 0; i < total; i++) expect(pool.take("any")).not.toBeNull();
    expect(pool.take("any")).toBeNull();
    expect(pool.take("north")).toBeNull();
  });

  it("serves corridor slots only to `corridor`, and the hall once they run out", () => {
    const layout = planLayout({ ...small, south: 0, hatches: 12 });
    const corridor = layout.corridor;
    if (corridor === null) throw new Error("no corridor");
    const pool = createSlotPool(layout);
    const inCorridor = (s: WallSlot) => s.x < layout.hall.x0;
    const taken: WallSlot[] = [];
    for (let s = pool.take("corridor"); s !== null && inCorridor(s);) {
      taken.push(s);
      s = pool.take("corridor");
      if (s !== null && !inCorridor(s)) {
        // Past the corridor it falls through to the hall, north first.
        expect(s.side).toBe("n");
        expect(s.x).toBeGreaterThanOrEqual(layout.hall.x0);
        break;
      }
    }
    expect(taken.length).toBeGreaterThanOrEqual(12);
    // `any` never reaches into the corridor.
    const again = createSlotPool(layout);
    for (let s = again.take("any"); s !== null; s = again.take("any")) {
      expect(inCorridor(s)).toBe(false);
    }
  });

  it("hands out bay slots after the hall's", () => {
    const layout = planLayout({ ...none, north: 60, east: 60, west: 60 });
    expect(layout.bays.length).toBeGreaterThan(0);
    const slots = drain(layout);
    const inHall = (s: WallSlot) =>
      s.x >= layout.hall.x0 && s.x < layout.hall.x1;
    const firstBay = slots.findIndex((s) => !inHall(s));
    expect(firstBay).toBeGreaterThan(0);
    expect(slots.slice(firstBay).every((s) => !inHall(s))).toBe(true);
    // Bays in bay order.
    const bayOf = (s: WallSlot) =>
      layout.bays.findIndex((b) => s.x >= b.x0 && s.x < b.x1);
    const order = slots.slice(firstBay).map(bayOf);
    expect(order).toEqual([...order].sort((a, b) => a - b));
  });
});

describe("wallRuns", () => {
  const hubLike: LayoutNeed = {
    north: 20,
    west: 20,
    east: 20,
    south: 0,
    any: 40,
    hatches: 20,
  };

  it("walks the runs in wallSlots' order: every second edge is a slot", () => {
    const hub = planLayout(hubLike);
    expect(hub.corridor).not.toBeNull();
    expect(hub.bays.length).toBeGreaterThan(0);
    for (const grid of [planLayout(small).grid, hub.grid, galleryRoom().grid]) {
      const odd = wallRuns(grid).flatMap((r) =>
        r.filter((_, i) => i % 2 === 1),
      );
      expect(odd).toEqual(wallSlots(grid));
    }
  });

  it("finds four runs of 7, 6, 7 and 6 edges in a plain 7x6 hall", () => {
    const runs = wallRuns(planLayout(small).grid);
    expect(runs.map((r) => r.length)).toEqual([7, 6, 7, 6]);
    expect(runs.map((r) => r[0]?.side)).toEqual(["n", "e", "s", "w"]);
  });

  it("covers every wall edge once, each a floor cell facing void", () => {
    const { grid } = planLayout(hubLike);
    const edges = wallRuns(grid).flat();
    expect(new Set(edges.map(key)).size).toBe(edges.length);
    for (const e of edges) {
      expect(isFloor(grid, e.x, e.y)).toBe(true);
      const [bx, by] = beyond(e);
      expect(isFloor(grid, bx, by)).toBe(false);
    }
  });
});

describe("doorwayColumns", () => {
  it("returns the corridor's doorway and the column before each bay", () => {
    const layout = planLayout({
      north: 5,
      west: 5,
      east: 5,
      south: 0,
      any: 25,
      hatches: 12,
    });
    const corridor = layout.corridor;
    if (corridor === null) throw new Error("no corridor");
    expect(layout.bays).toHaveLength(2);
    const expected = [
      layout.hall.x0 - 1,
      ...layout.bays.map((b) => b.x0 - 1),
    ].sort((a, b) => a - b);
    expect([...doorwayColumns(layout)].sort((a, b) => a - b)).toEqual(expected);
  });

  it("is empty for a plain hall", () => {
    expect(doorwayColumns(planLayout(small)).size).toBe(0);
  });
});
