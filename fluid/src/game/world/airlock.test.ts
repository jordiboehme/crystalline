/**
 * The airlock (M3 C24, C29): the round hand-built room, its lift to every
 * domain's bridge and its domain directory.
 */
import { describe, expect, it } from "vitest";

import { seedFor } from "../core/seed";
import { GAME_VERSION } from "../version";
import {
  AIRLOCK_CEILING,
  AIRLOCK_GRID,
  AXIS,
  BEACON_OFFSET,
  LOCKER_ROWS,
  RING,
  airlockRoom,
  type AirlockInput,
} from "./airlock";
import airlockSource from "./airlock.ts?raw";
import { CANNED_DOMAINS } from "./canned";
import { INTERIOR_CATALOGUE, interiorFootprint } from "./consoleRoom";
import airlockGolden from "./golden/airlock.json?raw";
import { isFloor } from "./layout";
import { LIFT_WORDS, SCREEN_LINES, byLabel, moreLine } from "./lifts";
import type { Fixture, RoomSpec } from "./types";
import { CELL } from "./units";

type Domains = NonNullable<AirlockInput["domains"]>;

/** `n` domains `d00`, `d01`, ..., every third one private. */
function domainsOf(n: number): Domains {
  return Array.from({ length: n }, (_, i) => ({
    name: `d${String(i).padStart(2, "0")}`,
    private: i % 3 === 0,
  }));
}

/** The room's lift fixture; the test fails when there is none. */
function liftOf(room: RoomSpec): Extract<Fixture, { kind: "lift" }> {
  const lift = room.fixtures.find((f) => f.kind === "lift");
  if (lift?.kind !== "lift") throw new Error("no lift");
  return lift;
}

/** The room's screen fixture; the test fails when there is none. */
function screenOf(room: RoomSpec): Extract<Fixture, { kind: "screen" }> {
  const screen = room.fixtures.find((f) => f.kind === "screen");
  if (screen?.kind !== "screen") throw new Error("no screen");
  return screen;
}

