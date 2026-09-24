/**
 * A `RoomSpec` becomes triangles: the 2D cell layout extruded into walls,
 * floor and ceiling, with every fixture built from boxes and flat panels.
 *
 * Everything is emitted into one interleaved, non-indexed vertex array in
 * world space, so the whole room is one draw call. Per vertex: position,
 * normal, a uv in metres (so panel seams fall on whole numbers and the
 * shader can draw edge lines there), the texture array layer, a tint from
 * the look, and a flag that tells the shader how the surface is lit:
 *
 * - `lit`: ordinary surface under the zone's light.
 * - `emissive`: screens and tag strips, full brightness whatever the light.
 * - `portal`: the shimmering portal surface.
 * - `frame`: a door or portal frame, which always carries a neon edge line.
 * - `lamp`: a ceiling light panel, as bright as its zone's current level.
 *
 * Every face is wound counter-clockwise seen from the side its normal points
 * to: the room shell faces inward, boxes face outward and wall panels face
 * into the room, so the renderer can cull back faces. The geometry test
 * checks every triangle's winding against its stored normal.
 *
 * Fixture recipes are written in a wall-local frame - `a` along the wall,
 * `d` out from it, `h` up - and turned into world space by the slot's wall,
 * which keeps every recipe independent of the side it stands on.
 */

import { createRng } from "../core/seed";
import { CELL } from "../world/generate";
import { FIXTURE_DEPTH, FIXTURE_WIDTH } from "../world/move";
import type { Fixture, MachineKind, RoomSpec, WallSlot } from "../world/types";
import { ASPECT, LAYER, TEXT_BASE, textRequests } from "./layers";
import { hueToRgb, type Look, type Rgb } from "./looks";

/**
 * Floats per vertex in the interleaved array: position 3, normal 3, uv 2,
 * layer 1, tint 3, flag 1. The renderer's vertex array layout reads the same
 * offsets in the same order.
 */
export const FLOATS_PER_VERTEX = 13;

/**
 * How the shader lights a surface, stored as the last float of each vertex.
 * The values are part of the contract with the shader, which compares them
 * as numbers.
 */
export const FLAG = {
  lit: 0,
  emissive: 1,
  portal: 2,
  frame: 3,
  lamp: 4,
} as const;
type Flag = (typeof FLAG)[keyof typeof FLAG];

/**
 * Interleaved vertices ready for one `drawArrays(TRIANGLES)`: `count`
 * vertices of `FLOATS_PER_VERTEX` floats each, three per triangle.
 */
export interface MeshData {
  vertices: Float32Array;
  count: number;
}

type V3 = [number, number, number];

interface Surface {
  layer: number;
  tint: Rgb;
  flag: Flag;
}

/** How far below the ceiling the top of a label stays. */
const LABEL_CLEARANCE = 0.05;

