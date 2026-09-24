import { describe, expect, it } from "vitest";

import { CANNED_BRIDGE, CANNED_HUB } from "../world/canned";
import { CELL, generateRoom } from "../world/generate";
import { BAY, isFloor } from "../world/layout";
import { scaffoldBoxes } from "../world/move";
import type { PlaceInput, RoomSpec } from "../world/types";
import {
  FLAG,
  FLOATS_PER_VERTEX,
  LINTEL,
  buildRoomMesh,
  type MeshData,
} from "./geometry";
import { LAYER, TEXT_BASE, layerPlan } from "./layers";
import { LOOKS } from "./looks";

const EPS = 1e-4;

function vertexOf(m: MeshData, i: number) {
  const o = i * FLOATS_PER_VERTEX;
  const v = (k: number) => m.vertices[o + k] ?? NaN;
  return {
    pos: [v(0), v(1), v(2)] as const,
    normal: [v(3), v(4), v(5)] as const,
    layer: v(8),
    tint: [v(9), v(10), v(11)],
    flag: v(12),
  };
}

type Vertex = ReturnType<typeof vertexOf>;

function all(m: MeshData): Vertex[] {
  return Array.from({ length: m.count }, (_, i) => vertexOf(m, i));
}

function triangles(m: MeshData): [Vertex, Vertex, Vertex][] {
  const vs = all(m);
  const out: [Vertex, Vertex, Vertex][] = [];
  for (let t = 0; t + 2 < vs.length; t += 3) {
    const [a, b, c] = [vs[t], vs[t + 1], vs[t + 2]];
    if (!a || !b || !c) throw new Error("short triangle");
    out.push([a, b, c]);
  }
  return out;
}

/**
 * For every triangle, the geometric normal of its winding (counter-clockwise
 * seen from the front) against the normal stored on its vertices: the two
 * must point the same way, or back-face culling would drop a face that
 * should be seen. Returns the smallest dot product of the two unit normals.
 */
function worstWinding(m: MeshData): number {
  let worst = Infinity;
  for (const [a, b, c] of triangles(m)) {
    const e1 = [b.pos[0] - a.pos[0], b.pos[1] - a.pos[1], b.pos[2] - a.pos[2]];
    const e2 = [c.pos[0] - a.pos[0], c.pos[1] - a.pos[1], c.pos[2] - a.pos[2]];
    const [e1x = 0, e1y = 0, e1z = 0] = e1;
    const [e2x = 0, e2y = 0, e2z = 0] = e2;
    const n = [
      e1y * e2z - e1z * e2y,
      e1z * e2x - e1x * e2z,
      e1x * e2y - e1y * e2x,
    ];
    const len = Math.hypot(...n);
    const dot =
      ((n[0] ?? 0) * a.normal[0] +
        (n[1] ?? 0) * a.normal[1] +
        (n[2] ?? 0) * a.normal[2]) /
      len;
    worst = Math.min(worst, dot);
  }
  return worst;
}

const ROOMS: [string, PlaceInput][] = [
  ["bridge", CANNED_BRIDGE],
  ["hub", CANNED_HUB],
  ["bridge under construction", { ...CANNED_BRIDGE, status: "draft" }],
  ["derelict bridge", { ...CANNED_BRIDGE, status: "archived" }],
];

