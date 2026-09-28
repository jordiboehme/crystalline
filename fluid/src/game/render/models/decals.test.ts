/**
 * The decal recipe (2.7 C20, C21): every decal of a room is a quad in the
 * static mesh on the decal layer, `DECAL_LIFT` off its wall, face or
 * floor, sampling its own kind's tile of the atlas; a floor arrow points
 * the way its turn says; no wall or face decal lies over a label, a
 * screen or a mark; every stencil is set from the approved marks; and a
 * room's decals stay inside their triangle budget.
 */

import { describe, expect, it } from "vitest";

import {
  CANNED_BRIDGE,
  CANNED_HUB,
  CANNED_WORKSHOP,
  galleryRoom,
} from "../../world/canned";
import { generateRoom } from "../../world/generate";
import { HERO_FRONT } from "../../world/footprints";
import type { Decal, RoomSpec } from "../../world/types";
import { CELL } from "../../world/units";
import {
  ACCENT_STRIPE,
  accentTint,
  createBuilder,
  FLAG,
  FLOATS_PER_VERTEX,
  buildRoomMesh,
  type MeshData,
  type V3,
} from "../geometry";
import { DECAL_LIFT, createKit } from "../kit";
import { LAYER, TEXT_BASE } from "../layers";
import { LOOKS } from "../looks";
import { DECAL_TILES } from "../textures";
import { DECAL_BUDGET, DECAL_TINT, buildDecals } from "./decals";
import { MARKS, stencilMarks } from "./marks";

/** One vertex read back from a mesh. */
interface Vertex {
  p: V3;
  n: V3;
  u: number;
  v: number;
  layer: number;
  tint: V3;
  flag: number;
}

function vertexAt(mesh: MeshData, i: number): Vertex {
  const o = i * FLOATS_PER_VERTEX;
  const f = (k: number) => mesh.vertices[o + k] ?? NaN;
  return {
    p: [f(0), f(1), f(2)],
    n: [f(3), f(4), f(5)],
    u: f(6),
    v: f(7),
    layer: f(8),
    tint: [f(9), f(10), f(11)],
    flag: f(12),
  };
}

/** A quad or triangle read back as a flat rectangle in its own plane. */
interface Flat {
  normal: V3;
  /** The plane's offset along the normal, `p . n`. */
  plane: number;
  /** Its extent along the plane's horizontal axis and up (vertical planes). */
  a0: number;
  a1: number;
  y0: number;
  y1: number;
}

/** A decal quad read back from the mesh. */
interface DecalQuad extends Flat {
  flag: number;
  layer: number;
  tint: V3;
  minY: number;
  cx: number;
  cz: number;
  /** How far it stands in front of its wall or crate face, vertical quads only. */
  offWall: number;
  /** The atlas tile its uv centre falls in. */
  tile: number;
  topX: number;
  topZ: number;
  bottomX: number;
  bottomZ: number;
}

const dot = (p: V3, q: V3) => p[0] * q[0] + p[1] * q[1] + p[2] * q[2];

/** The horizontal axis of a vertical plane with normal `n`. */
const acrossOf = (n: V3): V3 => {
  const l = Math.hypot(n[0], n[2]) || 1;
  return [-n[2] / l, 0, n[0] / l];
};

function flatOf(vs: readonly Vertex[]): Flat {
  const first = vs[0]!;
  const n = first.n;
  const across = acrossOf(n);
  const as = vs.map((v) => dot(v.p, across));
  const ys = vs.map((v) => v.p[1]);
  return {
    normal: n,
    plane: dot(first.p, n),
    a0: Math.min(...as),
    a1: Math.max(...as),
    y0: Math.min(...ys),
    y1: Math.max(...ys),
  };
}

/**
 * Every decal quad of a mesh: six vertices at a time from each triangle
 * carrying `FLAG.decal` (the recipe emits every decal as whole quads, so
 * the pairs line up). A vertical quad's `offWall` is its distance in front
 * of the nearest plane behind it: a cell border (a wall) or, given the
 * room, one of its crate faces (a face decal's anchor lies on its face).
 */