/** Grows a plain number array; turned into a Float32Array once at the end. */
function createBuilder() {
  const data: number[] = [];
  const push = (p: V3, n: V3, u: number, v: number, s: Surface) => {
    data.push(
      p[0],
      p[1],
      p[2],
      n[0],
      n[1],
      n[2],
      u,
      v,
      s.layer,
      s.tint[0],
      s.tint[1],
      s.tint[2],
      s.flag,
    );
  };
  const quad = (
    p0: V3,
    p1: V3,
    p2: V3,
    p3: V3,
    n: V3,
    uw: number,
    vh: number,
    s: Surface,
  ) => {
    push(p0, n, 0, 0, s);
    push(p1, n, uw, 0, s);
    push(p2, n, uw, vh, s);
    push(p0, n, 0, 0, s);
    push(p2, n, uw, vh, s);
    push(p3, n, 0, vh, s);
  };
  return {
    /**
     * A quad from its four corners, counter-clockwise seen from the front:
     * bottom-left, bottom-right, top-right, top-left. `uw`/`vh` are its uv
     * extent (metres for panels, 1 for a whole text layer).
     */
    quad,
    /** An axis-aligned box, all six faces facing out, uv in metres. */
    box(min: V3, max: V3, s: Surface) {
      const [x0, y0, z0] = min;
      const [x1, y1, z1] = max;
      const dx = x1 - x0;
      const dy = y1 - y0;
      const dz = z1 - z0;
      // +z, -z, +x, -x, +y, -y
      quad(
        [x0, y0, z1],
        [x1, y0, z1],
        [x1, y1, z1],
        [x0, y1, z1],
        [0, 0, 1],
        dx,
        dy,
        s,
      );
      quad(
        [x1, y0, z0],
        [x0, y0, z0],
        [x0, y1, z0],
        [x1, y1, z0],
        [0, 0, -1],
        dx,
        dy,
        s,
      );
      quad(
        [x1, y0, z1],
        [x1, y0, z0],
        [x1, y1, z0],
        [x1, y1, z1],
        [1, 0, 0],
        dz,
        dy,
        s,
      );
      quad(
        [x0, y0, z0],
        [x0, y0, z1],
        [x0, y1, z1],
        [x0, y1, z0],
        [-1, 0, 0],
        dz,
        dy,
        s,
      );
      quad(
        [x0, y1, z1],
        [x1, y1, z1],
        [x1, y1, z0],
        [x0, y1, z0],
        [0, 1, 0],
        dx,
        dz,
        s,
      );
      quad(
        [x0, y0, z0],
        [x1, y0, z0],
        [x1, y0, z1],
        [x0, y0, z1],
        [0, -1, 0],
        dx,
        dz,
        s,
      );
    },
    build(): MeshData {
      return {
        vertices: new Float32Array(data),
        count: data.length / FLOATS_PER_VERTEX,
      };
    },
  };
}

type Builder = ReturnType<typeof createBuilder>;

/**
 * A wall-local frame for a slot: `origin` is on the wall at floor level in
 * the middle of the cell, `along` runs along the wall so that `along x up`
 * is `inward`, and `inward` points into the room. That handedness is what
 * makes a panel quad built bottom-left, bottom-right, top-right, top-left in
 * `a` and `h` wind counter-clockwise seen from the room.
 */
function wallFrame(slot: WallSlot) {
  const cx = (slot.x + 0.5) * CELL;
  const cz = (slot.y + 0.5) * CELL;
  switch (slot.side) {
    case "n":
      return {
        origin: [cx, 0, slot.y * CELL] as V3,
        along: [1, 0, 0] as V3,
        inward: [0, 0, 1] as V3,
      };
    case "s":
      return {
        origin: [cx, 0, (slot.y + 1) * CELL] as V3,
        along: [-1, 0, 0] as V3,
        inward: [0, 0, -1] as V3,
      };
    case "w":
      return {
        origin: [slot.x * CELL, 0, cz] as V3,
        along: [0, 0, -1] as V3,
        inward: [1, 0, 0] as V3,
      };
    case "e":
      return {
        origin: [(slot.x + 1) * CELL, 0, cz] as V3,
        along: [0, 0, 1] as V3,
        inward: [-1, 0, 0] as V3,
      };
  }
}

type Frame = ReturnType<typeof wallFrame>;

function local(f: Frame, a: number, d: number, h: number): V3 {
  return [
    f.origin[0] + f.along[0] * a + f.inward[0] * d,
    h,
    f.origin[2] + f.along[2] * a + f.inward[2] * d,
  ];
}

/** A box given in wall-local ranges, emitted as a world-space box. */
function localBox(
  b: Builder,
  f: Frame,
  a0: number,
  a1: number,
  d0: number,
  d1: number,
  h0: number,
  h1: number,
  s: Surface,
) {
  const p = local(f, a0, d0, h0);
  const q = local(f, a1, d1, h1);
  b.box(
    [Math.min(p[0], q[0]), h0, Math.min(p[2], q[2])],
    [Math.max(p[0], q[0]), h1, Math.max(p[2], q[2])],
    s,
  );
}

