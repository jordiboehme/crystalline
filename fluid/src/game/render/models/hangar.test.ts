/**
 * The hangar's structure (M3 C15, C19): the bay door, the pads and the
 * gantries `buildHangar` draws, built into a fresh builder so the room's
 * own shell, fixtures and decals do not count. Every part stays in the
 * hall, under the ceiling and off the entrance; the bay door spans its
 * wall's span and no more, flush below its lintel; each part stands where
 * the world side keeps room for it (the pads' boxes, the legs' boxes, the
 * beams in plan); the lamps glow; and the pads' stencils lie on the plates.
 */

import { describe, expect, it } from "vitest";

import { CANNED_HANGAR } from "../../world/canned";
import { generateDeck } from "../../world/deck";
import { gantryBeams, gantryLegs, padBox } from "../../world/hangarShape";
import { lampBoxes } from "../../world/lamps";
import type { Box, RoomSpec } from "../../world/types";
import { CELL } from "../../world/units";
import {
  ACCENT_MARK,
  FLAG,
  FLOATS_PER_VERTEX,
  buildRoomMesh,
  createBuilder,
  type MeshData,
} from "../geometry";
import { DECAL_LIFT } from "../kit";
import { LOOKS } from "../looks";
import { worstWinding } from "../modelChecks";
import { FLUSH_DEPTH, HEADROOM, LAMP_TINT } from "./common";
import { BAY_FRAME, PAD_PLATE, buildHangar } from "./hangar";

const EPS = 1e-4;

/** One vertex read back from a mesh: position, tint and flag. */
interface Vertex {
  x: number;
  y: number;
  z: number;
  tint: [number, number, number];
  flag: number;
}

function vertices(m: MeshData): Vertex[] {
  return Array.from({ length: m.count }, (_, i) => {
    const o = i * FLOATS_PER_VERTEX;
    const v = (k: number) => m.vertices[o + k] ?? NaN;
    return {
      x: v(0),
      y: v(1),
      z: v(2),
      tint: [v(9), v(10), v(11)],
      flag: v(12),
    };
  });
}

/** The canned hangar and its structure alone, in the aperture look. */
function hangarAlone(): { room: RoomSpec; mesh: MeshData } {
  const room = generateDeck(CANNED_HANGAR, 0);
  const b = createBuilder();
  buildHangar(b, room, LOOKS.aperture);
  return { room, mesh: b.build() };
}

const inPlan = (box: Box, v: Vertex) =>
  v.x >= box.x0 - EPS &&
  v.x <= box.x1 + EPS &&
  v.z >= box.z0 - EPS &&
  v.z <= box.z1 + EPS;

