import { describe, expect, it } from "vitest";

import { CANNED_BRIDGE } from "../world/canned";
import { CELL, generateRoom } from "../world/generate";
import type { RoomSpec } from "../world/types";
import {
  FLAG,
  FLOATS_PER_VERTEX,
  buildRoomMesh,
  type MeshData,
} from "./geometry";
import { TEXT_BASE, layerCount } from "./layers";
import { LOOKS } from "./looks";

const room = generateRoom(CANNED_BRIDGE);
const mesh = buildRoomMesh(room, LOOKS.day);

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

function all(m: MeshData = mesh) {
  return Array.from({ length: m.count }, (_, i) => vertexOf(m, i));
}

/**
 * For every triangle, the geometric normal of its winding (counter-clockwise
 * seen from the front) against the normal stored on its vertices: the two
 * must point the same way, or back-face culling would drop a face that
 * should be seen. Returns the smallest dot product of the two unit normals.
 */
function worstWinding(m: MeshData): number {
  const vs = all(m);
  let worst = Infinity;
  for (let t = 0; t + 2 < vs.length; t += 3) {
    const [a, b, c] = [vs[t], vs[t + 1], vs[t + 2]];
    if (!a || !b || !c) throw new Error("short triangle");
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

describe("buildRoomMesh", () => {
  it("is whole triangles with nothing left over", () => {
    expect(mesh.count % 3).toBe(0);
    expect(mesh.vertices.length).toBe(mesh.count * FLOATS_PER_VERTEX);
  });

  it("stays inside the room shell", () => {
    for (const v of all()) {
      const [x, y, z] = v.pos;
      expect(x).toBeGreaterThanOrEqual(-1e-4);
      expect(x).toBeLessThanOrEqual(room.width * CELL + 1e-4);
      expect(z).toBeGreaterThanOrEqual(-1e-4);
      expect(z).toBeLessThanOrEqual(room.depth * CELL + 1e-4);
      expect(y).toBeGreaterThanOrEqual(-1e-4);
      expect(y).toBeLessThanOrEqual(room.ceiling + 1e-4);
    }
  });

  it("keeps labels under a low ceiling", () => {
    const low: RoomSpec = { ...room, ceiling: 3 };
    for (const v of all(buildRoomMesh(low, LOOKS.day))) {
      expect(v.pos[1]).toBeLessThanOrEqual(low.ceiling + 1e-4);
    }
  });

  it("uses unit normals and layers the texture array has", () => {
    const layers = layerCount(room);
    for (const v of all()) {
      expect(Math.hypot(...v.normal)).toBeCloseTo(1, 5);
      expect(Number.isInteger(v.layer)).toBe(true);
      expect(v.layer).toBeGreaterThanOrEqual(0);
      expect(v.layer).toBeLessThan(layers);
    }
  });

  it("winds every triangle counter-clockwise seen from its normal's side", () => {
    expect(worstWinding(mesh)).toBeGreaterThan(0.999);
    const building = buildRoomMesh(
      generateRoom({ ...CANNED_BRIDGE, status: "draft" }),
      LOOKS.day,
    );
    expect(worstWinding(building)).toBeGreaterThan(0.999);
  });

  it("draws every text layer somewhere", () => {
    const used = new Set(all().map((v) => v.layer));
    for (let l = TEXT_BASE; l < layerCount(room); l++)
      expect(used.has(l)).toBe(true);
  });

  it("has a portal surface and door frames", () => {
    const flags = new Set(all().map((v) => v.flag));
    expect(flags.has(FLAG.portal)).toBe(true);
    expect(flags.has(FLAG.frame)).toBe(true);
    expect(flags.has(FLAG.lamp)).toBe(true);
  });

  it("tints the cross-domain portal in the look's other portal colour", () => {
    const alt = LOOKS.day.palette.portalAlt;
    const portal = all().filter((v) => v.flag === FLAG.portal);
    expect(portal.length).toBeGreaterThan(0);
    for (const v of portal) {
      v.tint.forEach((c, i) => expect(c).toBeCloseTo(alt[i] ?? NaN, 5));
    }
  });

  it("is the same every time", () => {
    const again = buildRoomMesh(room, LOOKS.day);
    expect(Array.from(again.vertices)).toEqual(Array.from(mesh.vertices));
  });

  it("adds hazard stripes to a room under construction", () => {
    const building = generateRoom({ ...CANNED_BRIDGE, status: "draft" });
    const built = buildRoomMesh(building, LOOKS.day);
    const layers = new Set(
      Array.from(
        { length: built.count },
        (_, i) => built.vertices[i * FLOATS_PER_VERTEX + 8],
      ),
    );
    expect(layers.has(4)).toBe(true);
  });
});