/** A flat panel facing into the room at depth `d`, with a whole-layer uv. */
function localPanel(
  b: Builder,
  f: Frame,
  a0: number,
  a1: number,
  d: number,
  h0: number,
  h1: number,
  s: Surface,
  uw = 1,
  vh = 1,
) {
  b.quad(
    local(f, a0, d, h0),
    local(f, a1, d, h0),
    local(f, a1, d, h1),
    local(f, a0, d, h1),
    f.inward,
    uw,
    vh,
    s,
  );
}

/** Per machine kind: body proportions (width, depth, height) and a glowing detail height. */
const MACHINES: Record<
  MachineKind,
  { w: number; d: number; h: number; glow: number }
> = {
  workbench: { w: 1.8, d: 0.8, h: 0.9, glow: 1.3 },
  "lab-bench": { w: 1.6, d: 0.7, h: 1.0, glow: 1.6 },
  "server-rack": { w: 0.8, d: 0.8, h: 2.2, glow: 1.8 },
  "cryo-pod": { w: 1.0, d: 0.9, h: 2.0, glow: 1.2 },
  fabricator: { w: 1.6, d: 0.9, h: 1.6, glow: 1.0 },
  hydroponics: { w: 1.8, d: 0.6, h: 1.2, glow: 1.1 },
  "nav-table": { w: 1.4, d: 0.9, h: 1.0, glow: 1.05 },
  "comms-array": { w: 1.2, d: 0.5, h: 2.4, glow: 2.1 },
  "reactor-coupling": { w: 1.2, d: 0.9, h: 1.9, glow: 0.9 },
  "cargo-loader": { w: 1.8, d: 0.9, h: 1.4, glow: 1.5 },
  "med-scanner": { w: 1.0, d: 0.9, h: 1.8, glow: 1.4 },
  containment: { w: 1.2, d: 0.9, h: 2.1, glow: 1.0 },
};