function decalQuads(mesh: MeshData, room?: RoomSpec): DecalQuad[] {
  const faces = (room?.decals ?? []).filter((d) => d.on === "face");
  const out: DecalQuad[] = [];
  for (let i = 0; i + 2 < mesh.count; i += 3) {
    if (vertexAt(mesh, i).flag !== FLAG.decal) continue;
    const vs = Array.from({ length: 6 }, (_, k) => vertexAt(mesh, i + k));
    i += 3;
    const first = vs[0]!;
    const flat = flatOf(vs);
    const n = first.n;
    const c = flat.plane;
    let offWall = c - CELL * Math.floor(c / CELL + 1e-9);
    for (const f of faces) {
      const diff = c - dot([f.x * CELL, 0, f.y * CELL], n);
      if (diff > -1e-4 && diff < offWall) offWall = diff;
    }
    const u = vs.reduce((s, v) => s + v.u, 0) / 6;
    const v = vs.reduce((s, x) => s + x.v, 0) / 6;
    const vMax = Math.max(...vs.map((x) => x.v));
    const vMin = Math.min(...vs.map((x) => x.v));
    const mean = (list: Vertex[], k: 0 | 2) =>
      list.reduce((s, x) => s + x.p[k], 0) / list.length;
    const top = vs.filter((x) => x.v > vMax - 1e-6);
    const bottom = vs.filter((x) => x.v < vMin + 1e-6);
    const xs = vs.map((x) => x.p[0]);
    const zs = vs.map((x) => x.p[2]);
    out.push({
      ...flat,
      flag: first.flag,
      layer: first.layer,
      tint: first.tint,
      minY: Math.min(...vs.map((x) => x.p[1])),
      cx: (Math.min(...xs) + Math.max(...xs)) / 2,
      cz: (Math.min(...zs) + Math.max(...zs)) / 2,
      offWall,
      tile: Math.floor(v * 4) * 4 + Math.floor(u * 4),
      topX: mean(top, 0),
      topZ: mean(top, 2),
      bottomX: mean(bottom, 0),
      bottomZ: mean(bottom, 2),
    });
  }
  return out;
}

/**
 * Every vertical triangle that shows text or a screen: on a text layer
 * (`TEXT_BASE` and up), or emissive (screens, tag strips, labels' glow).
 */
function textQuads(mesh: MeshData): Flat[] {
  const out: Flat[] = [];
  for (let i = 0; i + 2 < mesh.count; i += 3) {
    const vs = [0, 1, 2].map((k) => vertexAt(mesh, i + k));
    const first = vs[0]!;
    if (first.layer < TEXT_BASE && first.flag !== FLAG.emissive) continue;
    if (Math.abs(first.n[1]) > 0.01) continue;
    out.push(flatOf(vs));
  }
  return out;
}

/**
 * Whether two flat pieces lie on one plane, facing the same way within
 * `tolerance` metres, and share area in it.
 */
function coplanarOverlap(p: Flat, q: Flat, tolerance: number): boolean {
  if (dot(p.normal, q.normal) < 0.99) return false;
  if (Math.abs(p.plane - q.plane) > tolerance) return false;
  const eps = 1e-6;
  return (
    p.a0 < q.a1 - eps &&
    q.a0 < p.a1 - eps &&
    p.y0 < q.y1 - eps &&
    q.y0 < p.y1 - eps
  );
}

/** The derelict hub the cap cuts (Task 10's `hub-1`), at 160 decals. */
const cappedHub = () =>
  generateRoom({ ...CANNED_HUB, status: "archived", permalink: "hub-1" });

