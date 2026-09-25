/**
 * The model gallery's room: one of everything the station can draw, laid
 * out so each piece can be walked up to and judged, and valid enough that
 * the renderer, the light grid and the movement code take it as they take
 * a generated room.
 */

import { describe, expect, it } from "vitest";

import { lightGrid } from "../render/lightgrid";
import { GAME_VERSION } from "../version";
import { BAY, isFloor } from "./layout";
import { decorFootprint, footprintOf, propFootprint } from "./footprints";
import { blockersFor } from "./move";
import { PROP_CATALOGUE, PROP_KINDS } from "./props";
import { dressingSites, edgeKey, turnForSide, wallAnchor } from "./sites";
import { galleryRoom } from "./canned";
import type {
  Box,
  DecorKind,
  DoorStyle,
  Fixture,
  MachineKind,
  Prop,
  RoomSpec,
  Side,
  WallSlot,
} from "./types";

/**
 * The wall edge a wall or ceiling prop is anchored on (ruling 1), recovered
 * from its turn and anchor point: the inverse of `wallAnchor`.
 */
const SIDE_OF_TURN: readonly Side[] = ["s", "w", "n", "e"];
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

/**
 * Every decor kind and door style, as the keys of a record the compiler
 * holds to the union: a kind added to `DecorKind` or `DoorStyle` and not
 * here fails the typecheck, so the gallery test cannot silently miss it.
 */
const DECOR_KINDS = Object.keys({
  "command-console": true,
  "captain-chair": true,
  "round-table": true,
  "council-chair": true,
  generator: true,
  "pipe-run": true,
  "shelf-row": true,
  "lab-island": true,
  "specimen-tank": true,
} satisfies Record<DecorKind, true>) as DecorKind[];

const DOOR_STYLES = Object.keys({
  sliding: true,
  bulkhead: true,
  blast: true,
} satisfies Record<DoorStyle, true>) as DoorStyle[];

const FIXTURE_KINDS = Object.keys({
  door: true,
  hatch: true,
  machine: true,
  placard: true,
  portal: true,
  poster: true,
  terminal: true,
} satisfies Record<Fixture["kind"], true>) as Fixture["kind"][];

const MACHINES = Object.keys({
  workbench: true,
  "lab-bench": true,
  "server-rack": true,
  "cryo-pod": true,
  fabricator: true,
  hydroponics: true,
  "nav-table": true,
  "comms-array": true,
  "reactor-coupling": true,
  "cargo-loader": true,
  "med-scanner": true,
  containment: true,
} satisfies Record<MachineKind, true>) as MachineKind[];

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

  it("is a v2 room with a hall and four bays east of it", () => {
    expect(room.version).toBe(GAME_VERSION);
    expect(room.grid).toHaveLength(room.depth);
    for (const row of room.grid) expect(row).toHaveLength(room.width);
    expect(room.hall.x0).toBe(0);
    expect(room.bays).toHaveLength(4);
    expect(room.width).toBe(room.hall.x1 + 4 * (BAY + 1));
    expect(room.condition).toBe("clean");
    expect(room.dropped).toBe(0);
  });

  it("carries one of every fixture kind", () => {
    const kinds = new Set(room.fixtures.map((f) => f.kind));
    expect([...kinds].sort()).toEqual([...FIXTURE_KINDS].sort());
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
    expect(machines.map((m) => m.machine).sort()).toEqual([...MACHINES].sort());
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
      ...room.props.map(propFootprint),
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

  it("carries every prop kind and variant exactly once, a run counting once per variant", () => {
    for (const kind of PROP_KINDS) {
      const entry = PROP_CATALOGUE[kind];
      for (let variant = 0; variant < entry.variants; variant++) {
        const matches = room.props.filter(
          (p) => p.kind === kind && p.variant === variant,
        );
        expect(matches.length, `${kind} ${String(variant)}`).toBe(
          entry.run ? 2 : 1,
        );
      }
    }
  });

  it("puts every wall prop on a free wall edge that is no fixture edge, one to an edge", () => {
    const sites = dressingSites(room);
    const fixtureEdges = new Set(room.fixtures.map((f) => edgeKey(f.slot)));
    const seenWall = new Set<string>();
    const seenCeiling = new Set<string>();
    for (const p of room.props) {
      if (p.anchor !== "wall" && p.anchor !== "ceiling") continue;
      if (PROP_CATALOGUE[p.kind].span) continue;
      const e = edgeOf(p);
      const k = edgeKey(e);
      expect(wallAnchor(e), `${p.kind} ${k}`).toEqual({
        x: p.x,
        y: p.y,
        turn: p.turn,
      });
      expect(p.turn, `${p.kind} ${k}`).toBe(turnForSide(e.side));
      expect(sites.free.has(k), `${p.kind} ${k}`).toBe(true);
      expect(fixtureEdges.has(k), `${p.kind} ${k}`).toBe(false);
      const seen = p.anchor === "wall" ? seenWall : seenCeiling;
      expect(seen.has(k), `${p.kind} ${k} twice`).toBe(false);
      seen.add(k);
    }
    expect(seenWall.size).toBeGreaterThan(0);
    expect(seenCeiling.size).toBeGreaterThan(0);
  });

  it("puts every ceiling prop on bay 3's or bay 4's north wall edge", () => {
    const propBays = room.bays.slice(2, 4);
    for (const p of room.props) {
      if (p.anchor !== "ceiling") continue;
      if (PROP_CATALOGUE[p.kind].span) continue;
      const e = edgeOf(p);
      expect(e.side).toBe("n");
      expect(
        propBays.some(
          (b) => e.x >= b.x0 && e.x < b.x1 && e.y >= b.y0 && e.y < b.y1,
        ),
      ).toBe(true);
    }
  });

  it("hangs every span over row 6 of bay 3 or bay 4, along x", () => {
    const propBays = room.bays.slice(2, 4);
    const spans = room.props.filter((p) => PROP_CATALOGUE[p.kind].span);
    expect(spans).toHaveLength(4);
    for (const p of spans) {
      expect(p.anchor).toBe("ceiling");
      expect(p.turn).toBe(0);
      expect(p.y).toBe(6.5);
      const first = p.x - 1; // the segment's first cell
      expect(Number.isInteger(first)).toBe(true);
      expect(propBays.some((b) => first >= b.x0 && first + 1 < b.x1)).toBe(
        true,
      );
    }
  });

  it("centres every floor prop on rows 1, 3 or 5 of bays 3 and 4, turn 0", () => {
    const propBays = room.bays.slice(2, 4);
    const floorProps = room.props.filter((p) => p.anchor === "floor");
    expect(floorProps.length).toBeGreaterThan(0);
    for (const p of floorProps) {
      expect(p.turn).toBe(0);
      expect(p.x % 1).toBeCloseTo(0.5, 6);
      expect(p.y % 1).toBeCloseTo(0.5, 6);
      const cx = Math.floor(p.x);
      const cy = Math.floor(p.y);
      expect([1, 3, 5]).toContain(cy);
      expect(
        propBays.some(
          (b) => cx >= b.x0 && cx < b.x1 && cy >= b.y0 && cy < b.y1,
        ),
      ).toBe(true);
    }
  });

  it("is the same room every time", () => {
    expect(JSON.stringify(galleryRoom())).toBe(JSON.stringify(room));
  });
});
