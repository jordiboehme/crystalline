import { describe, expect, it } from "vitest";

import { boxKey } from "../world/box";
import { CANNED_BRIDGE, CANNED_HUB, heroHallRoom } from "../world/canned";
import { consoleRoom } from "../world/consoleRoom";
import { plainFinish } from "../world/finish";
import { generateRoom } from "../world/generate";
import { BAY, isFloor, wallRuns } from "../world/layout";
import { edgeKey } from "../world/sites";
import type { PlaceInput, RoomSpec, Side } from "../world/types";
import { CELL } from "../world/units";
import {
  ACCENT_MARK,
  ACCENT_STRIPE,
  FLAG,
  FLOATS_PER_VERTEX,
  LINTEL,
  WALL_PATTERN_LOOK,
  accentTint,
  buildRoomMesh,
  wallPatternOf,
  type MeshData,
} from "./geometry";
import { DECAL_LIFT } from "./kit";
import { LAYER, TEXT_BASE, layerPlan } from "./layers";
import { LOOKS, accentFor, type Rgb } from "./looks";
import { positions, worstWinding } from "./modelChecks";
import { buildInteriorMesh } from "./models/interior";
import { CONSOLE_SHELL } from "./models/interior/common";
import { COLUMN } from "./models/interior/console";

const EPS = 1e-4;

