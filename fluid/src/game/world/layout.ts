/**
 * The floor plan of a room: a main hall, overflow bays and a backlink
 * corridor on one cell grid, and the wall slots its fixtures stand in.
 *
 * Milestone 1's room was one rectangle. A room now grows in parts, and the
 * parts are laid out by these rules, in this order:
 *
 * - **Hall.** `width = clamp(max(2*north + 1, 2*south + 3), 5, 24)` cells
 *   and `depth = clamp(2*max(west, east) + 2, 6, 24)`: the doors and portals
 *   on the north wall set its width, as in milestone 1, unless the south wall
 *   needs more, since up to eight hatches share it with the entrance and the
 *   placard and must all fit there; the terminals on the west and the
 *   machines on the east set its depth. It sits at grid x offset `ox` and y
 *   offset 0.
 * - **Entrance and placard.** The entrance is on the hall's south wall at
 *   `ox + floor(width/2)`; the placard sits one cell west of it. Both slots
 *   are reserved and never handed out.
 * - **Backlink corridor.** Only past eight hatches (up to eight fit on the
 *   hall's south wall). It is `2*ceil(hatches/2) + 1` cells long and
 *   `CORRIDOR_WIDTH` rows deep, bottom-aligned with the hall's south wall,
 *   and lies to the west, separated from the hall by one void column that is
 *   open on those two rows - the doorway. The hall then sits at
 *   `ox = corridorLength + 1`.
 * - **Bays.** When the hall's walls cannot hold every fixture that wants a
 *   hall wall (`north + west + east + south + any` against the slots the pool
 *   would hand out, entrance and placard excluded), `BAY` by `BAY` bays are
 *   added east of the hall, one after the other, until the fixtures fit or
 *   there are `MAX_BAYS`. Each is separated from what lies west of it by one
 *   void column open on rows 3 and 4 (the doorway), so the first bay's
 *   doorway is in the hall's east wall. The capacity is counted on the real
 *   grid after each bay, since a bay's doorway takes a slot from the wall it
 *   opens.
 * - **Grid.** One string per row, `"."` for floor and `" "` for void; every
 *   row is as long as the grid is wide. The grid is as deep as the hall, or
 *   as a bay when that is deeper.
 * - **Wall slots.** A floor cell's edge is a wall when the neighbour across
 *   it is void or outside the grid. The walls of each side are grouped into
 *   maximal straight runs, walked clockwise (north walls west to east, east
 *   walls north to south, south walls east to west, west walls south to
 *   north), and every second cell of a run is a slot, starting at the run's
 *   second cell. In a plain rectangle that is exactly milestone 1's slots,
 *   and two slots are never neighbours; only the placard, which sits one
 *   cell west of the entrance whatever the slot pattern, can stand next to a
 *   slot. A corridor's south wall runs on into
 *   the hall's, and walking it from the hall's east end keeps the hall's
 *   slots where they would be without a corridor.
 * - **Slot pool.** `north`, `west`, `east` and `south` ask for that wall of
 *   the hall, `corridor` for the corridor's walls, and `any` for the hall's
 *   walls in the order north, east, south, west and then each bay's walls in
 *   bay order. A preference that runs out falls through to `any`; `any`
 *   never reaches into the corridor, which is for hatches. `take` returns
 *   null when nothing is left.
 *
 * Everything here is pure and integer: the same need gives the same plan.
 */

import type { Rect, Side, WallSlot } from "./types";

export type { Rect };

/** Cells a side of one overflow bay. */
export const BAY = 8;

/** The most bays a room grows; a fixture past them is dropped. */
export const MAX_BAYS = 4;

/** Rows of the backlink corridor. */
export const CORRIDOR_WIDTH = 2;

/** The largest hall, in cells a side. */
export const HALL_CAP = 24;

/** How many hatches the hall's south wall takes before a corridor is built. */
export const SOUTH_HATCHES = 8;

/** The smallest hall, so even an empty engram is a room to stand in. */
const MIN_WIDTH = 5;
const MIN_DEPTH = 6;

/** The rows a bay's doorway is open on. */
const BAY_DOOR_ROWS = [3, 4] as const;

