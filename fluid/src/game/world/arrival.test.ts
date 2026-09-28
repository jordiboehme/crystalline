/**
 * The arrival box (2.6e C14): the police box a walk out of the console room
 * lands in, stood free near the bridge's entrance for that one visit, a
 * walkway all round it ("free-standing, always"), and the room re-dressed
 * round it through the shared forced-hero seam (C15).
 */

import { describe, expect, it } from "vitest";

import { circleOverlapsBox } from "../dev/spots";
import {
  ARRIVAL_LIFT_CLEARANCE,
  ARRIVAL_REACH,
  arrivalBoxCandidates,
  placeArrivalBox,
  withArrivalBox,
} from "./arrival";
import { CANNED_BRIDGE, CANNED_HUB, CANNED_WORKSHOP } from "./canned";
import {
  HERO_FRONT,
  heroFootprint,
  propFootprint,
  turnedBox,
} from "./footprints";
import { generateRoom, withHeroes } from "./generate";
import {
  HERO_CLEAR,
  HERO_USE_OUT,
  HERO_WALKWAY,
  heroUsePoint,
  standsFree,
} from "./heroes";
import { blockersFor } from "./move";
import { WALL_PROP_DEPTH } from "./props";
import { dressingSites, fitsFloor, grow, overlaps } from "./sites";
import type { Box, Fixture, Hero, PlaceInput, Prop, RoomSpec } from "./types";
import { CELL } from "./units";

/**
 * The statuses the sweep cycles through: every condition once in four
 * rooms.
 */
const STATUSES = ["stable", "draft", "archived", "deprecated"] as const;

/**
 * The share of the landing sweep's rooms the box lands in, less 0.03. The
 * probe (2.6e Task 13) measured the free-standing box on the fifteen
 * layouts of the police box's sweep, every type and condition, 1500 rooms:
 * 1485 landed (0.99); the 15 that did not are drafts of the 5 by 6 bridge,
 * whose one open side of the entrance a scaffold frame fills. The 500
 * reseeds of the canned bridge land in all 500.
 */
const MEASURED_MINUS_MARGIN = 0.96;

/**
 * A bridge-typed place whose hall is at its narrowest (no relations, so
 * the width comes from the south wall) and whose south wall holds `n`
 * hatches, eight the most it takes before a corridor: the entrance's
 * neighbours are as crowded as a hall gets.
 */
function narrowWithHatches(n = 8): PlaceInput {
  const inbound = Array.from({ length: n }, (_, i) => ({
    address: { domain: CANNED_BRIDGE.domain, permalink: `in-${String(i)}` },
    title: `In ${String(i)}`,
    relType: "relates_to",
  }));
  return {
    ...CANNED_BRIDGE,
    permalink: `narrow-${String(n)}`,
    relations: [],
    links: [],
    inbound,
    inboundTotal: n,
  };
}

/**
 * The layouts of the landing sweep beyond the canned bridge's reseeds: the
 * narrow hall with 0, 4 and 8 hatches (5, 11 and 19 cells wide), the
 * bridge with one relation (5 by 6, split by its entrance lane), the
 * workshop and the hub with some of their ways and with all of them.
 */
function sweepLayouts(): PlaceInput[] {
  return [
    narrowWithHatches(0),
    narrowWithHatches(4),
    narrowWithHatches(8),
    { ...CANNED_BRIDGE, relations: CANNED_BRIDGE.relations.slice(0, 1) },
    { ...CANNED_WORKSHOP, relations: CANNED_WORKSHOP.relations.slice(0, 2) },
    CANNED_WORKSHOP,
    {
      ...CANNED_HUB,
      relations: CANNED_HUB.relations.slice(0, 8),
      inbound: CANNED_HUB.inbound.slice(0, 8),
      inboundTotal: 8,
    },
    CANNED_HUB,
  ];
}

const circleOverlaps = circleOverlapsBox; // from dev/spots.ts

/** The floor of one cell, in metres, as a scaffold box. */
function cellBox(x: number, y: number): Box {
  return { x0: x * CELL, x1: (x + 1) * CELL, z0: y * CELL, z1: (y + 1) * CELL };
}