describe("buildHangar (M3 C15)", () => {
  it("keeps the whole structure in the hall, under the ceiling and off the entrance", () => {
    // Mutation caught: a part through the ceiling (the catwalk's rails
    // raised past it), a part outside the hall, or one in front of the
    // lift on the entrance edge.
    const { room, mesh } = hangarAlone();
    const vs = vertices(mesh);
    expect(vs.length).toBeGreaterThan(0);
    const entrance: Box = {
      x0: room.entrance.x * CELL,
      x1: (room.entrance.x + 1) * CELL,
      z0: room.entrance.y * CELL,
      z1: (room.entrance.y + 1) * CELL,
    };
    for (const v of vs) {
      expect(v.x).toBeGreaterThanOrEqual(-EPS);
      expect(v.x).toBeLessThanOrEqual(room.width * CELL + EPS);
      expect(v.z).toBeGreaterThanOrEqual(-EPS);
      expect(v.z).toBeLessThanOrEqual(room.depth * CELL + EPS);
      expect(v.y).toBeGreaterThanOrEqual(-EPS);
      expect(v.y).toBeLessThanOrEqual(room.ceiling - HEADROOM + EPS);
      expect(inPlan(entrance, v)).toBe(false);
    }
  });

  it("spans the bay door over x 10 to 30 m of the north wall, flush below its lintel", () => {
    // Mutation caught: the door built across the whole wall (it would
    // run into the deck's screen beside it and the door slot past it),
    // or its frame standing out past `FLUSH_DEPTH` where the player
    // walks.
    const { room, mesh } = hangarAlone();
    const bay = room.hangar!.bayDoor;
    // Nothing else of the structure comes within a metre of the north
    // wall: the pads start at z 8 m and the row 3 leg at 6.5 m.
    const door = vertices(mesh).filter((v) => v.z < 1);
    expect(door.length).toBeGreaterThan(0);
    expect(Math.min(...door.map((v) => v.x))).toBeCloseTo(bay.x0 * CELL, 6);
    expect(Math.max(...door.map((v) => v.x))).toBeCloseTo(bay.x1 * CELL, 6);
    expect(bay.x0 * CELL).toBe(10);
    expect(bay.x1 * CELL).toBe(30);
    for (const v of door.filter((v) => v.y < bay.h - EPS))
      expect(v.z).toBeLessThanOrEqual(FLUSH_DEPTH + EPS);
    expect(Math.max(...door.map((v) => v.y))).toBeCloseTo(
      bay.h + BAY_FRAME.head,
      6,
    );
  });

  it("stands every part where the world keeps room for it: the door, the pads, the legs and the beams", () => {
    // Mutation caught: the catwalk wider than `GANTRY_BEAM` (it would
    // hang where the ceiling spans may run), the truss hung below its
    // height, a leg post standing out of its leg box, or a pad's lamp
    // off its pad.
    const { room, mesh } = hangarAlone();
    const hangar = room.hangar!;
    const h = hangar.gantries[0]!.h;
    expect(hangar.gantries.every((g) => g.h === h)).toBe(true);
    const bayTop = hangar.bayDoor.h + BAY_FRAME.head;
    const door: Box = {
      x0: hangar.bayDoor.x0 * CELL,
      x1: hangar.bayDoor.x1 * CELL,
      z0: 0,
      z1: 0.4,
    };
    const pads = hangar.pads.map(padBox);
    const legs = gantryLegs(room);
    const beams = gantryBeams(room);
    expect(pads.length).toBe(2);
    expect(legs.length).toBe(4);
    expect(beams.length).toBe(2);
    const misplaced = vertices(mesh).filter(
      (v) =>
        !(inPlan(door, v) && v.y <= bayTop + EPS) &&
        !pads.some((p) => inPlan(p, v) && v.y <= 0.3) &&
        !legs.some((l) => inPlan(l, v) && v.y <= h + EPS) &&
        !beams.some(
          (bm) =>
            inPlan(bm, v) &&
            v.y >= h - EPS &&
            v.y <= room.ceiling - HEADROOM + EPS,
        ),
    );
    expect(misplaced.slice(0, 3)).toEqual([]);
    // Each kind of part is there.
    const vs = vertices(mesh);
    for (const box of [door, ...pads, ...legs, ...beams])
      expect(vs.some((v) => inPlan(box, v))).toBe(true);
  });

  it("winds every face counter-clockwise against its normal", () => {
    // Mutation caught: a face wound the wrong way, which back-face
    // culling would drop (the truss's underside and the catwalk are seen
    // from below).
    expect(worstWinding(hangarAlone().mesh)).toBeGreaterThan(0.999);
  });

  it("lights the bay door's strip and corner lamps and every pad's corner lamps (C19)", () => {
    // Mutation caught: a lamp drawn lit instead of glowing (it would dim
    // with the room at the far end), a corner lamp left out, or the strip
    // drawn in a plain colour.
    const { room, mesh } = hangarAlone();
    const hangar = room.hangar!;
    const vs = vertices(mesh);
    const x0 = hangar.bayDoor.x0 * CELL;
    const x1 = hangar.bayDoor.x1 * CELL;
    const amber = vs.filter(
      (v) =>
        v.flag === FLAG.signal &&
        v.z < 1 &&
        v.tint.every((c, i) => Math.abs(c - (LAMP_TINT[i] ?? NaN)) < EPS),
    );
    expect(amber.some((v) => v.x < x0 + 1)).toBe(true);
    expect(amber.some((v) => v.x > x1 - 1)).toBe(true);
    for (const v of amber) {
      expect(v.y).toBeGreaterThan(hangar.bayDoor.h);
      expect(v.x < x0 + 1 || v.x > x1 - 1).toBe(true);
    }
    const strip = vs.filter(
      (v) =>
        v.flag === FLAG.emissive &&
        v.tint[0] === ACCENT_MARK &&
        v.z < 1 &&
        v.y > hangar.bayDoor.h,
    );
    expect(strip.length).toBeGreaterThan(0);
    const xs = strip.map((v) => v.x);
    expect(Math.max(...xs) - Math.min(...xs)).toBeGreaterThan(15);
    for (const p of hangar.pads.map(padBox)) {
      const corners = [
        [p.x0, p.z0],
        [p.x1, p.z0],
        [p.x0, p.z1],
        [p.x1, p.z1],
      ] as const;
      for (const [cx, cz] of corners)
        expect(
          vs.some(
            (v) =>
              v.flag === FLAG.signal &&
              inPlan(p, v) &&
              Math.hypot(v.x - cx, v.z - cz) < 0.5,
          ),
        ).toBe(true);
    }
  });

  it("hangs no ceiling lamp directly over a gantry's beam", () => {
    // Mutation caught: a gantry moved under a row of lamps (onto row 1,
    // under the first row of the canned hangar's zone lamps), which would
    // hide those lamps behind the truss from below.
    const room = generateDeck(CANNED_HANGAR, 0);
    const lamps = lampBoxes(room);
    const beams = gantryBeams(room);
    expect(lamps.length).toBeGreaterThan(0);
    expect(beams.length).toBeGreaterThan(0);
    for (const l of lamps)
      for (const bm of beams)
        expect(l.z1 <= bm.z0 || l.z0 >= bm.z1 || l.x1 <= bm.x0).toBe(true);
  });

  it("lays the pads' stencils on the plates, not under them", () => {
    // Mutation caught: a floor decal on a pad laid at the floor's
    // `DECAL_LIFT` (inside the plate, so never seen).
    const room = generateDeck(CANNED_HANGAR, 0);
    const pads = room.hangar!.pads.map(padBox);
    const onPads = vertices(buildRoomMesh(room, LOOKS.aperture).static).filter(
      (v) => v.flag === FLAG.decal && pads.some((p) => inPlan(p, v)),
    );
    expect(onPads.length).toBeGreaterThan(0);
    for (const v of onPads)
      expect(v.y).toBeGreaterThanOrEqual(PAD_PLATE + DECAL_LIFT - EPS);
  });
});
