/**
 * The arrival box (2.6e C14): the police box a walk out of the console room
 * lands in, stood by the bridge's entrance for that one visit, and the
 * room re-dressed round it through the shared forced-hero seam (C15).
 */

import { describe, expect, it } from "vitest";

import { circleOverlapsBox } from "../dev/spots";
import {
  arrivalBoxCandidates,
  placeArrivalBox,
  withArrivalBox,
} from "./arrival";
import { CANNED_BRIDGE, CANNED_HUB, CANNED_WORKSHOP } from "./canned";
import { heroFootprint, propFootprint } from "./footprints";
import { generateRoom, withHeroes } from "./generate";
import { HERO_CLEAR } from "./heroes";
import { blockersFor } from "./move";
import { dressingSites, edgeKey, grow, overlaps } from "./sites";
import type { Box, Fixture, Hero, PlaceInput, RoomSpec } from "./types";
import { CELL } from "./units";

/** The statuses the sweep cycles through: every condition once in four rooms. */
const STATUSES = ["stable", "draft", "archived", "deprecated"] as const;

/**
 * The share of the 500 reseeded bridges the box lands in, less 0.03. The
 * probe measured 500 of 500 (1.0), every one at the entrance's east
 * neighbour: the reseeds share the canned bridge's ways, so its layout,
 * and vary only its seed and its condition (decor, scaffold, heroes).
 */
const MEASURED_MINUS_MARGIN = 0.97;

/**
 * A bridge-typed place whose hall is at its narrowest (no relations, so
 * the width comes from the south wall) and whose south wall holds eight
 * hatches, the most it takes before a corridor: the entrance's neighbours
 * are as crowded as a hall gets.
 */
function narrowWithHatches(): PlaceInput {
  const inbound = Array.from({ length: 8 }, (_, i) => ({
    address: { domain: CANNED_BRIDGE.domain, permalink: `in-${String(i)}` },
    title: `In ${String(i)}`,
    relType: "relates_to",
  }));
  return {
    ...CANNED_BRIDGE,
    permalink: "narrow",
    relations: [],
    links: [],
    inbound,
    inboundTotal: 8,
  };
}

const circleOverlaps = circleOverlapsBox; // from dev/spots.ts

/** The floor of one cell, in metres, as a scaffold box. */
function cellBox(x: number, y: number): Box {
  return { x0: x * CELL, x1: (x + 1) * CELL, z0: y * CELL, z1: (y + 1) * CELL };
}