/**
 * How many fixtures want each wall. `north`, `west`, `east` and `south` count
 * those asking for that hall wall and `any` those content with any hall or
 * bay wall; all of them decide the bays. `hatches` is the number of hatches,
 * which decides the corridor: past eight they go there instead of counting
 * towards `south`.
 */
export interface LayoutNeed {
  north: number;
  west: number;
  east: number;
  south: number;
  any: number;
  hatches: number;
}

/**
 * A floor plan: the grid, as wide and deep as all its parts together, the
 * rectangle of each part, and the two reserved slots of the entrance.
 */
export interface Layout {
  width: number;
  depth: number;
  grid: string[];
  hall: Rect;
  bays: Rect[];
  corridor: Rect | null;
  entrance: WallSlot;
  placard: WallSlot;
}

/** Which walls a fixture would like to stand against. */
export type SlotPref = "north" | "west" | "east" | "south" | "corridor" | "any";

/** The free wall slots of a layout, handed out by preference. */
export interface SlotPool {
  /** The first free slot the preference allows, or null when none is left. */
  take(pref: SlotPref): WallSlot | null;
}

function clamp(v: number, lo: number, hi: number) {
  return Math.max(lo, Math.min(hi, v));
}

function inside(r: Rect, x: number, y: number) {
  return x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1;
}

function slotKey(s: WallSlot) {
  return `${s.x},${s.y},${s.side}`;
}

/** True when the cell is floor; anything outside the grid is void. */
export function isFloor(
  grid: readonly string[],
  x: number,
  y: number,
): boolean {
  return grid[y]?.[x] === ".";
}

/** The plan with exactly `bayCount` bays. */
function build(need: LayoutNeed, bayCount: number): Layout {
  const hallWidth = clamp(
    Math.max(2 * need.north + 1, 2 * need.south + 3),
    MIN_WIDTH,
    HALL_CAP,
  );
  const hallDepth = clamp(
    2 * Math.max(need.west, need.east) + 2,
    MIN_DEPTH,
    HALL_CAP,
  );
  const corridorLength =
    need.hatches > SOUTH_HATCHES ? 2 * Math.ceil(need.hatches / 2) + 1 : 0;
  const ox = corridorLength > 0 ? corridorLength + 1 : 0;
  const hall: Rect = { x0: ox, y0: 0, x1: ox + hallWidth, y1: hallDepth };
  const corridor: Rect | null =
    corridorLength > 0
      ? {
          x0: 0,
          y0: hallDepth - CORRIDOR_WIDTH,
          x1: corridorLength,
          y1: hallDepth,
        }
      : null;
  const bays: Rect[] = [];
  for (let i = 0; i < bayCount; i++) {
    const x0 = hall.x1 + 1 + i * (BAY + 1);
    bays.push({ x0, y0: 0, x1: x0 + BAY, y1: BAY });
  }
  const width = bays.at(-1)?.x1 ?? hall.x1;
  const depth = Math.max(hallDepth, bayCount > 0 ? BAY : 0);

  const cells = Array.from({ length: depth }, () =>
    Array.from({ length: width }, () => " "),
  );
  const fill = (r: Rect) => {
    for (let y = r.y0; y < r.y1; y++)
      for (let x = r.x0; x < r.x1; x++) {
        const row = cells[y];
        if (row !== undefined) row[x] = ".";
      }
  };
  fill(hall);
  if (corridor !== null) {
    // The corridor and its doorway column, which is open on the same rows.
    fill({ ...corridor, x1: corridor.x1 + 1 });
  }
  for (const bay of bays) {
    fill(bay);
    for (const y of BAY_DOOR_ROWS) {
      const row = cells[y];
      if (row !== undefined) row[bay.x0 - 1] = ".";
    }
  }

  const spawnX = ox + Math.floor(hallWidth / 2);
  return {
    width,
    depth,
    grid: cells.map((row) => row.join("")),
    hall,
    bays,
    corridor,
    entrance: { x: spawnX, y: hallDepth - 1, side: "s" },
    placard: { x: spawnX - 1, y: hallDepth - 1, side: "s" },
  };
}

/**
 * The plan for a need: the hall, a corridor past eight hatches, and as few
 * bays as hold the rest. See the module doc for the rules.
 */