for (const [name, place] of ROOMS) {
  describe(`buildRoomMesh: ${name}`, () => {
    const room = generateRoom(place);
    const built = buildRoomMesh(room, LOOKS.day);
    const meshes = [built.static, ...built.movers.map((m) => m.mesh)];
    const plan = layerPlan(room);

    it("is whole triangles with nothing left over", () => {
      for (const m of meshes) {
        expect(m.count % 3).toBe(0);
        expect(m.vertices.length).toBe(m.count * FLOATS_PER_VERTEX);
      }
    });

    it("stays inside the grid's bounding box and below the ceiling", () => {
      const outside = meshes.flatMap((m) =>
        all(m).filter(({ pos: [x, y, z] }) => {
          const inside =
            x >= -EPS &&
            x <= room.width * CELL + EPS &&
            z >= -EPS &&
            z <= room.depth * CELL + EPS &&
            y >= -EPS &&
            y <= room.ceiling + EPS;
          return !inside;
        }),
      );
      expect(outside.map((v) => v.pos)).toEqual([]);
    });

    it("uses unit normals and layers the texture array has", () => {
      const wrong = meshes.flatMap((m) =>
        all(m).filter(
          (v) =>
            Math.abs(Math.hypot(...v.normal) - 1) > 1e-5 ||
            !Number.isInteger(v.layer) ||
            v.layer < 0 ||
            v.layer >= plan.count,
        ),
      );
      expect(wrong).toEqual([]);
    });

    it("winds every triangle counter-clockwise seen from its normal's side", () => {
      for (const m of meshes) expect(worstWinding(m)).toBeGreaterThan(0.999);
    });

    it("draws every text layer somewhere", () => {
      const used = new Set(all(built.static).map((v) => v.layer));
      for (let l = TEXT_BASE; l < plan.count; l++)
        expect(used.has(l)).toBe(true);
    });

    it("never puts a wall between two floor cells", () => {
      let walls = 0;
      for (const [a, b, c] of triangles(built.static)) {
        const n = a.normal;
        if (Math.abs(n[1]) > EPS) continue;
        const ys = [a.pos[1], b.pos[1], c.pos[1]];
        if (Math.min(...ys) > EPS) continue;
        if (Math.max(...ys) < room.ceiling - EPS) continue;
        // A full-height vertical triangle: which plane is it on?
        const alongX = Math.abs(n[2]) > 0.5;
        const plane = alongX ? a.pos[2] : a.pos[0];
        const onBorder =
          Math.abs(plane / CELL - Math.round(plane / CELL)) < EPS;
        const span = alongX
          ? Math.max(a.pos[0], b.pos[0], c.pos[0]) -
            Math.min(a.pos[0], b.pos[0], c.pos[0])
          : Math.max(a.pos[2], b.pos[2], c.pos[2]) -
            Math.min(a.pos[2], b.pos[2], c.pos[2]);
        if (!onBorder || Math.abs(span - CELL) > EPS) continue;
        walls++;
        // The cell the wall faces is floor, the one behind it is not.
        const mid = alongX
          ? (a.pos[0] + b.pos[0] + c.pos[0]) / 3
          : (a.pos[2] + b.pos[2] + c.pos[2]) / 3;
        const along = Math.floor(mid / CELL);
        const border = Math.round(plane / CELL);
        const facing = (alongX ? n[2] : n[0]) > 0 ? border : border - 1;
        const behind = (alongX ? n[2] : n[0]) > 0 ? border - 1 : border;
        const cell = (across: number) =>
          alongX
            ? isFloor(room.grid, along, across)
            : isFloor(room.grid, across, along);
        expect(cell(facing)).toBe(true);
        expect(cell(behind)).toBe(false);
      }
      expect(walls).toBeGreaterThan(0);
    });

    it("gives every unsealed door a mover and nothing else one", () => {
      const doors = room.fixtures
        .map((f, i) => ({ f, i }))
        .filter(({ f }) => f.kind === "door" && f.address !== null)
        .map(({ i }) => `door:${i}`);
      expect(doors.length).toBeGreaterThan(0);
      expect(new Set(built.movers.map((m) => m.key))).toEqual(new Set(doors));
    });

    it("hangs one lamp panel per light zone", () => {
      const lamps = triangles(built.static).filter(
        ([a]) => a.flag === FLAG.lamp,
      );
      expect(lamps.length).toBe(room.lights.length * 2);
    });

    it("is the same every time", () => {
      const again = buildRoomMesh(room, LOOKS.day).static.vertices;
      const first = built.static.vertices;
      expect(again.length).toBe(first.length);
      // A plain loop: toEqual on millions of floats is too slow for the hub.
      let differ = -1;
      for (let i = 0; i < first.length && differ < 0; i++)
        if (!Object.is(again[i], first[i])) differ = i;
      expect(differ).toBe(-1);
    }, 20_000);
  });
}

