/**
 * The runtime seams a live change runs on a built room (M4 C16, C17, C19):
 * the room diff, where the player stands after a reshape, the room gone
 * dark in place and the light curve of a flicker and a dip.
 */

import { describe, expect, it } from "vitest";

import { seedFor } from "../core/seed";
import { MANIFEST_PERMALINK } from "../paths";
import { freeEdgeTest } from "./bridge";
import { CANNED_WORKSHOP } from "./canned";
import {
  DIP_MS,
  DIP_SWAP_MS,
  FLICKER_MS,
  darkened,
  diffRooms,
  dipFactor,
  settleSpot,
} from "./diff";
import { generateRoom } from "./generate";
import { ARRIVAL_DISTANCE, wallPoint } from "./interact";
import { isFloor, wallRuns, wallSlots } from "./layout";
import { LIFT_WORDS } from "./lifts";
import { blockersFor } from "./move";
import { arrivalPoint, reach } from "./reachChecks";
import { edgeKey } from "./sites";
import type { Box, Fixture, RoomSpec } from "./types";
import { CELL } from "./units";

/** True when the point lies inside the box, edges included. */
function inBox(x: number, z: number, b: Box): boolean {
  return x >= b.x0 && x <= b.x1 && z >= b.z0 && z <= b.z1;
}

/** The centre of a cell, in metres. */
function centre(cx: number, cy: number) {
  return { x: (cx + 0.5) * CELL, z: (cy + 0.5) * CELL };
}

const SLOT = { x: 1, y: 0, side: "n" } as const;

/**
 * One fixture of every kind and every edit C16 counts as text on it. Keyed
 * by the fixture union's own `kind`, so a new kind is a type error here
 * until its text fields are listed. A kind with no text (a machine) lists
 * no edit. Each edit changes one field on a copy of the fixture.
 */
const TEXT_EDITS = {
  terminal: {
    fixture: {
      kind: "terminal",
      slot: SLOT,
      heading: "HEAD",
      lines: ["a"],
      section: 0,
      seed: 1,
    },
    edits: [
      (f) => {
        if (f.kind === "terminal") f.heading = "OTHER";
      },
      (f) => {
        if (f.kind === "terminal") f.lines = ["b", "c"];
      },
    ],
  },
  door: {
    fixture: {
      kind: "door",
      slot: SLOT,
      style: "sliding",
      relType: "depends_on",
      label: "DOOR",
      address: { domain: "eng", permalink: "b" },
      sealedLabel: null,
      seed: 2,
    },
    edits: [
      (f) => {
        if (f.kind === "door") f.label = "OTHER";
      },
    ],
  },
  portal: {
    fixture: {
      kind: "portal",
      slot: SLOT,
      label: "PORTAL",
      address: { domain: "ops", permalink: "b" },
      crossDomain: true,
      sealedLabel: null,
      seed: 3,
    },
    edits: [
      (f) => {
        if (f.kind === "portal") f.label = "OTHER";
      },
    ],
  },
  hatch: {
    fixture: {
      kind: "hatch",
      slot: SLOT,
      label: "HATCH",
      address: { domain: "eng", permalink: "c" },
      seed: 4,
    },
    edits: [
      (f) => {
        if (f.kind === "hatch") f.label = "OTHER";
      },
    ],
  },
  machine: {
    fixture: {
      kind: "machine",
      slot: SLOT,
      machine: "workbench",
      tag: "tag",
      hue: 0,
      seed: 5,
    },
    edits: [],
  },
  poster: {
    fixture: {
      kind: "poster",
      slot: SLOT,
      category: "warning",
      lines: ["a"],
      seed: 6,
    },
    edits: [
      (f) => {
        if (f.kind === "poster") f.lines = ["b"];
      },
    ],
  },
  placard: {
    fixture: { kind: "placard", slot: SLOT, lines: ["a"] },
    edits: [
      (f) => {
        if (f.kind === "placard") f.lines = ["b", "c"];
      },
    ],
  },
  lift: {
    fixture: {
      kind: "lift",
      slot: SLOT,
      stops: [
        { label: "STOP", to: { kind: "airlock" }, key: false, here: false },
      ],
      note: null,
      seed: 7,
    },
    edits: [
      (f) => {
        const stop = f.kind === "lift" ? f.stops[0] : undefined;
        if (stop !== undefined) stop.label = "OTHER";
      },
      (f) => {
        if (f.kind === "lift") f.note = "NOTE";
      },
    ],
  },
  screen: {
    fixture: {
      kind: "screen",
      slot: SLOT,
      lines: ["a"],
      keys: [],
      seed: 8,
    },
    edits: [
      (f) => {
        if (f.kind === "screen") f.lines = ["b"];
      },
    ],
  },
  exit: {
    fixture: {
      kind: "exit",
      slot: SLOT,
      label: "EXIT",
      to: { kind: "airlock" },
      seed: 9,
    },
    edits: [
      (f) => {
        if (f.kind === "exit") f.label = "OTHER";
      },
    ],
  },
} satisfies Record<
  Fixture["kind"],
  { fixture: Fixture; edits: ((f: Fixture) => void)[] }