describe("the arrival box (2.6e C14)", () => {
  it("orders the south row by distance from the entrance, east first, skipping the entrance, the placard and every fixture's edge", () => {
    // Mutation caught: west before east, the entrance or the placard offered,
    // a hatch's edge offered.
    for (const place of [CANNED_BRIDGE, CANNED_WORKSHOP, narrowWithHatches()]) {
      const room = generateRoom(place);
      const list = arrivalBoxCandidates(room);
      expect(list.length).toBeGreaterThan(0);
      const e = room.entrance;
      const held = new Set(room.fixtures.map((f) => edgeKey(f.slot)));
      for (const c of list) {
        expect(c.side).toBe("s");
        expect(c.y).toBe(room.hall.y1 - 1);
        expect(c.x).not.toBe(e.x);
        expect(c.x).not.toBe(e.x - 1);
        expect(held.has(edgeKey(c))).toBe(false);
      }
      const dist = list.map((c) => Math.abs(c.x - e.x));
      expect([...dist].sort((a, b) => a - b)).toEqual(dist);
      list.forEach((c, i) => {
        const next = list[i + 1];
        if (
          next !== undefined &&
          Math.abs(next.x - e.x) === Math.abs(c.x - e.x)
        )
          expect(c.x).toBeGreaterThan(e.x);
      });
    }
  });

  it("stands the box beside the entrance on the canned bridge, facing into the hall", () => {
    const { room, box, spawn } = withArrivalBox(
      CANNED_BRIDGE,
      generateRoom(CANNED_BRIDGE),
    );
    expect(box).not.toBeNull();
    const h = room.heroes[box!]!;
    expect(h.kind).toBe("police-box");
    expect(h.turn).toBe(0);
    expect(Math.abs(h.x - (room.entrance.x + 0.5))).toBeLessThanOrEqual(3);
    expect(spawn?.yaw).toBe(0);
  });

  it("places the box clear of every way, or not at all, on crowded south walls (Review Focus 3)", () => {
    // Mutation caught: a lane, a fixture footprint or the scaffold ignored,
    // or a throw on a wall with no room.
    const places = [
      CANNED_HUB,
      { ...CANNED_BRIDGE, status: "draft" },
      narrowWithHatches(),
    ];
    for (const place of places) {
      const built = generateRoom(place);
      const out = withArrivalBox(place, built);
      if (out.box === null) {
        expect(out.room).toBe(built);
        expect(out.spawn).toBeNull();
        continue;
      }
      const h = out.room.heroes[out.box]!;
      const fp = heroFootprint(h);
      const sites = dressingSites(built);
      expect(sites.lanes.some((l) => overlaps(l, fp))).toBe(false);
      expect(sites.taken.some((t) => overlaps(t, fp))).toBe(false);
      const s = out.spawn!;
      expect(
        blockersFor(out.room).some((b) => circleOverlaps(s.x, s.z, b)),
      ).toBe(false);
    }
  });

  it("steps past a lane, a taken box or a blocked spawn over the first candidate, and places nothing when every candidate is blocked (Review Focus 3)", () => {
    // Mutation caught: the lane check, the taken-box check (a fixture's
    // footprint, the furniture or a scaffold frame) or the spawn's check
    // dropped (the box or its spawn lands on the obstacle), or a throw, a
    // changed room or a spawn when no candidate fits. The generated rooms
    // above all land at the first candidate, clear of everything, so
    // neither branch shows there; each obstacle here covers only the part
    // its own check reads.
    const built = generateRoom(CANNED_BRIDGE);
    const list = arrivalBoxCandidates(built);
    expect(list.length).toBeGreaterThan(1);
    const first = list[0]!;
    const cell = cellBox(first.x, first.y);
    // The candidate's south wall runs along the cell's south side.
    const wall = cell.z1;
    // A hatch planted on the cell's west edge: flush, so it takes no
    // floor, but its lane covers the cell and the box's floor with it.
    const hatch: Fixture = {
      kind: "hatch",
      slot: { x: first.x, y: first.y, side: "w" },
      label: "In lane",
      address: { domain: CANNED_BRIDGE.domain, permalink: "in-lane" },
      seed: 1,
    };
    const laned: RoomSpec = { ...built, fixtures: [...built.fixtures, hatch] };
    // A frame over the box's floor by the wall only, short of the spawn.
    const low: Box = { ...cell, z0: wall - 0.5 };
    // A frame over the spawn only, short of the box's floor.
    const out: Box = { ...cell, z0: wall - 2.0, z1: wall - 1.5 };
    const cases: [RoomSpec, Box | null][] = [
      [laned, null],
      [{ ...built, scaffold: [...built.scaffold, low] }, low],
      [{ ...built, scaffold: [...built.scaffold, out] }, out],
    ];
    for (const [room, frame] of cases) {
      const got = withArrivalBox(CANNED_BRIDGE, room);
      expect(got.box).not.toBeNull();
      const h = got.room.heroes[got.box!]!;
      expect(h.x).toBe(list[1]!.x + 0.5);
      const fp = heroFootprint(h);
      expect(dressingSites(room).lanes.some((l) => overlaps(l, fp))).toBe(
        false,
      );
      const s = got.spawn!;
      if (frame !== null) {
        expect(overlaps(fp, frame)).toBe(false);
        expect(circleOverlaps(s.x, s.z, frame)).toBe(false);
      }
      expect(
        blockersFor(got.room).some((b) => circleOverlaps(s.x, s.z, b)),
      ).toBe(false);
    }

    const walled: RoomSpec = {
      ...built,
      scaffold: [...built.scaffold, ...list.map((c) => cellBox(c.x, c.y))],
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
    // A hero of the room's own, planted right beside the box's cell, and one far away.
    const crowd: Hero = {
      kind: "turret",
      variant: 0,
      x: first!.x + 1,
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
    const built = generateRoom(CANNED_HUB);
    const placed = placeArrivalBox(built);
    expect(placed).not.toBeNull();
    const moat = grow(heroFootprint(placed!), HERO_CLEAR);
    const inMoat = (room: RoomSpec) =>
      room.props.some((p) => {
        const f = propFootprint(p);
        return f !== null && overlaps(f, moat);
      });
    // The hub's own dressing stands a crate stack where the box goes.
    expect(inMoat(built)).toBe(true);
    const out = withArrivalBox(CANNED_HUB, built);
    expect(out.box).not.toBeNull();
    expect(inMoat(out.room)).toBe(false);
  });

  it("lands on the share of reseeded bridges the probe measured", () => {
    // Mutation caught: a candidate rule that rejects almost everything.
    let landed = 0;
    for (let i = 0; i < 500; i++) {
      const place: PlaceInput = {
        ...CANNED_BRIDGE,
        permalink: `manifest-${String(i)}`,
        status: STATUSES[i % 4] ?? null,
      };
      if (withArrivalBox(place, generateRoom(place)).box !== null) landed++;
    }
    expect(landed / 500).toBeGreaterThanOrEqual(MEASURED_MINUS_MARGIN);
  }, 30_000);
});