describe("buildRoomMesh details", () => {
  it("keeps everything under a low ceiling", () => {
    const room: RoomSpec = { ...generateRoom(CANNED_BRIDGE), ceiling: 3 };
    for (const v of all(buildRoomMesh(room, LOOKS.day).static)) {
      expect(v.pos[1]).toBeLessThanOrEqual(room.ceiling + EPS);
    }
  });

  it("has a portal surface, door frames and lamps", () => {
    const flags = new Set(
      all(buildRoomMesh(generateRoom(CANNED_BRIDGE), LOOKS.day).static).map(
        (v) => v.flag,
      ),
    );
    expect(flags.has(FLAG.portal)).toBe(true);
    expect(flags.has(FLAG.frame)).toBe(true);
    expect(flags.has(FLAG.lamp)).toBe(true);
  });

  it("puts a lintel over every doorway between the hall and a bay or the corridor", () => {
    const room = generateRoom(CANNED_HUB);
    const tris = triangles(buildRoomMesh(room, LOOKS.day).static);
    // Lintel triangles: vertical, from LINTEL up to the ceiling.
    const lintels = tris.filter(([a, b, c]) => {
      const ys = [a.pos[1], b.pos[1], c.pos[1]];
      return (
        Math.abs(a.normal[1]) < EPS &&
        Math.abs(Math.min(...ys) - LINTEL) < EPS &&
        Math.abs(Math.max(...ys) - room.ceiling) < EPS
      );
    });
    // Each lintel face, keyed by the cell it faces into and that side.
    const faces = new Map<string, number>();
    for (const [a, b, c] of lintels) {
      const n = a.normal;
      const mx = (a.pos[0] + b.pos[0] + c.pos[0]) / 3;
      const mz = (a.pos[2] + b.pos[2] + c.pos[2]) / 3;
      // Step half a cell along the normal from the face into its cell.
      const x = Math.floor((mx + (n[0] * CELL) / 2) / CELL);
      const y = Math.floor((mz + (n[2] * CELL) / 2) / CELL);
      const side =
        n[0] > 0.5 ? "w" : n[0] < -0.5 ? "e" : n[2] > 0.5 ? "n" : "s";
      const key = `${x},${y},${side}`;
      faces.set(key, (faces.get(key) ?? 0) + 1);
    }
    // The doorway columns: west of the hall when there is a corridor, and
    // before each bay east of it.
    const doorways = new Set<number>();
    if (room.hall.x0 > 0) doorways.add(room.hall.x0 - 1);
    for (let x = room.hall.x1; x < room.width; x += BAY + 1) doorways.add(x);
    expect(doorways.size).toBeGreaterThan(1);
    // One lintel face on each side of every edge where a doorway cell
    // meets another floor cell: both faces of both ends of every doorway.
    const expected: string[] = [];
    for (let y = 0; y < room.depth; y++) {
      for (let x = 0; x < room.width; x++) {
        if (!isFloor(room.grid, x, y)) continue;
        for (const [side, dx] of [
          ["w", -1],
          ["e", 1],
        ] as const) {
          const nx = x + dx;
          if (isFloor(room.grid, nx, y) && doorways.has(x) !== doorways.has(nx))
            expected.push(`${x},${y},${side}`);
        }
      }
    }
    expect([...faces.keys()].sort()).toEqual(expected.sort());
    // Two triangles per face: exactly one quad, never a doubled one.
    for (const count of faces.values()) expect(count).toBe(2);
  }, 20_000);

  it("stands the scaffold poles exactly on the scaffold boxes", () => {
    // Without fixtures and furniture the only metal left is the scaffold.
    const room: RoomSpec = {
      ...generateRoom({ ...CANNED_BRIDGE, status: "draft" }),
      fixtures: [],
      decor: [],
    };
    const boxes = scaffoldBoxes(room);
    expect(boxes.length).toBeGreaterThan(0);
    const vs = all(buildRoomMesh(room, LOOKS.day).static);
    const metal = vs.filter((v) => v.layer === LAYER.metal);
    expect(metal.length).toBeGreaterThan(0);
    const inside = (v: Vertex) =>
      boxes.some(
        (b) =>
          v.pos[0] >= b.x0 - EPS &&
          v.pos[0] <= b.x1 + EPS &&
          v.pos[2] >= b.z0 - EPS &&
          v.pos[2] <= b.z1 + EPS,
      );
    for (const v of metal) expect(inside(v)).toBe(true);
    // Each box has a pole in each of its corners.
    for (const b of boxes) {
      for (const [x, z] of [
        [b.x0, b.z0],
        [b.x1, b.z0],
        [b.x0, b.z1],
        [b.x1, b.z1],
      ] as const) {
        const near = metal.some(
          (v) =>
            Math.abs(v.pos[0] - x) < 0.2 &&
            Math.abs(v.pos[2] - z) < 0.2 &&
            v.pos[1] < EPS,
        );
        expect(near).toBe(true);
      }
    }
    expect(vs.some((v) => v.layer === LAYER.hazard)).toBe(true);
    expect(scaffoldBoxes(generateRoom(CANNED_BRIDGE))).toHaveLength(0);
  });

  it("builds no scaffold in a room that is not under construction", () => {
    // Without fixtures and furniture, scaffold poles are the only metal.
    for (const status of ["stable", "archived", "deprecated"]) {
      const room: RoomSpec = {
        ...generateRoom({ ...CANNED_BRIDGE, status }),
        fixtures: [],
        decor: [],
      };
      expect(room.condition).not.toBe("construction");
      const metal = all(buildRoomMesh(room, LOOKS.day).static).filter(
        (v) => v.layer === LAYER.metal,
      );
      expect(metal).toHaveLength(0);
    }
  });

  it("tints the cross-domain portal in the look's other portal colour", () => {
    const alt = LOOKS.day.palette.portalAlt;
    const portal = all(
      buildRoomMesh(generateRoom(CANNED_BRIDGE), LOOKS.day).static,
    ).filter((v) => v.flag === FLAG.portal);
    expect(portal.length).toBeGreaterThan(0);
    for (const v of portal) {
      v.tint.forEach((c, i) => expect(c).toBeCloseTo(alt[i] ?? NaN, 5));
    }
  });
});