/** The box a wall prop stands in: its whole edge, the wall band deep. */
function wallPropBox(p: Prop): Box {
  return turnedBox(p.x, p.y, p.turn, {
    a0: -CELL / 2,
    a1: CELL / 2,
    d0: 0,
    d1: WALL_PROP_DEPTH,
  });
}

/**
 * What stands in the walkway round the box at `index` in `room` (its
 * footprint grown by `HERO_WALKWAY`), by name: the hall's walls or its
 * edge, a taken box (fixture, decor, scaffold), another hero, a floor or
 * wall prop, a curio. Empty when the box stands free.
 */
function inWalkway(room: RoomSpec, index: number): string[] {
  const h = room.heroes[index];
  if (h === undefined) return ["no box"];
  const walk = grow(heroFootprint(h), HERO_WALKWAY);
  const out: string[] = [];
  const hall = room.hall;
  if (!(
    walk.x0 >= hall.x0 * CELL &&
    walk.x1 <= hall.x1 * CELL &&
    walk.z0 >= hall.y0 * CELL &&
    walk.z1 <= hall.y1 * CELL &&
    fitsFloor(room, walk)
  ))
    out.push("wall");
  if (dressingSites(room).taken.some((t) => overlaps(walk, t)))
    out.push("taken");
  room.heroes.forEach((o, i) => {
    if (i !== index && overlaps(walk, heroFootprint(o))) out.push(o.kind);
  });
  for (const p of room.props) {
    const f = p.anchor === "wall" ? wallPropBox(p) : propFootprint(p);
    if (f !== null && overlaps(walk, f)) out.push(p.kind);
  }
  for (const c of room.curios) {
    const x = c.x * CELL;
    const z = c.y * CELL;
    if (x > walk.x0 && x < walk.x1 && z > walk.z0 && z < walk.z1)
      out.push(c.kind);
  }
  return out;
}