>;

describe("the room diff (M4 C16)", () => {
  it("tells same, text and shape apart", () => {
    // Mutation caught: a label change read as a shape change (a reshape per
    // typo), a door added read as text, a seed change blanked.
    const room = generateRoom(CANNED_WORKSHOP);
    expect(diffRooms(room, structuredClone(room))).toBe("same");

    const relabelled = structuredClone(room);
    const terminal = relabelled.fixtures.find((f) => f.kind === "terminal");
    expect(terminal).toBeDefined();
    if (terminal?.kind === "terminal") terminal.lines = ["CHANGED"];
    expect(diffRooms(room, relabelled)).toBe("text");

    const retitled = structuredClone(room);
    retitled.title = "Another Workshop";
    const door = retitled.fixtures.find((f) => f.kind === "door");
    expect(door).toBeDefined();
    if (door?.kind === "door") door.label = "ELSEWHERE";
    expect(diffRooms(room, retitled)).toBe("text");

    // A seed is never blanked: the same text on a new seed is a new room.
    const reseeded = structuredClone(room);
    const first = reseeded.fixtures.find((f) => f.kind === "terminal");
    if (first?.kind === "terminal") first.seed += 1;
    expect(diffRooms(room, reseeded)).toBe("shape");

    // A sixth tag adds a machine on the east wall: a shape change (F3).
    const grown = generateRoom({
      ...CANNED_WORKSHOP,
      tags: [...CANNED_WORKSHOP.tags, "fresh-tag"],
    });
    expect(
      grown.fixtures.filter((f) => f.kind === "machine").length,
    ).toBeGreaterThan(room.fixtures.filter((f) => f.kind === "machine").length);
    expect(diffRooms(room, grown)).toBe("shape");
  });

  it("reads a text edit on every fixture kind as text (M4 C16)", () => {
    // Mutation caught: a kind's text fields left out of the blanking (a
    // portal, hatch or exit label, a poster, placard or screen line, a lift
    // stop label or note), so a typo there reshapes the room.
    const room = generateRoom(CANNED_WORKSHOP);
    const kinds = Object.entries(TEXT_EDITS);
    expect(kinds.length).toBeGreaterThan(0);
    for (const [kind, { fixture, edits }] of kinds) {
      expect(fixture.kind).toBe(kind);
      if (kind !== "machine") expect(edits.length).toBeGreaterThan(0);
      const base: RoomSpec = { ...room, fixtures: [...room.fixtures, fixture] };
      edits.forEach((edit, i) => {
        const edited = structuredClone(base);
        const last = edited.fixtures.at(-1);
        expect(last).toBeDefined();
        if (last === undefined) return;
        edit(last);
        expect(`${kind} ${i}: ${diffRooms(base, edited)}`).toBe(
          `${kind} ${i}: text`,
        );
      });
      // Its seed is never text.
      const reseeded = structuredClone(base);
      const last = reseeded.fixtures.at(-1);
      if (last !== undefined && "seed" in last) {
        last.seed += 1;
        expect(`${kind}: ${diffRooms(base, reseeded)}`).toBe(`${kind}: shape`);
      }
    }
  });

  it("keeps the player where they stand when the floor is free, else the nearest free cell (M4 C17)", () => {
    // Mutation caught: always the entrance, the farthest cell, a blocked
    // cell, a void cell.
    const room = generateRoom(CANNED_WORKSHOP);
    const blockers = blockersFor(room);
    expect(blockers.length).toBeGreaterThan(0);

    // A free spot on the real room, off its cell's centre: kept as it is.
    let free: { x: number; z: number } | null = null;
    for (let cy = room.hall.y0; cy < room.hall.y1 && free === null; cy++)
      for (let cx = room.hall.x0; cx < room.hall.x1 && free === null; cx++) {
        const c = centre(cx, cy);
        const spot = { x: c.x + 0.3, z: c.z - 0.2 };
        if (
          isFloor(room.grid, cx, cy) &&
          !blockers.some((b) => inBox(spot.x, spot.z, b)) &&
          (cx !== room.entrance.x || cy !== room.entrance.y)
        )
          free = spot;
      }
    expect(free).not.toBeNull();
    if (free === null) return;
    expect(settleSpot(room, free.x, free.z)).toEqual(free);

    // A bare hall with one blocker over one cell: standing inside it moves
    // the player to the nearest clear cell centre, the four neighbours tied
    // and the northern one first (row before column).
    const bare: RoomSpec = {
      ...room,
      fixtures: [],
      decor: [],
      props: [],
      heroes: [],
      scaffold: [],
    };
    delete bare.interior;
    expect(blockersFor(bare)).toEqual([]);
    const cx = room.hall.x0 + 2;
    const cy = room.hall.y0 + 2;
    for (const [x, y] of [
      [cx, cy],
      [cx, cy - 1],
      [cx - 1, cy],
      [cx + 1, cy],
      [cx, cy + 1],
    ] as const)
      expect(isFloor(bare.grid, x, y)).toBe(true);
    const walled: RoomSpec = {
      ...bare,
      scaffold: [
        {
          x0: cx * CELL,
          x1: (cx + 1) * CELL,
          z0: cy * CELL,
          z1: (cy + 1) * CELL,
        },
      ],
    };
    const inside = centre(cx, cy);
    expect(settleSpot(walled, inside.x, inside.z)).toEqual(centre(cx, cy - 1));

    // A void spot just north of the hall's north-west cell: that cell's
    // centre is the only one a cell away.
    const vx = room.hall.x0;
    const vy = room.hall.y0 - 1;
    expect(isFloor(bare.grid, vx, vy)).toBe(false);
    expect(isFloor(bare.grid, vx - 1, vy)).toBe(false);
    expect(isFloor(bare.grid, vx + 1, vy)).toBe(false);
    const voidSpot = centre(vx, vy);
    expect(settleSpot(bare, voidSpot.x, voidSpot.z)).toEqual(
      centre(room.hall.x0, room.hall.y0),
    );
  });

  it("darkens a room in place with a hatch to the bridge (M4 C19)", () => {
    // Mutation caught: the lights not dimmed, a slot taken twice, the hatch
    // on the entrance edge, the input mutated, a room with no free slot
    // getting a hatch anyway.
    const room = generateRoom(CANNED_WORKSHOP);
    const before = JSON.stringify(room);
    const dark = darkened(room, "eng");
    expect(JSON.stringify(room)).toBe(before);

    expect(room.lights.length).toBeGreaterThan(0);
    expect(dark.lights.length).toBe(room.lights.length);
    expect(room.lights.some((z) => Math.round(z.level / 4) !== z.level)).toBe(
      true,
    );
    room.lights.forEach((zone, i) => {
      expect(dark.lights[i]).toEqual({
        ...zone,
        level: Math.round(zone.level / 4),
        special: "failing",
      });
    });

    expect(dark.fixtures.length).toBe(room.fixtures.length + 1);
    expect(dark.fixtures.slice(0, room.fixtures.length)).toEqual(room.fixtures);
    const hatch = dark.fixtures.at(-1);
    expect(hatch?.kind).toBe("hatch");
    if (hatch?.kind !== "hatch") return;
    expect(hatch).toEqual({
      kind: "hatch",
      slot: hatch.slot,
      label: LIFT_WORDS.bridge,
      address: { domain: "eng", permalink: MANIFEST_PERMALINK },
      seed: seedFor(room.seed, "dark-hatch"),
    });

    // Every wall edge, in `wallSlots` order: the slots alone are all taken
    // by fixtures and wall props in a generated room.
    const slots = wallRuns(room.grid).flat();
    expect(slots.length).toBeGreaterThan(0);
    const slotKeys = new Set(wallSlots(room.grid).map(edgeKey));
    expect(slots.filter((s) => slotKeys.has(edgeKey(s)))).toEqual(
      wallSlots(room.grid),
    );
    const key = edgeKey(hatch.slot);
    const at = slots.findIndex((s) => edgeKey(s) === key);
    expect(at).toBeGreaterThanOrEqual(0);
    expect(room.fixtures.map((f) => edgeKey(f.slot))).not.toContain(key);
    expect(key).not.toBe(edgeKey({ ...room.entrance, side: "s" }));

    // The first such edge in `wallSlots` order: every edge before it is
    // taken or has its arrival point inside a blocker.
    const free = freeEdgeTest(room);
    const blockers = blockersFor(room);
    const arrivalClear = (s: (typeof slots)[number]) => {
      const w = wallPoint(s);
      const x = w.x + w.inward[0] * ARRIVAL_DISTANCE;
      const z = w.z + w.inward[1] * ARRIVAL_DISTANCE;
      return !blockers.some((b) => inBox(x, z, b));
    };
    expect(free(hatch.slot) && arrivalClear(hatch.slot)).toBe(true);
    for (const s of slots.slice(0, at))
      expect(free(s) && arrivalClear(s)).toBe(false);

    // The player can walk to it in the dressed dark room.
    const arrive = arrivalPoint(hatch.slot);
    expect(reach(dark, blockersFor(dark))(arrive.x, arrive.z)).toBe(true);

    // Every edge taken: no hatch, the lights still down.
    const taken = new Set(room.fixtures.map((f) => edgeKey(f.slot)));
    const placards: Fixture[] = slots
      .filter((s) => !taken.has(edgeKey(s)))
      .map((slot) => ({ kind: "placard", slot, lines: ["FULL"] }));
    expect(placards.length).toBeGreaterThan(0);
    const full: RoomSpec = {
      ...room,
      fixtures: [...room.fixtures, ...placards],
    };
    const fullDark = darkened(full, "eng");
    expect(fullDark.fixtures).toEqual(full.fixtures);
    expect(fullDark.lights.every((z) => z.special === "failing")).toBe(true);
  });

  it("dips and flickers to the numbers of the ruling (M4 C17)", () => {
    // Mutation caught: the dip not reaching 0.1, the swap not at the dark
    // point, the factor not back to 1 at the end.
    expect(dipFactor("dip", 0)).toBe(1);
    expect(dipFactor("dip", 200)).toBeCloseTo(0.55);
    expect(dipFactor("dip", 400)).toBeCloseTo(0.1);
    expect(dipFactor("dip", DIP_SWAP_MS)).toBeCloseTo(0.1);
    expect(dipFactor("dip", 800)).toBeCloseTo(0.55);
    expect(dipFactor("dip", DIP_MS)).toBe(1);
    expect(dipFactor("dip", DIP_MS + 500)).toBe(1);
    expect(dipFactor("flicker", FLICKER_MS)).toBe(1);
    expect(dipFactor("flicker", -1)).toBe(1);
    const flicker = [0, 50, 100, 150, 200, 250, 300, 350].map((ms) =>
      dipFactor("flicker", ms),
    );
    expect(Math.min(...flicker)).toBeLessThan(0.8);
    expect(flicker).toEqual([0.55, 1, 0.7, 1, 0.6, 0.9, 0.75, 1]);
  });
});