function vertexOf(m: MeshData, i: number) {
  const o = i * FLOATS_PER_VERTEX;
  const v = (k: number) => m.vertices[o + k] ?? NaN;
  return {
    pos: [v(0), v(1), v(2)] as const,
    normal: [v(3), v(4), v(5)] as const,
    uv: [v(6), v(7)] as const,
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

    it("gives every way its movers", () => {
      const keys = room.fixtures.flatMap((f, i) => {
        switch (f.kind) {
          case "door":
            return [`door:${i}`, `lamp:${i}`, `spark:${i}`];
          case "hatch":
            return [`lid:${i}`];
          case "portal":
            return [`disc:${i}`];
          default:
            return [];
        }
      });
      expect(keys.length).toBeGreaterThan(0);
      expect(new Set(built.movers.map((m) => m.key))).toEqual(new Set(keys));
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
    // The portal surface is a mover (its disc), so the movers count too.
    const built = buildRoomMesh(generateRoom(CANNED_BRIDGE), LOOKS.day);
    const flags = new Set(
      [built.static, ...built.movers.map((m) => m.mesh)].flatMap((m) =>
        all(m).map((v) => v.flag),
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
    const boxes = room.scaffold;
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
    expect(generateRoom(CANNED_BRIDGE).scaffold).toHaveLength(0);
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
    // The portal surface is its disc, a mover.
    const built = buildRoomMesh(generateRoom(CANNED_BRIDGE), LOOKS.day);
    const portal = built.movers
      .filter((m) => m.part === "disc")
      .flatMap((m) => all(m.mesh))
      .filter((v) => v.flag === FLAG.portal);
    expect(portal.length).toBeGreaterThan(0);
    for (const v of portal) {
      v.tint.forEach((c, i) => expect(c).toBeCloseTo(alt[i] ?? NaN, 5));
    }
  });
});

describe("buildRoomMesh's hero movers", () => {
  it("gives every police box in a room its two leaves as movers, and no other hero any (2.6e C17)", () => {
    // Mutation caught: the hero movers not emitted, or keyed by the fixture
    // index.
    const room = heroHallRoom();
    const boxes = room.heroes.flatMap((h, i) =>
      h.kind === "police-box" ? [i] : [],
    );
    expect(boxes.length).toBeGreaterThan(0);
    const { movers } = buildRoomMesh(room, LOOKS.aperture);
    const wings = movers.filter((m) => m.part === "wing");
    expect(wings.length).toBe(2 * boxes.length);
    expect(new Set(wings.map((m) => m.key))).toEqual(
      new Set(boxes.map(boxKey)),
    );
  });
});

describe("the console room's shell (2.6e C4)", () => {
  /** The shell's floor: upward faces at height 0 on the floor layer. */
  const floorOf = (m: MeshData) =>
    all(m).filter(
      (v) =>
        v.layer === LAYER.floor &&
        Math.abs(v.pos[1]) < EPS &&
        Math.abs(v.normal[1] - 1) < EPS,
    );
  const expectTint = (tint: readonly number[], want: Rgb) => {
    expect(tint[0]).toBeCloseTo(want[0], 5);
    expect(tint[1]).toBeCloseTo(want[1], 5);
    expect(tint[2]).toBeCloseTo(want[2], 5);
  };

  it("draws the floor, the ceiling and the walls in the room's own tints in every look", () => {
    // Mutation caught: the shell of a room with fittings drawn in the
    // look's palette, so the console room's colours change with the look.
    for (const look of Object.values(LOOKS)) {
      const m = buildRoomMesh(consoleRoom(), look).static;
      const floor = floorOf(m);
      expect(floor.length).toBeGreaterThan(0);
      for (const v of floor) expectTint(v.tint, CONSOLE_SHELL.floor);
      const ceiling = all(m).filter((v) => v.layer === LAYER.ceiling);
      const panels = ceiling.filter((v) => v.flag === FLAG.lit);
      expect(panels.length).toBeGreaterThan(0);
      for (const v of panels) expectTint(v.tint, CONSOLE_SHELL.ceiling);
      const walls = all(m).filter(
        (v) => v.layer === LAYER.panel && Math.abs(v.normal[1]) < EPS,
      );
      expect(walls.length).toBeGreaterThan(0);
      for (const v of walls) expectTint(v.tint, CONSOLE_SHELL.wall);
    }
  });

  it("leaves a generated room's floor in the look's own colour", () => {
    // Mutation caught: every room's shell drawn in the console room's
    // tints.
    for (const look of Object.values(LOOKS)) {
      const floor = floorOf(
        buildRoomMesh(generateRoom(CANNED_BRIDGE), look).static,
      );
      expect(floor.length).toBeGreaterThan(0);
      for (const v of floor) expectTint(v.tint, look.palette.floor);
    }
  });
});

describe("the console room's movers and size (2.6e C9, C19)", () => {
  it("gives the console room exactly one mover, the rotor, and it stays inside the column (2.6e C9)", () => {
    // Mutation caught: a second moving part (the doors, the scanner), or the
    // rotor travelling through the column's top ring.
    const { movers } = buildRoomMesh(consoleRoom(), LOOKS.aperture);
    expect(movers.map((m) => m.part)).toEqual(["rotor"]);
    const m = movers[0]!;
    const top = Math.max(...positions(m.mesh).map((p) => p[1])) + m.travel;
    expect(top).toBeLessThanOrEqual(COLUMN.h1 - 0.1);
    const r = Math.max(
      ...positions(m.mesh).map((p) => Math.hypot(p[0] - 6, p[2] - 6)),
    );
    expect(r).toBeLessThan(COLUMN.radius);
  });

  it("keeps the whole console room under 60,000 triangles (2.6e C19)", () => {
    // Mutation caught: a fitting or the shell grown past the room's share
    // of the frame.
    const room = consoleRoom();
    const { static: s, movers } = buildRoomMesh(room, LOOKS.aperture);
    const pieces = (room.interior ?? []).reduce(
      (n, p) => n + buildInteriorMesh(p.kind, p.variant, LOOKS.aperture).count,
      0,
    );
    const total =
      (s.count + pieces + movers.reduce((n, m) => n + m.mesh.count, 0)) / 3;
    expect(total).toBeLessThan(60_000);
  });
});

/**
 * The accent stripe's quads in a mesh: walked six vertices at a time, the
 * vertical quads whose tint carries the accent mark and that span a whole
 * cell edge `DECAL_LIFT` off a cell border (a model's accent parts, such as
 * a terminal's chair back, are neither), each with its tint, its bottom and
 * top height and the key of the cell edge it lies on (read back from its
 * world x and z and its normal, as `edgeQuad` places it).
 */
function stripeQuads(
  m: MeshData,
): { tint: number[]; h0: number; h1: number; edge: string }[] {
  const vs = all(m);
  const out: { tint: number[]; h0: number; h1: number; edge: string }[] = [];
  for (let q = 0; q + 5 < vs.length; q += 6) {
    const quad = vs.slice(q, q + 6);
    const first = quad[0];
    if (first === undefined) continue;
    if (!quad.every((v) => v.tint[0] === ACCENT_MARK)) continue;
    if (!quad.every((v) => Math.abs(v.normal[1]) < EPS)) continue;
    const [nx, , nz] = first.normal;
    const [plane, run]: [0 | 2, 0 | 2] = Math.abs(nz) > 0.5 ? [2, 0] : [0, 2];
    const at = first.pos[plane];
    const lift = Math.abs(at - Math.round(at / CELL) * CELL);
    const runs = quad.map((v) => v.pos[run]);
    const span = Math.max(...runs) - Math.min(...runs);
    if (Math.abs(lift - DECAL_LIFT) > EPS || Math.abs(span - CELL) > EPS)
      continue;
    const side: Side = nz > 0.5 ? "n" : nz < -0.5 ? "s" : nx > 0.5 ? "w" : "e";
    const mid = (k: 0 | 2) =>
      quad.reduce((sum, v) => sum + v.pos[k], 0) / quad.length;
    const ys = quad.map((v) => v.pos[1]);
    out.push({
      tint: first.tint,
      h0: Math.min(...ys),
      h1: Math.max(...ys),
      edge: edgeKey({
        x: Math.floor(mid(0) / CELL),
        y: Math.floor(mid(2) / CELL),
        side,
      }),
    });
  }
  return out;
}

describe("the accent stripe (2.7 C8, C9)", () => {
  it("runs the accent stripe on every full wall but fixture edges and the entrance (2.7 C9)", () => {
    // Mutation caught: a stripe on a door's edge, on the entrance, on a
    // lintel, at the wrong height, or tinted with a real colour instead of
    // the accent mark.
    const room = generateRoom(CANNED_BRIDGE);
    const stripes = stripeQuads(buildRoomMesh(room, LOOKS.aperture).static);
    expect(stripes.length).toBeGreaterThan(0);
    const fixtureEdges = new Set(room.fixtures.map((f) => edgeKey(f.slot)));
    const entrance = edgeKey({ ...room.entrance, side: "s" });
    for (const q of stripes) {
      expect(q.tint).toEqual(accentTint(1));
      expect(q.h0).toBeCloseTo(ACCENT_STRIPE.h0, 6);
      expect(q.h1).toBeCloseTo(ACCENT_STRIPE.h1, 6);
      expect(fixtureEdges.has(q.edge)).toBe(false);
      expect(q.edge).not.toBe(entrance);
    }
    // Every free wall edge carries one.
    const walls = wallRuns(room.grid).flat().map(edgeKey);
    const expected = walls.filter(
      (e) => !fixtureEdges.has(e) && e !== entrance,
    );
    expect(new Set(stripes.map((q) => q.edge))).toEqual(new Set(expected));
  });

  it("uploads the look's accent at the room's index, and draws no stripe in the console room (Review Focus 5)", () => {
    // Mutation caught: the stripe drawn in a room with fittings, or the
    // accent read from the wrong look after a switch.
    expect(
      stripeQuads(buildRoomMesh(consoleRoom(), LOOKS.aperture).static),
    ).toEqual([]);
    const room = {
      ...generateRoom(CANNED_BRIDGE),
      finish: { ...plainFinish(0), accent: 3 },
    };
    expect(accentFor(room, LOOKS.freescape)).toEqual(
      LOOKS.freescape.accents[3],
    );
    expect(accentFor(room, LOOKS.aperture)).toEqual(LOOKS.aperture.accents[3]);
  });
});

/**
 * The shell's full-height wall quads in a mesh (Review Focus 1): walked six
 * vertices at a time, every vertical quad on one of the wall pattern's own
 * layers (never the accent stripe) that spans the room's whole height,
 * with its layer, the uv spans `du` and `dv` across the quad, its height
 * and the cell `(cx, cy)` it faces into (read back from its position and
 * its normal, as `edgeQuad` places it).
 */
function wallQuads(m: MeshData): {
  layer: number;
  du: number;
  dv: number;
  height: number;
  cx: number;
  cy: number;
}[] {
  const vs = all(m);
  const patternLayers = new Set(WALL_PATTERN_LOOK.map((p) => p.layer));
  const candidates: {
    layer: number;
    du: number;
    dv: number;
    height: number;
    cx: number;
    cy: number;
  }[] = [];
  for (let q = 0; q + 5 < vs.length; q += 6) {
    const quad = vs.slice(q, q + 6);
    const first = quad[0];
    if (first === undefined) continue;
    if (!patternLayers.has(first.layer)) continue;
    if (quad.some((v) => v.tint[0] === ACCENT_MARK)) continue;
    if (!quad.every((v) => Math.abs(v.normal[1]) < EPS)) continue;
    const ys = quad.map((v) => v.pos[1]);
    const height = Math.max(...ys) - Math.min(...ys);
    const us = quad.map((v) => v.uv[0]);
    const vs2 = quad.map((v) => v.uv[1]);
    const du = Math.max(...us) - Math.min(...us);
    const dv = Math.max(...vs2) - Math.min(...vs2);
    const [nx, , nz] = first.normal;
    const mid = (k: 0 | 2) =>
      quad.reduce((sum, v) => sum + v.pos[k], 0) / quad.length;
    const cx = Math.floor((mid(0) + nx * CELL * 0.5 + EPS) / CELL);
    const cy = Math.floor((mid(2) + nz * CELL * 0.5 + EPS) / CELL);
    candidates.push({ layer: first.layer, du, dv, height, cx, cy });
  }
  const full = Math.max(...candidates.map((c) => c.height));
  return candidates.filter((c) => Math.abs(c.height - full) < EPS);
}

describe("wall patterns (2.7 C11)", () => {
  it("gives each bay and the corridor a pattern of their own, on the built walls (Review Focus 1)", () => {
    // Mutation caught: every wall on the hall's layer, a bay's walls on the
    // hall's pattern, or the pattern's uv scale left out (Aperture's seams
    // would not move).
    const room = generateRoom(CANNED_HUB);
    expect(room.bays.length).toBeGreaterThan(0);
    expect(room.corridor).not.toBeNull();
    const walls = wallQuads(buildRoomMesh(room, LOOKS.aperture).static);
    expect(walls.length).toBeGreaterThan(0);
    for (const w of walls) {
      const want = WALL_PATTERN_LOOK[wallPatternOf(room, w.cx, w.cy)]!;
      expect(w.layer).toBe(want.layer);
      expect(w.du / CELL).toBeCloseTo(want.u, 6); // uv run along one 2 m edge over 2 m
      expect(w.dv / w.height).toBeCloseTo(want.v, 6);
    }
    const bayCell = { x: room.bays[0]!.x0 + 1, y: room.bays[0]!.y0 + 1 };
    expect(wallPatternOf(room, bayCell.x, bayCell.y)).toBe(
      room.finish.bayWalls[0],
    );
    expect(room.finish.bayWalls[0]).not.toBe(room.finish.hallWalls);
  });
});
