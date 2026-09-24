/**
 * The model gallery's room: one of everything the station can draw, laid
 * out so each piece can be walked up to and judged, and valid enough that
 * the renderer, the light grid and the movement code take it as they take
 * a generated room.
 */

import { describe, expect, it } from "vitest";

import { lightGrid } from "../render/lightgrid";
import { GAME_VERSION } from "../version";
import { MACHINE_KINDS } from "./generate";
import { BAY, isFloor } from "./layout";
import { blockersFor, decorFootprint, footprintOf, type Box } from "./move";
import { galleryRoom } from "./canned";
import type { DecorKind, DoorStyle, Fixture, RoomSpec } from "./types";

const DECOR_KINDS: readonly DecorKind[] = [
  "command-console",
  "captain-chair",
  "round-table",
  "council-chair",
  "generator",
  "pipe-run",
  "shelf-row",
  "lab-island",
  "specimen-tank",
];

const DOOR_STYLES: readonly DoorStyle[] = ["sliding", "bulkhead", "blast"];

function inside(r: RoomSpec["hall"], x: number, y: number) {
  return x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1;
}

function overlaps(a: Box, b: Box) {
  return a.x0 < b.x1 && b.x0 < a.x1 && a.z0 < b.z1 && b.z0 < a.z1;
}

describe("galleryRoom", () => {
  const room = galleryRoom();
  const of = <K extends Fixture["kind"]>(kind: K) =>
    room.fixtures.filter(
      (f): f is Extract<Fixture, { kind: K }> => f.kind === kind,
    );

  it("is a v2 room with a hall and two bays east of it", () => {
    expect(room.version).toBe(GAME_VERSION);
    expect(room.grid).toHaveLength(room.depth);
    for (const row of room.grid) expect(row).toHaveLength(room.width);
    expect(room.hall.x0).toBe(0);
    expect(room.width).toBe(room.hall.x1 + 2 * (BAY + 1));
    expect(room.condition).toBe("clean");
    expect(room.dropped).toBe(0);
  });

  it("carries one of every fixture kind", () => {
    const kinds = new Set(room.fixtures.map((f) => f.kind));
    expect([...kinds].sort()).toEqual([
      "door",
      "hatch",
      "machine",
      "placard",
      "portal",
      "poster",
      "terminal",
    ]);
  });

  it("carries every door style open, and both sealed ways", () => {
    const doors = of("door");
    for (const style of DOOR_STYLES) {
      expect(doors.some((d) => d.style === style && d.address !== null)).toBe(
        true,
      );
    }
    const sealed = [...doors, ...of("portal")]
      .map((d) => d.sealedLabel)
      .filter((l) => l !== null);
    expect(new Set(sealed)).toEqual(new Set(["?FILE NOT FOUND", "NO ROUTE"]));
    const portals = of("portal");
    expect(portals.some((p) => p.crossDomain && p.address !== null)).toBe(true);
    expect(portals.some((p) => !p.crossDomain && p.address !== null)).toBe(
      true,
    );
  });

  it("carries every machine kind once, all of them in the bays", () => {
    const machines = of("machine");
    expect(machines.map((m) => m.machine).sort()).toEqual(
      [...MACHINE_KINDS].sort(),
    );
    for (const m of machines) {
      expect(inside(room.hall, m.slot.x, m.slot.y)).toBe(false);
    }
  });

  it("carries every decor kind once, in the hall", () => {
    expect(room.decor.map((d) => d.kind).sort()).toEqual(
      [...DECOR_KINDS].sort(),
    );
    for (const d of room.decor) {
      expect(inside(room.hall, Math.floor(d.x), Math.floor(d.y))).toBe(true);
    }
  });

  it("puts every fixture in its own slot on a floor cell", () => {
    const keys = room.fixtures.map(
      (f) => `${f.slot.x},${f.slot.y},${f.slot.side}`,
    );
    expect(new Set(keys).size).toBe(keys.length);
    for (const f of room.fixtures) {
      expect(isFloor(room.grid, f.slot.x, f.slot.y)).toBe(true);
    }
    expect(isFloor(room.grid, room.spawn.x, room.spawn.y)).toBe(true);
  });

  it("lets nothing that blocks overlap anything else that blocks", () => {
    const boxes = [
      ...room.fixtures.map(footprintOf),
      ...room.decor.map(decorFootprint),
    ].filter((b) => b !== null);
    expect(blockersFor(room)).toHaveLength(boxes.length);
    for (let i = 0; i < boxes.length; i++) {
      for (let j = i + 1; j < boxes.length; j++) {
        const a = boxes[i];
        const b = boxes[j];
        if (a === undefined || b === undefined) continue;
        expect(overlaps(a, b), `boxes ${String(i)} and ${String(j)}`).toBe(
          false,
        );
      }
    }
  });

  it("lights every floor cell", () => {
    const grid = lightGrid(room);
    for (let y = 0; y < room.depth; y++) {
      for (let x = 0; x < room.width; x++) {
        if (!isFloor(room.grid, x, y)) continue;
        expect(grid.zoneOfCell[y * room.width + x]).toBeGreaterThanOrEqual(0);
      }
    }
  });

  it("is the same room every time", () => {
    expect(JSON.stringify(galleryRoom())).toBe(JSON.stringify(room));
  });
});