describe("the airlock (M3 C24)", () => {
  it("is round, lit and bare", () => {
    // Mutation caught: a square grid, corners cut in steps of more than
    // one cell a row (a notched square), a lopsided row, the dressing run
    // on the airlock, a dim or flickering zone, a floor cell left outside
    // every zone.
    const room = airlockRoom({ domains: CANNED_DOMAINS, here: null });
    expect(AIRLOCK_GRID).toEqual([
      "   .....   ",
      "  .......  ",
      " ......... ",
      "...........",
      "...........",
      "...........",
      "...........",
      "...........",
      " ......... ",
      "  .......  ",
      "   .....   ",
    ]);
    expect(room.grid).toEqual(AIRLOCK_GRID);
    expect([room.width, room.depth]).toEqual([11, 11]);
    // Round: every row centred, and each corner steps in one cell a row,
    // over at least three rows, both ways.
    const lefts = room.grid.map((row) => row.length - row.trimStart().length);
    room.grid.forEach((row, y) => {
      expect(row.trim().length + 2 * (lefts[y] ?? 0), `row ${String(y)}`).toBe(
        room.width,
      );
      if (y > 0)
        expect(Math.abs((lefts[y] ?? 0) - (lefts[y - 1] ?? 0))).toBeLessThan(2);
    });
    expect(lefts.filter((l) => l > 0).length).toBeGreaterThanOrEqual(6);
    expect(room.entrance).toEqual({ x: 5, y: 10 });
    expect(room.ceiling).toBe(AIRLOCK_CEILING);
    expect(room.space).toBe("airlock");
    expect(room.condition).toBe("clean");
    expect(room.seed).toBe(seedFor(GAME_VERSION, "airlock"));
    expect([room.domain, room.permalink]).toEqual(["", ""]);
    expect(room.props).toEqual([]);
    expect(room.heroes).toEqual([]);
    expect(room.curios).toEqual([]);
    expect(room.decor).toEqual([]);
    expect(room.scaffold).toEqual([]);
    expect(room.fixtures.map((f) => f.kind)).toEqual(["lift", "screen"]);
    expect(room.lights.length).toBeGreaterThan(0);
    for (const z of room.lights) {
      expect(z.special).toBe("steady");
      expect(z.level).toBeGreaterThanOrEqual(208);
    }
    let floor = 0;
    for (let y = 0; y < room.depth; y++)
      for (let x = 0; x < room.width; x++) {
        if (!isFloor(room.grid, x, y)) continue;
        floor += 1;
        const lit = room.lights.some(
          (z) => x >= z.x0 && x < z.x1 && y >= z.y0 && y < z.y1,
        );
        expect(lit, `cell ${String(x)},${String(y)}`).toBe(true);
      }
    expect(floor).toBe(97);
  });

  it("stands the lift, the hatch, the directory and the iris light on the room's axis", () => {
    // Mutation caught: a fixture or a fitting off the axis (the half cell
    // the first airlock was off by), the beacons not mirrored about it,
    // the directory drawn at the wall screen's size.
    const room = airlockRoom({ domains: CANNED_DOMAINS, here: null });
    const middle = room.width / 2;
    expect(AXIS + 0.5).toBe(middle);
    expect(liftOf(room).slot).toEqual({
      x: AXIS,
      y: room.depth - 1,
      side: "s",
    });
    const screen = screenOf(room);
    expect(screen.slot).toEqual({ x: AXIS, y: 0, side: "n" });
    expect(screen.large).toBe(true);
    const pieces = room.interior ?? [];
    const of = (kind: string) => pieces.filter((p) => p.kind === kind);
    const [hatch] = of("outer-hatch");
    expect(hatch).toMatchObject({ x: middle, y: 0, turn: 2, variant: 0 });
    expect(of("outer-hatch")).toHaveLength(1);
    const beacons = of("beacon");
    expect(beacons.map((b) => [b.variant, b.y, b.turn])).toEqual([
      [0, 0, 2],
      [1, 0, 2],
    ]);
    expect((beacons[0]?.x ?? NaN) - middle).toBeCloseTo(-BEACON_OFFSET, 9);
    expect((beacons[1]?.x ?? NaN) - middle).toBeCloseTo(BEACON_OFFSET, 9);
    expect(of("iris-light")).toEqual([
      expect.objectContaining({ x: middle, y: middle, turn: 0 }),
    ]);
  });

  it("carries the hatch's beacons, the iris light, the lockers and the ring (M3 C24)", () => {
    // Mutation caught: a fitting or a decal missing, the iris light hung
    // anywhere but at the ceiling, a locker off the walls' straight runs
    // or out of the grid, the ring off the floor's centre, a CYCLE
    // stencil outside the ring or not reading a word.
    const room = airlockRoom({ domains: CANNED_DOMAINS, here: null });
    const pieces = room.interior ?? [];
    expect(pieces.map((p) => p.kind).slice(0, 4)).toEqual([
      "outer-hatch",
      "beacon",
      "beacon",
      "iris-light",
    ]);
    expect(INTERIOR_CATALOGUE["iris-light"].top).toBe(AIRLOCK_CEILING);
    const lockers = pieces.filter((p) => p.kind === "suit-locker");
    expect(lockers).toHaveLength(2 * 2 * LOCKER_ROWS.length);
    for (const l of lockers) {
      const box = interiorFootprint(l);
      if (box === null) throw new Error("a locker with no footprint");
      // On the west or east wall, whose cell and its neighbours along the
      // wall are all floor: a straight run, clear of the steps.
      const x = l.x < 1 ? 0 : room.width - 1;
      expect([0, room.width]).toContain(l.x);
      const y = Math.floor(l.y);
      for (const dy of [-1, 0, 1])
        expect(isFloor(room.grid, x, y + dy), `${String(x)},${String(y)}`).toBe(
          true,
        );
      expect(box.z0).toBeGreaterThanOrEqual(y * CELL);
      expect(box.z1).toBeLessThanOrEqual((y + 1) * CELL);
    }
    // The wall pieces keep clear of each other: the beacons out past the
    // hatch's collar along the north wall, and no two lockers' boxes
    // overlap.
    const alongNorth = pieces
      .filter((p) => p.kind === "outer-hatch" || p.kind === "beacon")
      .map((p) => {
        const hw = INTERIOR_CATALOGUE[p.kind].width / 2;
        return [p.x * CELL - hw, p.x * CELL + hw] as const;
      })
      .sort((a, b) => a[0] - b[0]);
    expect(alongNorth).toHaveLength(3);
    for (let i = 1; i < alongNorth.length; i++)
      expect(alongNorth[i]![0]).toBeGreaterThan(alongNorth[i - 1]![1]);
    const boxes = lockers.map((l) => interiorFootprint(l)!);
    boxes.forEach((a, i) =>
      boxes.slice(i + 1).forEach((b) => {
        const apart =
          a.x1 <= b.x0 || b.x1 <= a.x0 || a.z1 <= b.z0 || b.z1 <= a.z0;
        expect(apart).toBe(true);
      }),
    );
    const middle = room.width / 2;
    const [ring, ...stencils] = room.decals;
    expect(ring).toMatchObject({
      kind: "ring",
      on: "floor",
      x: middle,
      y: middle,
      width: RING.width,
      length: RING.band,
    });
    expect(stencils).toHaveLength(2);
    for (const s of stencils) {
      expect(s).toMatchObject({ kind: "stencil", on: "floor", word: "cycle" });
      expect(s.stencil).toBeUndefined();
      const reach = Math.hypot(
        s.width / 2,
        Math.abs(s.y - middle) * CELL + s.length / 2,
      );
      expect(reach).toBeLessThan(RING.width / 2 - RING.band);
    }
    expect(stencils.map((s) => s.turn).sort()).toEqual([0, 2]);
  });

  it("lists every domain on the lift and on the directory, keys on the private ones", () => {
    // Mutation caught: the directory not capped, the lift capped too, the
    // cap one line off, the key on the wrong line.
    const three: Domains = [
      { name: "vault", private: true },
      { name: "Atlas", private: false },
      { name: "mill", private: true },
    ];
    const small = airlockRoom({ domains: three, here: "mill" });
    const lift = liftOf(small);
    expect(lift.slot).toEqual({ x: 5, y: 10, side: "s" });
    expect(lift.note).toBeNull();
    expect(lift.stops).toEqual([
      {
        label: "Atlas",
        to: { kind: "bridge", domain: "Atlas" },
        key: false,
        here: false,
      },
      {
        label: "mill",
        to: { kind: "bridge", domain: "mill" },
        key: true,
        here: true,
      },
      {
        label: "vault",
        to: { kind: "bridge", domain: "vault" },
        key: true,
        here: false,
      },
    ]);
    const screen = screenOf(small);
    expect(screen.slot).toEqual({ x: 5, y: 0, side: "n" });
    expect(screen.lines).toEqual([
      LIFT_WORDS.airlock,
      "Atlas",
      "mill",
      "vault",
    ]);
    expect(screen.keys).toEqual([2, 3]);

    // Eleven names and the heading fill the screen exactly; twelve do not.
    const eleven = screenOf(
      airlockRoom({ domains: domainsOf(11), here: null }),
    );
    expect(eleven.lines).toHaveLength(SCREEN_LINES);
    expect(eleven.lines.at(-1)).toBe("d10");
    const twelve = screenOf(
      airlockRoom({ domains: domainsOf(12), here: null }),
    );
    expect(twelve.lines).toHaveLength(SCREEN_LINES);
    expect(twelve.lines.at(-1)).toBe(moreLine(2));

    const thirty = domainsOf(30);
    const big = airlockRoom({ domains: thirty, here: null });
    const names = thirty.map((d) => d.name).sort(byLabel);
    const directory = screenOf(big);
    expect(directory.lines).toEqual([
      LIFT_WORDS.airlock,
      ...names.slice(0, 10),
      moreLine(20),
    ]);
    expect(directory.lines).toHaveLength(SCREEN_LINES);
    // d00, d03, d06 and d09 are private: lines 1, 4, 7 and 10.
    expect(directory.keys).toEqual([1, 4, 7, 10]);
    expect(liftOf(big).stops.map((s) => s.label)).toEqual(names);
  });

  it("says when the listing failed", () => {
    // Mutation caught: a failed listing drawn as an empty station (no
    // line), the note missing from the lift (its status line), an empty
    // listing drawn with stops, keys or a note.
    const room = airlockRoom({ domains: null, here: null });
    const lift = liftOf(room);
    expect(lift.stops).toEqual([]);
    expect(lift.note).toBe(LIFT_WORDS.domainError);
    const screen = screenOf(room);
    expect(screen.lines).toEqual([LIFT_WORDS.airlock, LIFT_WORDS.domainError]);
    expect(screen.keys).toEqual([]);
    // An empty listing is no failure: the heading alone, no note.
    const empty = airlockRoom({ domains: [], here: null });
    expect(liftOf(empty).note).toBeNull();
    expect(liftOf(empty).stops).toEqual([]);
    expect(screenOf(empty).lines).toEqual([LIFT_WORDS.airlock]);
    expect(screenOf(empty).keys).toEqual([]);
  });

  it("keeps airlock.ts on the generator side: no ui, render or session imports", () => {
    // Mutation caught: an import of `ui/`, `render/`, the session
    // (`../session`) or a session-side world module (`move`, `malfunction`,
    // `interact`, `box`, `arrival`, `station`) into the generator side.
    expect(airlockSource).not.toMatch(
      /\b(?:from|import)\s*\(?\s*["'](?:\.\.\/(?:ui|render|session)(?:\/[^"']*)?|\.\/(?:move|malfunction|interact|box|arrival|station))["']/,
    );
  });

  it("builds the same airlock twice and matches the golden byte for byte", () => {
    // Mutation caught: any change to the builder's output, key order included.
    const input: AirlockInput = { domains: CANNED_DOMAINS, here: null };
    expect(airlockRoom(input)).toEqual(airlockRoom(input));
    expect(JSON.stringify(airlockRoom(input), null, 2) + "\n").toBe(
      airlockGolden,
    );
  });
});