describe("the decal recipe", () => {
  it("lifts every decal off its surface and maps it to its kind's tile (2.7 C21)", () => {
    // Mutation caught: a decal flush with its wall (depth fighting), a floor
    // decal under the floor, or a quad sampling another kind's tile.
    const room = generateRoom({ ...CANNED_WORKSHOP, status: "archived" });
    expect(room.decals.length).toBeGreaterThan(0);
    expect(room.decals.some((d) => d.on === "face")).toBe(true);
    const quads = decalQuads(buildRoomMesh(room, LOOKS.aperture).static, room);
    expect(quads.length).toBeGreaterThanOrEqual(room.decals.length);
    for (const q of quads) {
      expect(q.layer).toBe(LAYER.decal);
      if (q.normal[1] > 0.99) expect(q.minY).toBeCloseTo(DECAL_LIFT, 6);
      // Four places: the mesh is float32, a few micrometres at 40 m.
      else expect(q.offWall).toBeCloseTo(DECAL_LIFT, 4);
    }
  });

  it("points every built floor arrow the way its turn says", () => {
    // Mutation caught: the arrow's uv flipped (it would point into the room
    // instead of out), or the floor quad turned the wrong way.
    const room = generateRoom(CANNED_HUB);
    const arrows = room.decals.filter((d) => d.kind === "arrow");
    expect(arrows.length).toBeGreaterThan(0);
    const quads = decalQuads(buildRoomMesh(room, LOOKS.aperture).static).filter(
      (q) => q.tile === DECAL_TILES.arrow[0],
    );
    expect(quads.length).toBe(arrows.length);
    const DIR: Record<number, [number, number]> = {
      0: [0, -1],
      1: [1, 0],
      2: [0, 1],
      3: [-1, 0],
    };
    for (const q of quads) {
      const a = arrows.find(
        (d) =>
          Math.abs(d.x * CELL - q.cx) < 0.01 &&
          Math.abs(d.y * CELL - q.cz) < 0.01,
      );
      expect(a).toBeDefined();
      const [dx, dz] = DIR[a!.turn]!;
      // The vertices at the tile's top v lie further along the turn than those at its bottom v.
      expect(
        (q.topX - q.bottomX) * dx + (q.topZ - q.bottomZ) * dz,
      ).toBeGreaterThan(0.5);
    }
  });

  it("never lays a decal quad over a label, a screen or a mark (the spec's test)", () => {
    // Mutation caught: a wall decal on a text surface's plane and rectangle,
    // in any condition.
    for (const status of ["stable", "draft", "deprecated", "archived"])
      for (const p of [CANNED_BRIDGE, CANNED_WORKSHOP, CANNED_HUB]) {
        const room = generateRoom({ ...p, status });
        const mesh = buildRoomMesh(room, LOOKS.aperture).static;
        const text = textQuads(mesh); // layer >= TEXT_BASE, or emissive screens
        const decals = decalQuads(mesh).filter((q) => q.normal[1] < 0.99);
        expect(text.length).toBeGreaterThan(0);
        for (const d of decals)
          for (const t of text) expect(coplanarOverlap(d, t, 0.05)).toBe(false);
      }
  }, 30_000);

  it("sets every possible stencil through allowed marks only (Review Focus 3)", () => {
    // Mutation caught: a stencil drawn from a template string ("DECK 23"),
    // which the runtime guard would reject, or a numeral outside MARKS.
    const allowed = new Set<string>([
      MARKS.deckWord,
      MARKS.bayWord,
      ...MARKS.numerals,
      ...MARKS.bayLetters,
    ]);
    for (let n = 1; n <= 99; n++)
      for (const letter of [0, 1, 2, 3, 4])
        for (const lines of [1, 2] as const)
          for (const line of stencilMarks({ deck: n, bay: n, letter, lines }))
            for (const mark of line) expect(allowed.has(mark)).toBe(true);
  });

  it("tints each kind as C20 says, from its own tiles (2.7 C20)", () => {
    // Mutation caught: a kind drawn in another kind's colour or tile, an
    // arrow in a fixed colour instead of the room's accent, a wall stencil
    // light or a floor stencil dark, or a stencil off the solid tile.
    const room = generateRoom({ ...CANNED_HUB, status: "deprecated" });
    const quads = decalQuads(buildRoomMesh(room, LOOKS.aperture).static);
    const kinds = new Set<string>();
    const same = (p: V3, q: readonly number[]) =>
      p.every((x, i) => Math.abs(x - (q[i] ?? NaN)) < 1e-6);
    const inTiles = (k: keyof typeof DECAL_TILES, t: number) =>
      DECAL_TILES[k].includes(t);
    for (const q of quads) {
      if (inTiles("chevrons", q.tile)) {
        expect(same(q.tint, DECAL_TINT.chevrons)).toBe(true);
        kinds.add("chevrons");
      } else if (inTiles("arrow", q.tile)) {
        expect([...q.tint]).toEqual(accentTint(1));
        kinds.add("arrow");
      } else if (inTiles("grime", q.tile)) {
        expect(same(q.tint, DECAL_TINT.grime)).toBe(true);
        kinds.add("grime");
      } else if (inTiles("streak", q.tile)) {
        expect(same(q.tint, DECAL_TINT.streak)).toBe(true);
        kinds.add("streak");
      } else if (inTiles("rust", q.tile)) {
        expect(same(q.tint, DECAL_TINT.rust)).toBe(true);
        kinds.add("rust");
      } else {
        expect(q.tile).toBe(DECAL_TILES.solid[0]);
        const floor = q.normal[1] > 0.99;
        expect(
          same(
            q.tint,
            floor ? DECAL_TINT.floorStencil : DECAL_TINT.wallStencil,
          ),
        ).toBe(true);
        kinds.add(floor ? "floor stencil" : "wall stencil");
      }
    }
    expect([...kinds]).toEqual(
      expect.arrayContaining([
        "arrow",
        "chevrons",
        "floor stencil",
        "grime",
        "rust",
        "wall stencil",
      ]),
    );
  });

  it("keeps every wall decal off the accent stripe's band (2.7 C9, C21)", () => {
    // Mutation caught: a streak hung across the stripe in one quad, the
    // two coplanar at `DECAL_LIFT` and fighting for the same depth.
    let crossing = 0;
    for (const status of ["deprecated", "archived"])
      for (const p of [CANNED_BRIDGE, CANNED_WORKSHOP, CANNED_HUB]) {
        const room = generateRoom({ ...p, status });
        crossing += room.decals.filter(
          (d) =>
            d.on === "wall" &&
            d.h < ACCENT_STRIPE.h1 &&
            d.h + d.length > ACCENT_STRIPE.h0,
        ).length;
        const quads = decalQuads(buildRoomMesh(room, LOOKS.aperture).static);
        for (const q of quads) {
          if (q.normal[1] > 0.99) continue;
          expect(
            q.y0 < ACCENT_STRIPE.h1 - 1e-4 && q.y1 > ACCENT_STRIPE.h0 + 1e-4,
          ).toBe(false);
        }
      }
    // Some wall decals do cross the band, so the split is exercised.
    expect(crossing).toBeGreaterThan(0);
  });

  it(`keeps a room's decals under ${String(DECAL_BUDGET)} triangles: the hub at every condition and at the cap (2.7 C21)`, () => {
    // Mutation caught: a stencil's runs drawn as boxes (six faces each,
    // as `pixelBoxes` lays them) instead of one quad each, which takes the
    // derelict hub past the budget.
    expect(DECAL_BUDGET).toBe(1500);
    const rooms = [
      ...["stable", "draft", "deprecated", "archived"].map((status) =>
        generateRoom({ ...CANNED_HUB, status }),
      ),
      cappedHub(),
    ];
    expect(rooms.at(-1)!.decals.length).toBe(160);
    for (const room of rooms) {
      const mesh = buildRoomMesh(room, LOOKS.aperture).static;
      let vertices = 0;
      for (let i = 0; i < mesh.count; i++)
        if (mesh.vertices[i * FLOATS_PER_VERTEX + 12] === FLAG.decal)
          vertices++;
      expect(vertices).toBeGreaterThan(0);
      expect(vertices / 3).toBeLessThan(DECAL_BUDGET);
    }
  }, 30_000);

  it("lays no decal triangle on the plane of the baseboard, the stripe or any other surface it overlaps (2.7 C21)", () => {
    // Mutation caught: wall decals not cut out of the construction
    // baseboard's band (0 to 0.3 m, `DECAL_LIFT` off the wall like the
    // decals), so a low smear or a riser's bleed fights the hazard stripes
    // for depth; or not cut out of the accent stripe's band.
    const rooms: RoomSpec[] = [];
    for (const status of ["stable", "draft", "deprecated", "archived"])
      for (const p of [CANNED_BRIDGE, CANNED_WORKSHOP, CANNED_HUB])
        rooms.push(generateRoom({ ...p, status }));
    for (let i = 0; i < 12; i++)
      for (const p of [CANNED_BRIDGE, CANNED_WORKSHOP])
        rooms.push(
          generateRoom({
            ...p,
            status: "draft",
            permalink: `${p.permalink}-${String(i)}`,
          }),
        );
    let checked = 0;
    let low = 0;
    for (const room of rooms) {
      const mesh = buildRoomMesh(room, LOOKS.aperture).static;
      const key = (n: V3, plane: number) =>
        `${n[0].toFixed(2)},${n[2].toFixed(2)},${String(Math.round(plane * 100))}`;
      const others = new Map<string, Flat[]>();
      const decals: Flat[] = [];
      for (let i = 0; i + 2 < mesh.count; i += 3) {
        const vs = [0, 1, 2].map((k) => vertexAt(mesh, i + k));
        const first = vs[0]!;
        if (Math.abs(first.n[1]) > 0.01) continue;
        const f = flatOf(vs);
        if (first.flag === FLAG.decal) decals.push(f);
        else {
          const k = key(f.normal, f.plane);
          const list = others.get(k) ?? [];
          list.push(f);
          others.set(k, list);
        }
      }
      if (room.condition === "construction")
        low += room.decals.filter((d) => d.on === "wall" && d.h < 0.3).length;
      for (const d of decals) {
        checked++;
        for (const dp of [-1, 0, 1])
          for (const o of others.get(key(d.normal, d.plane + dp / 100)) ?? [])
            expect(coplanarOverlap(d, o, 1e-3)).toBe(false);
      }
    }
    expect(checked).toBeGreaterThan(1000);
    // Wall decals do reach down into the baseboard's band in construction
    // rooms, so the cut is exercised.
    expect(low).toBeGreaterThan(0);
  }, 30_000);

  it("reads every stencil the right way round on the floor at each turn and on the wall (2.7 C19)", () => {
    // Mutation caught: a stencil line laid turned half round in place
    // (mirrored left to right and flipped top to bottom), which reads
    // upside down, or the floor quad's axes swapped.
    const base = {
      kind: "stencil" as const,
      along: 0,
      variant: 0,
      seed: 0,
      width: 1.0,
      length: 0.3,
    };
    const quadsOf = (d: Decal) => {
      const b = createBuilder();
      buildDecals((f) => createKit(b, f), b, { ...galleryRoom(), decals: [d] });
      const mesh = b.build();
      expect(mesh.count % 6).toBe(0);
      return Array.from({ length: mesh.count / 6 }, (_, q) => {
        const vs = Array.from({ length: 6 }, (_, k) =>
          vertexAt(mesh, q * 6 + k),
        );
        const mean = (k: 0 | 1 | 2) => vs.reduce((s, v) => s + v.p[k], 0) / 6;
        return [mean(0), mean(1), mean(2)] as V3;
      });
    };
    for (const turn of [0, 1, 2, 3]) {
      const quads = quadsOf({
        ...base,
        on: "floor",
        x: 5.5,
        y: 5.5,
        turn,
        h: 0,
        stencil: { deck: 1, bay: 23, letter: 2, lines: 1 },
      });
      expect(quads.length).toBeGreaterThan(10);
      const [fx, fz] = HERO_FRONT[turn]!;
      const right = (p: V3) => -fz * p[0] + fx * p[2];
      const up = (p: V3) => fx * p[0] + fz * p[2];
      // The first run laid is row 0's first (top left), the last row 4's
      // last (bottom right).
      const first = quads[0]!;
      const last = quads.at(-1)!;
      expect(right(first), `turn ${String(turn)}`).toBeLessThan(
        right(last) - 0.5,
      );
      expect(up(first), `turn ${String(turn)}`).toBeGreaterThan(up(last) + 0.1);
    }
    const wall = quadsOf({
      ...base,
      on: "wall",
      x: 2.5,
      y: 3,
      turn: 0,
      h: 1.5,
      width: 0.9,
      stencil: { deck: 23, bay: 45, letter: 0, lines: 2 },
    });
    // A south wall's slot frame runs `along` towards -x; the lines' split
    // is the middle of the block (11 rows, the gap row in the middle).
    const mid = 1.5 + (11 * 0.024) / 2;
    for (const line of [
      wall.filter((p) => p[1] > mid),
      wall.filter((p) => p[1] < mid),
    ]) {
      expect(line.length).toBeGreaterThan(10);
      const first = line[0]!;
      const last = line.at(-1)!;
      expect(-first[0]).toBeLessThan(-last[0] - 0.3);
      expect(first[1]).toBeGreaterThan(last[1] + 0.05);
    }
    // DECK over BAY.
    expect(wall[0]![1]).toBeGreaterThan(mid);
  });
});