function fixture(
  b: Builder,
  room: RoomSpec,
  fx: Fixture,
  index: number,
  textLayer: Map<string, number>,
  look: Look,
) {
  const f = wallFrame(fx.slot);
  const p = look.palette;
  const metal: Surface = { layer: LAYER.metal, tint: p.metal, flag: FLAG.lit };
  const text = (key: string, flag: Flag, tint: Rgb): Surface => ({
    layer: textLayer.get(key) ?? LAYER.panel,
    tint,
    flag,
  });
  // A label above a door, portal or tag strip, clamped under a low ceiling.
  const labelTop = (bottom: number) =>
    Math.min(bottom + 1.8 / ASPECT.label, room.ceiling - LABEL_CLEARANCE);
  switch (fx.kind) {
    case "terminal": {
      localBox(b, f, -0.6, 0.6, 0, 0.7, 0, 0.9, metal);
      localBox(b, f, -0.6, 0.6, 0, 0.15, 0.9, 1.95, metal);
      const h = 0.8 / ASPECT.screen;
      localPanel(
        b,
        f,
        -0.5,
        0.5,
        0.151,
        1.02,
        1.02 + h * 1.25,
        text(`terminal:${index}`, FLAG.emissive, [1, 1, 1]),
      );
      break;
    }
    case "door": {
      const frame: Surface = {
        layer: LAYER.metal,
        tint: fx.style === "sliding" ? p.metal : p.door,
        flag:
          fx.style === "blast" || look.edge.everywhere ? FLAG.frame : FLAG.lit,
      };
      const t =
        fx.style === "sliding" ? 0.12 : fx.style === "bulkhead" ? 0.22 : 0.3;
      localBox(b, f, -1, -0.8, 0, t, 0, 2.6, frame);
      localBox(b, f, 0.8, 1, 0, t, 0, 2.6, frame);
      localBox(b, f, -1, 1, 0, t, 2.4, 2.6, frame);
      localPanel(
        b,
        f,
        -0.8,
        0.8,
        0.04,
        0,
        2.4,
        {
          layer: fx.style === "bulkhead" ? LAYER.hazard : LAYER.metal,
          tint: p.door,
          flag: FLAG.lit,
        },
        1.6,
        2.4,
      );
      localPanel(
        b,
        f,
        -0.9,
        0.9,
        t + 0.001,
        2.62,
        labelTop(2.62),
        text(`door:${index}`, FLAG.emissive, [1, 1, 1]),
      );
      break;
    }
    case "portal": {
      const colour = fx.crossDomain ? p.portalAlt : p.portal;
      const frame: Surface = {
        layer: LAYER.metal,
        tint: colour,
        flag: FLAG.frame,
      };
      localBox(b, f, -1, -0.85, 0, 0.18, 0, 2.7, frame);
      localBox(b, f, 0.85, 1, 0, 0.18, 0, 2.7, frame);
      localBox(b, f, -1, 1, 0, 0.18, 2.55, 2.7, frame);
      localBox(b, f, -1, 1, 0, 0.18, 0, 0.1, frame);
      localPanel(
        b,
        f,
        -0.85,
        0.85,
        0.08,
        0.1,
        2.55,
        fx.sealed
          ? { layer: LAYER.hazard, tint: p.metal, flag: FLAG.lit }
          : { layer: LAYER.portal, tint: colour, flag: FLAG.portal },
        1.7,
        2.45,
      );
      localPanel(
        b,
        f,
        -0.9,
        0.9,
        0.181,
        2.72,
        labelTop(2.72),
        text(`portal:${index}`, FLAG.emissive, [1, 1, 1]),
      );
      break;
    }
    case "machine": {
      const m = MACHINES[fx.machine];
      const rng = createRng(fx.seed);
      const hw = m.w / 2;
      const depth = Math.min(m.d, FIXTURE_DEPTH);
      const body: Surface = {
        layer: LAYER.panel,
        tint: p.machine,
        flag: FLAG.lit,
      };
      localBox(b, f, -hw, hw, 0.05, depth, 0, m.h, body);
      const hue = hueToRgb(fx.hue, 0.85, 0.55);
      const detailW = rng.range(0.3, hw * 0.8);
      localPanel(
        b,
        f,
        -detailW,
        detailW,
        depth + 0.002,
        m.glow,
        m.glow + 0.12,
        {
          layer: LAYER.panel,
          tint: hue,
          flag: FLAG.emissive,
        },
      );
      // The tag strip along the wall above, in the tag's colour, and its name.
      localBox(
        b,
        f,
        -FIXTURE_WIDTH / 2 + 0.05,
        FIXTURE_WIDTH / 2 - 0.05,
        0,
        0.03,
        2.5,
        2.58,
        { layer: LAYER.panel, tint: hue, flag: FLAG.emissive },
      );
      localPanel(
        b,
        f,
        -0.9,
        0.9,
        0.031,
        2.6,
        labelTop(2.6),
        text(`tag:${index}`, FLAG.emissive, hue),
      );
      break;
    }
    case "placard": {
      localPanel(
        b,
        f,
        -0.5,
        0.5,
        0.02,
        1.3,
        1.3 + 1 / ASPECT.placard,
        text("placard", FLAG.lit, p.panel),
      );
      break;
    }
  }
}

/**
 * The whole room as one vertex array: floor and ceiling, the four walls
 * facing in, a lamp panel per light zone, the hazard baseboards and scaffold
 * of a room under construction, and every fixture. Text quads take their
 * layer from `textRequests`, request `i` in layer `TEXT_BASE + i`. Pure and
 * deterministic: the same room and look give the same floats.
 */