export function planLayout(need: LayoutNeed): Layout {
  const demand = need.north + need.west + need.east + need.south + need.any;
  let layout = build(need, 0);
  for (let bays = 1; bays <= MAX_BAYS; bays++) {
    if (hallAndBaySlots(layout).length >= demand) break;
    layout = build(need, bays);
  }
  return layout;
}

/**
 * Every slot of a grid: every second cell of every straight wall run, north
 * walls first, then east, south and west, each walked clockwise. See the
 * module doc for the rules.
 */
export function wallSlots(grid: readonly string[]): WallSlot[] {
  const depth = grid.length;
  const width = grid.reduce((w, row) => Math.max(w, row.length), 0);
  const slots: WallSlot[] = [];
  // Walks one line of cells in order and cuts it into runs of wall edges.
  const walk = (
    cells: [number, number][],
    side: Side,
    dx: number,
    dy: number,
  ) => {
    let n = 0;
    for (const [x, y] of cells) {
      if (isFloor(grid, x, y) && !isFloor(grid, x + dx, y + dy)) {
        if (n % 2 === 1) slots.push({ x, y, side });
        n++;
      } else {
        n = 0;
      }
    }
  };
  const range = (from: number, to: number) => {
    const out: number[] = [];
    if (from <= to) for (let i = from; i <= to; i++) out.push(i);
    else for (let i = from; i >= to; i--) out.push(i);
    return out;
  };
  for (let y = 0; y < depth; y++)
    walk(
      range(0, width - 1).map((x) => [x, y]),
      "n",
      0,
      -1,
    );
  for (let x = 0; x < width; x++)
    walk(
      range(0, depth - 1).map((y) => [x, y]),
      "e",
      1,
      0,
    );
  for (let y = 0; y < depth; y++)
    walk(
      range(width - 1, 0).map((x) => [x, y]),
      "s",
      0,
      1,
    );
  for (let x = 0; x < width; x++)
    walk(
      range(depth - 1, 0).map((y) => [x, y]),
      "w",
      -1,
      0,
    );
  return slots;
}

const SIDES: readonly Side[] = ["n", "e", "s", "w"];

/**
 * The hall's and the bays' slots in `any` order, entrance and placard left
 * out: the hall's walls north, east, south, west, then each bay's.
 */
function hallAndBaySlots(layout: Layout): WallSlot[] {
  const reserved = new Set([slotKey(layout.entrance), slotKey(layout.placard)]);
  const slots = wallSlots(layout.grid).filter((s) => !reserved.has(slotKey(s)));
  const bySide = (r: Rect) =>
    SIDES.flatMap((side) =>
      slots.filter((s) => s.side === side && inside(r, s.x, s.y)),
    );
  return [layout.hall, ...layout.bays].flatMap(bySide);
}

const HALL_SIDE: Record<"north" | "west" | "east" | "south", Side> = {
  north: "n",
  west: "w",
  east: "e",
  south: "s",
};

/** A pool over a layout's slots. See the module doc for the preferences. */
export function createSlotPool(layout: Layout): SlotPool {
  const reserved = new Set([slotKey(layout.entrance), slotKey(layout.placard)]);
  const all = wallSlots(layout.grid).filter((s) => !reserved.has(slotKey(s)));
  const inHall = (s: WallSlot) => inside(layout.hall, s.x, s.y);
  const inBay = (s: WallSlot) => layout.bays.some((b) => inside(b, s.x, s.y));
  const any = hallAndBaySlots(layout);
  // What is neither hall nor bay is the corridor and its doorway column.
  const corridor = all.filter((s) => !inHall(s) && !inBay(s));
  const used = new Set<string>();
  const firstFree = (list: readonly WallSlot[]) =>
    list.find((s) => !used.has(slotKey(s))) ?? null;
  return {
    take(pref) {
      let own: WallSlot | null = null;
      if (pref === "corridor") own = firstFree(corridor);
      else if (pref !== "any") {
        const side = HALL_SIDE[pref];
        own = firstFree(any.filter((s) => s.side === side && inHall(s)));
      }
      const slot = own ?? firstFree(any);
      if (slot !== null) used.add(slotKey(slot));
      return slot;
    },
  };
}