describe("the arrival box (2.6e C14)", () => {
  it("offers the hall's half-cell points near the entrance, nearest first, east first at a tie", () => {
    // Mutation caught: `ARRIVAL_REACH` ignored (a point across the hall
    // offered), the points unsorted, or west before east at a tie.
    for (const place of [CANNED_BRIDGE, CANNED_WORKSHOP, narrowWithHatches()]) {
      const room = generateRoom(place);
      const list = arrivalBoxCandidates(room);
      expect(list.length).toBeGreaterThan(0);
      const ex = room.entrance.x + 0.5;
      const ey = room.entrance.y + 1;
      const far = list.map((c) => Math.hypot(c.x - ex, c.y - ey));
      for (const [i, c] of list.entries()) {
        expect(far[i]).toBeLessThanOrEqual(ARRIVAL_REACH + 1e-9);
        expect(c.x * 2).toBe(Math.round(c.x * 2));
        expect(c.y * 2).toBe(Math.round(c.y * 2));
        expect(c.x).toBeGreaterThanOrEqual(room.hall.x0);
        expect(c.x).toBeLessThanOrEqual(room.hall.x1);
        expect(c.y).toBeGreaterThanOrEqual(room.hall.y0);
        expect(c.y).toBeLessThanOrEqual(room.hall.y1);
        const next = list[i + 1];
        const nf = far[i + 1];
        if (next === undefined || nf === undefined) continue;
        expect(nf).toBeGreaterThanOrEqual((far[i] ?? 0) - 1e-9);
        if (Math.abs(nf - (far[i] ?? 0)) < 1e-9)
          expect(next.x).toBeLessThan(c.x);
      }
    }
  });

  it("stands the box free beside the entrance on the canned bridge, facing into the hall, the player stepping out of its front", () => {
    // Mutation caught: `standsFree` dropped (the box against the south
    // wall, the nearest candidates), the walkway measured without the
    // wall band, a turn other than north, or a spawn behind the box or
    // turned away from the hall.
    const built = generateRoom(CANNED_BRIDGE);
    const { room, box, spawn } = withArrivalBox(CANNED_BRIDGE, built);
    expect(box).not.toBeNull();
    const h = room.heroes[box!]!;
    expect(h.kind).toBe("police-box");
    expect(h.turn).toBe(0);
    const fp = heroFootprint(h);
    expect(standsFree(room, fp)).toBe(true);
    // Clear of the south wall by the walkway and the wall band.
    expect(room.hall.y1 * CELL - fp.z1).toBeGreaterThanOrEqual(
      HERO_WALKWAY + WALL_PROP_DEPTH - 1e-9,
    );
    expect(inWalkway(room, box!)).toEqual([]);
    expect(
      Math.hypot(h.x - (room.entrance.x + 0.5), h.y - (room.entrance.y + 1)),
    ).toBeLessThanOrEqual(ARRIVAL_REACH);
    // The front faces into the hall: towards its centre.
    const [fx, fz] = HERO_FRONT[h.turn] ?? [0, -1];
    const cy = (room.hall.y0 + room.hall.y1) / 2;
    expect(fx * 0 + fz * (cy - h.y)).toBeGreaterThan(0);
    // The player steps out of the front, facing the way the box faces.
    expect(spawn?.yaw).toBe(0);
    expect(spawn?.x).toBeCloseTo(h.x * CELL);
    expect(fp.z0 - (spawn?.z ?? 0)).toBeCloseTo(HERO_USE_OUT);
    expect(heroUsePoint(h)).toEqual({ x: spawn?.x, z: spawn?.z });
  });

  it("places the box clear of every way, or not at all, on crowded halls (Review Focus 3)", () => {
    // Mutation caught: a lane, a fixture footprint or the scaffold ignored,
    // or a throw on a hall with no room.
    const places = [
      CANNED_HUB,
      { ...CANNED_BRIDGE, status: "draft" },
      narrowWithHatches(),
      {
        ...CANNED_BRIDGE,
        relations: CANNED_BRIDGE.relations.slice(0, 1),
        permalink: "manifest-5",
        status: "draft",
      },
    ];
    let none = 0;
    for (const place of places) {
      const built = generateRoom(place);
      const out = withArrivalBox(place, built);
      if (out.box === null) {
        none++;
        expect(out.room).toBe(built);
        expect(out.spawn).toBeNull();
        continue;
      }
      const h = out.room.heroes[out.box]!;
      const fp = heroFootprint(h);
      const sites = dressingSites(built);
      expect(sites.lanes.some((l) => overlaps(l, fp))).toBe(false);
      expect(sites.taken.some((t) => overlaps(t, grow(fp, HERO_CLEAR)))).toBe(
        false,
      );
      expect(inWalkway(out.room, out.box)).toEqual([]);
      const s = out.spawn!;
      expect(
        blockersFor(out.room).some((b) => circleOverlaps(s.x, s.z, b)),
      ).toBe(false);
    }
    // The drafted 5 by 6 bridge has no spot: the null branch runs.
    expect(none).toBeGreaterThan(0);
  });

  it("steps past a lane, a frame in its moat or a pipe run over the first spot, and places nothing when every spot is blocked (Review Focus 3)", () => {
    // Mutation caught: the lane check, the moat check (a frame read
    // against the box's floor alone) or the pipe-run check dropped (the
    // box lands at the first spot, the obstacle in its walkway or through
    // its roof), or a throw, a changed room or a spawn when nothing fits.
    const built = generateRoom(CANNED_BRIDGE);
    const first = placeArrivalBox(built);
    expect(first).not.toBeNull();
    const fp = heroFootprint(first!);
    const cx = Math.floor(first!.x);
    const cy = Math.floor(first!.y - 0.5);
    // A hatch planted on the west edge of the cell the box's centre is
    // in: flush, so it takes no floor, but its lane runs over the box.
    const hatch: Fixture = {
      kind: "hatch",
      slot: { x: cx, y: cy, side: "w" },
      label: "In lane",
      address: { domain: CANNED_BRIDGE.domain, permalink: "in-lane" },
      seed: 1,
    };
    const laned: RoomSpec = { ...built, fixtures: [...built.fixtures, hatch] };
    expect(dressingSites(laned).lanes.some((l) => overlaps(l, fp))).toBe(true);
    // A thin frame 0.5 m off the box's east side: in its moat and its
    // walkway, off its floor and off the spawn.
    const frame: Box = {
      x0: fp.x1 + 0.5,
      x1: fp.x1 + 0.6,
      z0: fp.z0,
      z1: fp.z1,
    };
    expect(overlaps(frame, fp)).toBe(false);
    // A pipe run hung over the box, along x.
    const pipe = {
      kind: "pipe-run" as const,
      x: first!.x,
      y: first!.y,
      turn: 0,
      seed: 3,
    };
    const cases: [string, RoomSpec][] = [
      ["lane", laned],
      ["frame", { ...built, scaffold: [...built.scaffold, frame] }],
      ["pipe", { ...built, decor: [...built.decor, pipe] }],
    ];
    for (const [name, room] of cases) {
      const got = placeArrivalBox(room);
      expect(got, name).not.toBeNull();
      expect(
        got!.x !== first!.x || got!.y !== first!.y,
        `${name}: still on the first spot`,
      ).toBe(true);
      const moved = heroFootprint(got!);
      expect(dressingSites(room).lanes.some((l) => overlaps(l, moved))).toBe(
        false,
      );
      if (name === "frame")
        expect(overlaps(grow(moved, HERO_CLEAR), frame)).toBe(false);
    }

    const walled: RoomSpec = {
      ...built,
      scaffold: [
        ...built.scaffold,
        ...arrivalBoxCandidates(built).map((c) =>
          cellBox(Math.floor(c.x - 0.25), Math.floor(c.y - 0.25)),
        ),
      ],
    };
    let none: ReturnType<typeof withArrivalBox> | undefined;
    expect(() => {
      none = withArrivalBox(CANNED_BRIDGE, walled);
    }).not.toThrow();
    expect(none?.box).toBeNull();
    expect(none?.spawn).toBeNull();
    expect(none?.room).toBe(walled);
    expect(placeArrivalBox(walled)).toBeNull();
  });

  it("drops the room's own heroes that would crowd the box, and keeps the rest", () => {
    // Mutation caught: a crowding hero kept (two heroes inside each other's
    // moat), a far hero dropped, or the room not re-dressed round the box.
    const place = CANNED_BRIDGE;
    const built = generateRoom(place);
    const first = placeArrivalBox(built);
    expect(first).not.toBeNull();
    const box = heroFootprint(first!);
    // A hero of the room's own, planted right beside the box, and one far away.
    const crowd: Hero = {
      kind: "turret",
      variant: 0,
      x: first!.x - 1,
      y: first!.y - 1,
      turn: 0,
      seed: 11,
    };
    const far: Hero = {
      kind: "turret",
      variant: 0,
      x: built.hall.x0 + 1.5,
      y: built.hall.y0 + 1.5,
      turn: 0,
      seed: 12,
    };
    expect(overlaps(grow(box, HERO_CLEAR), heroFootprint(crowd))).toBe(true);
    expect(overlaps(grow(box, HERO_CLEAR), heroFootprint(far))).toBe(false);
    const withTwo = withHeroes(place, built, [crowd, far]);
    const out = withArrivalBox(place, withTwo);
    expect(out.box).not.toBeNull();
    expect(out.room.heroes).toContainEqual(far);
    expect(out.room.heroes).not.toContainEqual(crowd);
    expect(out.room).toEqual(
      withHeroes(place, withTwo, [far, out.room.heroes[out.box!]!]),
    );
  });

  it("re-dresses the room round the box, so no floor prop stays in its moat", () => {
    // Mutation caught: the box stood in the room without the dressing run
    // again, its props kept as they were drawn round the room's own heroes.
    const place: PlaceInput = {
      ...narrowWithHatches(0),
      permalink: "narrow-0",
    };
    const built = generateRoom(place);
    const placed = placeArrivalBox(built);
    expect(placed).not.toBeNull();
    const moat = grow(heroFootprint(placed!), HERO_CLEAR);
    const inMoat = (room: RoomSpec) =>
      room.props.some((p) => {
        const f = propFootprint(p);
        return f !== null && overlaps(f, moat);
      });
    // The narrow hall's own dressing stands a crate where the box goes.
    expect(inMoat(built)).toBe(true);
    const out = withArrivalBox(place, built);
    expect(out.box).not.toBeNull();
    expect(inMoat(out.room)).toBe(false);
  });

  it("lands free on the share of reseeded bridges and crowded halls the probe measured, nothing in its walkway", () => {
    // Mutation caught: a candidate rule that rejects almost everything, or
    // one that lets anything into the walkway on some layout (a wall, a
    // fixture, a scaffold frame, a prop, a hero or a curio).
    let landed = 0;
    let rooms = 0;
    const check = (place: PlaceInput) => {
      rooms++;
      const out = withArrivalBox(place, generateRoom(place));
      if (out.box === null) return;
      landed++;
      const h = out.room.heroes[out.box]!;
      expect(h.turn).toBe(0);
      expect(inWalkway(out.room, out.box), place.permalink).toEqual([]);
      const s = out.spawn!;
      expect(
        blockersFor(out.room).some((b) => circleOverlaps(s.x, s.z, b)),
        place.permalink,
      ).toBe(false);
    };
    for (let i = 0; i < 500; i++)
      check({
        ...CANNED_BRIDGE,
        permalink: `manifest-${String(i)}`,
        status: STATUSES[i % 4] ?? null,
      });
    for (const layout of sweepLayouts())
      for (let i = 0; i < 20; i++)
        check({
          ...layout,
          permalink: `${layout.permalink}-${String(i)}`,
          status: STATUSES[i % 4] ?? null,
        });
    expect(rooms).toBe(660);
    expect(landed / rooms).toBeGreaterThanOrEqual(MEASURED_MINUS_MARGIN);
  }, 30_000);

  it("lands clear of the lift's column, off the path between the lift and the hall's centre (Jordi, M4)", () => {
    // Mutation caught: the lift-clearance filter dropped, shrunk below two
    // cells, or skipped for the "does not stand between" half of the rule,
    // letting the box land beside the lift again and block the sightline
    // between the lift and the room.
    const wide = [
      CANNED_BRIDGE,
      CANNED_WORKSHOP,
      CANNED_HUB,
      narrowWithHatches(4),
      narrowWithHatches(8),
    ];
    let checked = 0;
    for (const place of wide) {
      const room = generateRoom(place);
      const out = withArrivalBox(place, room);
      expect(out.box, place.permalink).not.toBeNull();
      const h = out.room.heroes[out.box!]!;
      checked++;
      const liftX = room.entrance.x + 0.5;
      const liftY = room.entrance.y + 1;
      const centreX = (room.hall.x0 + room.hall.x1) / 2;
      const centreY = (room.hall.y0 + room.hall.y1) / 2;
      expect(Math.abs(h.x - liftX), place.permalink).toBeGreaterThanOrEqual(
        ARRIVAL_LIFT_CLEARANCE - 1e-9,
      );
      const betweenX =
        h.x >= Math.min(liftX, centreX) - 1e-9 &&
        h.x <= Math.max(liftX, centreX) + 1e-9;
      const betweenY =
        h.y >= Math.min(liftY, centreY) - 1e-9 &&
        h.y <= Math.max(liftY, centreY) + 1e-9;
      expect(betweenX && betweenY, place.permalink).toBe(false);
    }
    expect(checked).toBe(wide.length);
    // A hall too narrow to hold the clearance (5 cells wide, less than
    // twice ARRIVAL_LIFT_CLEARANCE) still lands the box, falling back to
    // the plain nearest free candidate: the rule never turns a landing
    // that used to succeed into a null.
    const narrow = generateRoom(narrowWithHatches(0));
    expect(narrow.hall.x1 - narrow.hall.x0).toBeLessThan(5.5);
    const out = withArrivalBox(narrowWithHatches(0), narrow);
    expect(out.box).not.toBeNull();
  });
});