export function buildRoomMesh(room: RoomSpec, look: Look): MeshData {
  const b = createBuilder();
  const p = look.palette;
  const W = room.width * CELL;
  const D = room.depth * CELL;
  const H = room.ceiling;
  const wall: Surface = { layer: LAYER.panel, tint: p.panel, flag: FLAG.lit };

  // Floor (facing up) and ceiling (facing down), uv in metres.
  b.quad([0, 0, D], [W, 0, D], [W, 0, 0], [0, 0, 0], [0, 1, 0], W, D, {
    layer: LAYER.floor,
    tint: p.floor,
    flag: FLAG.lit,
  });
  b.quad([0, H, 0], [W, H, 0], [W, H, D], [0, H, D], [0, -1, 0], W, D, {
    layer: LAYER.ceiling,
    tint: p.ceiling,
    flag: FLAG.lit,
  });
  // The four walls, facing in.
  b.quad([0, 0, 0], [W, 0, 0], [W, H, 0], [0, H, 0], [0, 0, 1], W, H, wall);
  b.quad([W, 0, D], [0, 0, D], [0, H, D], [W, H, D], [0, 0, -1], W, H, wall);
  b.quad([0, 0, D], [0, 0, 0], [0, H, 0], [0, H, D], [1, 0, 0], D, H, wall);
  b.quad([W, 0, 0], [W, 0, D], [W, H, D], [W, H, 0], [-1, 0, 0], D, H, wall);

  // One lamp panel in the middle of every light zone, just under the ceiling.
  for (const z of room.lights) {
    const cx = ((z.x0 + z.x1) / 2) * CELL;
    const cz = ((z.y0 + z.y1) / 2) * CELL;
    const y = H - 0.01;
    b.quad(
      [cx - 0.8, y, cz - 0.3],
      [cx + 0.8, y, cz - 0.3],
      [cx + 0.8, y, cz + 0.3],
      [cx - 0.8, y, cz + 0.3],
      [0, -1, 0],
      1,
      1,
      { layer: LAYER.ceiling, tint: p.lamp, flag: FLAG.lamp },
    );
  }

  // Under construction: hazard-striped baseboards and two scaffold frames.
  if (room.condition === "construction") {
    const hazard: Surface = {
      layer: LAYER.hazard,
      tint: [1, 1, 1],
      flag: FLAG.lit,
    };
    b.quad(
      [0, 0, 0.01],
      [W, 0, 0.01],
      [W, 0.3, 0.01],
      [0, 0.3, 0.01],
      [0, 0, 1],
      W,
      0.3,
      hazard,
    );
    b.quad(
      [W, 0, D - 0.01],
      [0, 0, D - 0.01],
      [0, 0.3, D - 0.01],
      [W, 0.3, D - 0.01],
      [0, 0, -1],
      W,
      0.3,
      hazard,
    );
    const rng = createRng(room.seed);
    for (let i = 0; i < 2; i++) {
      const x = rng.range(CELL * 1.5, W - CELL * 1.5);
      const z = rng.range(CELL * 1.5, D - CELL * 1.5);
      const s: Surface = { layer: LAYER.metal, tint: p.door, flag: FLAG.lit };
      for (const [dx, dz] of [
        [-0.6, -0.6],
        [0.6, -0.6],
        [-0.6, 0.6],
        [0.6, 0.6],
      ] as const) {
        b.box(
          [x + dx - 0.04, 0, z + dz - 0.04],
          [x + dx + 0.04, H - 0.2, z + dz + 0.04],
          s,
        );
      }
      b.box([x - 0.7, 2.2, z - 0.7], [x + 0.7, 2.28, z + 0.7], s);
    }
  }

  const textLayer = new Map(
    textRequests(room).map((r, i) => [r.key, TEXT_BASE + i]),
  );
  room.fixtures.forEach((fx, i) => {
    fixture(b, room, fx, i, textLayer, look);
  });
  return b.build();
}
