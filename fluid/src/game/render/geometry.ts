/**
 * A `RoomSpec` becomes triangles: the room's cell grid built into floor,
 * ceiling and walls, the detailed models of every fixture and piece of
 * furniture, the lamps, and the scaffolding of a room under construction.
 *
 * Everything static is emitted into one interleaved, non-indexed vertex
 * array in world space, so the whole room is one draw call; the moving
 * parts of every way come back as movers, each its own small mesh, for the
 * renderer to slide, blink, scale or turn, and after them every hero's
 * moving parts (`buildHeroMovers`: a police box's two door leaves, keyed
 * by the hero's index in `room.heroes`), and last the console room
 * fittings' (`buildInteriorMovers`, by the piece's index in
 * `room.interior`). The heroes and the fittings themselves are drawn
 * instanced, without those parts. Per vertex: position, normal, a uv in
 * metres (so panel seams fall on whole numbers and the shader can draw
 * edge lines there), the texture array layer, a tint from the look, and a
 * flag that tells the shader how the surface is lit:
 *
 * - `lit`: ordinary surface under the zone's light.
 * - `emissive`: screens and tag strips, full brightness whatever the light.
 * - `portal`: the shimmering portal surface.
 * - `frame`: a door or portal frame, which always carries a neon edge line.
 * - `lamp`: a ceiling light panel, as bright as its zone's current level.
 * - `signal`: a light that shines by itself, never dimmed by the room
 *   (H12): the doors' warning lamps and the heroes' steady lights.
 * - `blink` to `blink + 7`: a signal light whose gain is its blink
 *   channel's, one flag per group of the hero's blink bank (`blink.ts`).
 *
 * Every face is wound counter-clockwise seen from the side its normal points
 * to: the room shell faces inward, boxes face outward and wall panels face
 * into the room, so the renderer can cull back faces. The geometry test
 * checks every triangle's winding against its stored normal.
 *
 * The shell is built cell by cell: a floor and a ceiling quad per floor
 * cell and a wall quad on every edge where a floor cell meets void or the
 * grid's edge, the same rule `wallSlots` uses for the walls fixtures stand
 * against. The uv of every shell quad is its world position in metres, so
 * the seams run on unbroken across cells. A room with `interior` (the
 * console room, 2.6e C4) draws its shell in `CONSOLE_SHELL`'s fixed tints
 * on the same layers, whatever the look; its flush fittings cover the
 * walls. Every other room's full wall quads carry the accent stripe
 * (`ACCENT_STRIPE`, 2.7 C9), a band in the accent mark (`accentTint`), off
 * fixture edges, the entrance edge and lintels. The models themselves are
 * built by the recipes in `models/`, with the modelling kit of `kit.ts`.
 */

import { lampBoxes } from "../world/lamps";
import { STEP, doorwayColumns, isFloor } from "../world/layout";
import type { Box, RoomSpec, Side } from "../world/types";
import { CELL } from "../world/units";
import { BLINK_GROUPS } from "./blink";
import { DECAL_LIFT, createKit } from "./kit";
import { LAYER, layerPlan } from "./layers";
import type { Look, Rgb } from "./looks";
import {
  buildDecor,
  buildFixture,
  type KitAt,
  type ModelContext,
  type Mover,
} from "./models";
import { buildHeroMovers } from "./models/heroes";
import { buildInteriorMovers } from "./models/interior";
import { CONSOLE_SHELL } from "./models/interior/common";

/**
 * Floats per vertex in the interleaved array: position 3, normal 3, uv 2,
 * layer 1, tint 3, flag 1. The renderer's vertex array layout reads the same
 * offsets in the same order.
 */
export const FLOATS_PER_VERTEX = 13;

/**
 * How the shader lights a surface, stored as the last float of each vertex.
 * The values are part of the contract with the shader, which compares them
 * as numbers (the shader's source emits them from here).
 *
 * - `lit`, `emissive`, `portal`, `frame` and `lamp`: see the module doc.
 *   `lamp` is the ceiling panels' flag and follows its cell's light.
 * - `signal`: a light that shines by itself, never dimmed by the room
 *   (H12): the warning lamps and hero lights. Its colour is its tint times
 *   `SIGNAL_GAIN` times `uGain`, with no room light in it.
 * - `blink`: the first of `BLINK_GROUPS` flags `blink + g`, a signal light
 *   whose gain is its blink channel's: the group `g` of the bank named by
 *   the instance slot (`blinkFlag`, H11).
 */
export const FLAG = {
  lit: 0,
  emissive: 1,
  portal: 2,
  frame: 3,
  lamp: 4,
  signal: 5,
  blink: 6,
} as const;

/** A blink group's flag: `FLAG.blink + group`, group 0 to 7. */
export type BlinkFlag = 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13;

/** Every flag a surface may carry: one of `FLAG`, or a blink group's. */
type Flag = (typeof FLAG)[keyof typeof FLAG] | BlinkFlag;

/** The flag of blink group `group`; throws outside 0 to `BLINK_GROUPS - 1`. */
export function blinkFlag(group: number): BlinkFlag {
  if (!Number.isInteger(group) || group < 0 || group >= BLINK_GROUPS)
    throw new Error(`blinkFlag: no group ${String(group)}`);
  return (FLAG.blink + group) as BlinkFlag;
}

/**
 * Interleaved vertices ready for one `drawArrays(TRIANGLES)`: `count`
 * vertices of `FLOATS_PER_VERTEX` floats each, three per triangle.
 */
export interface MeshData {
  vertices: Float32Array;
  count: number;
}

/** A point or direction in world space (or a frame's local space). */
export type V3 = [number, number, number];

/**
 * What a face is made of: its texture array layer, the look's tint and the
 * lighting flag. Every vertex of a face carries the same surface, and the
 * modelling kit takes one per primitive.
 */
export interface Surface {
  layer: number;
  tint: Rgb;
  flag: Flag;
}

/**
 * The accent mark (2.7 C8): a tint whose first channel is this negative
 * number is no colour but "the room's accent, times the second channel".
 * No real colour has a negative channel, so the vertex shader tells the two
 * apart by the first channel's sign and swaps a marked tint for `uAccent`
 * (the look's accent at the room's index, `accentFor` in `looks.ts`) times
 * `k`. So a mesh that carries the accent stays one mesh per look: the
 * static room and the instanced families take the room's accent at draw
 * time, never at build time.
 */
export const ACCENT_MARK = -1;

/**
 * The accent mark's tint (2.7 C8): `[ACCENT_MARK, k, 0]`, the room's
 * accent scaled by `k` (1 the accent itself, below 1 a darker shade of it).
 * A surface carrying it is drawn in the room's accent in every look.
 */
export function accentTint(k: number): Rgb {
  return [ACCENT_MARK, k, 0];
}

/**
 * The accent stripe (2.7 C9): a band 0.08 m tall, from `h0` to `h1`
 * metres, one `DECAL_LIFT` proud of every full wall quad of the hall, the
 * bays and the corridor, except fixture edges, the entrance edge and
 * lintels. It carries the accent mark, so it takes the room's accent in
 * every look.
 */
export const ACCENT_STRIPE = { h0: 1.2, h1: 1.28 } as const;

/** Floats the builder reserves at first: room for 1024 vertices. */
const INITIAL_FLOATS = 1024 * FLOATS_PER_VERTEX;

/**
 * Writes interleaved vertices straight into a Float32Array that doubles its
 * capacity whenever it fills, and hands out a trimmed copy at the end. The
 * room mesh and the modelling kit (`kit.ts`) emit into the same builder, so
 * a room with all its models stays one vertex array and one draw call. A
 * hub's worth of models is millions of floats, which is why they never go
 * through a plain number array first.
 */
export function createBuilder() {
  let data = new Float32Array(INITIAL_FLOATS);
  let length = 0;
  const push = (p: V3, n: V3, u: number, v: number, s: Surface) => {
    if (length + FLOATS_PER_VERTEX > data.length) {
      const grown = new Float32Array(data.length * 2);
      grown.set(data);
      data = grown;
    }
    const d = data;
    let i = length;
    d[i++] = p[0];
    d[i++] = p[1];
    d[i++] = p[2];
    d[i++] = n[0];
    d[i++] = n[1];
    d[i++] = n[2];
    d[i++] = u;
    d[i++] = v;
    d[i++] = s.layer;
    d[i++] = s.tint[0];
    d[i++] = s.tint[1];
    d[i++] = s.tint[2];
    d[i++] = s.flag;
    length = i;
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
     * One vertex: world position, unit normal, uv and surface. Three calls
     * make a triangle, wound counter-clockwise seen from the side the normal
     * points to. The modelling kit emits its faces through this.
     */
    vertex: push,
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
    /**
     * The vertices so far, trimmed to their length. A copy, so the builder
     * can go on growing without changing a mesh already handed out.
     */
    build(): MeshData {
      return {
        vertices: data.slice(0, length),
        count: length / FLOATS_PER_VERTEX,
      };
    },
  };
}

/** The builder `createBuilder` returns, as the modelling kit takes it. */
export type Builder = ReturnType<typeof createBuilder>;

/**
 * The height of a doorway between the hall and a bay or the backlink
 * corridor: above it a lintel closes the gap to the ceiling on both faces of
 * the opening, so the way through reads as a doorway and not as a gap in
 * the wall.
 */
export const LINTEL = 2.4;

/** How tall the hazard-striped baseboards of a room under construction are. */
const BASEBOARD = 0.3;
/** How far the baseboards stand off their wall, against z-fighting. */
const BASEBOARD_INSET = 0.01;

/** How far below the ceiling a lamp panel hangs, against z-fighting. */
const LAMP_DROP = 0.01;

/** A scaffold pole's thickness and its inset from its frame's corner. */
const POLE = 0.08;
const POLE_INSET = 0.06;
/** How far below the ceiling the scaffold poles stop. */
const POLE_CLEARANCE = 0.2;
/** Heights of the scaffold's rails: a mid rail and the top frame. */
const MID_RAIL = 1.1;
const TOP_RAIL = 2.2;

/**
 * The static room and its moving parts: `static` is the one vertex array
 * of shell, lamps, scaffolding and every model's fixed parts; `movers` are
 * the moving parts of every way at rest, by `Mover.key`: each door's
 * leaves (`door:<i>`), lamp (`lamp:<i>`) and sparks (`spark:<i>`), each
 * hatch's lid (`lid:<i>`) and each portal's disc (`disc:<i>`).
 */
export interface RoomMesh {
  static: MeshData;
  movers: Mover[];
}

const SIDES: readonly Side[] = ["n", "e", "s", "w"];

/**
 * One vertical quad on a side of cell `(x, y)`, from `h0` to `h1`, `inset`
 * metres into the cell, facing into it. Its uv is the world position along
 * the wall and the height, in metres, so neighbouring quads continue each
 * other's seams.
 */
function edgeQuad(
  b: Builder,
  x: number,
  y: number,
  side: Side,
  h0: number,
  h1: number,
  inset: number,
  s: Surface,
) {
  const X0 = x * CELL;
  const X1 = X0 + CELL;
  const Z0 = y * CELL;
  const Z1 = Z0 + CELL;
  // Bottom-left and bottom-right seen from inside the cell, and the normal.
  let l: [number, number];
  let r: [number, number];
  let n: V3;
  switch (side) {
    case "n":
      [l, r, n] = [
        [X0, Z0 + inset],
        [X1, Z0 + inset],
        [0, 0, 1],
      ];
      break;
    case "s":
      [l, r, n] = [
        [X1, Z1 - inset],
        [X0, Z1 - inset],
        [0, 0, -1],
      ];
      break;
    case "w":
      [l, r, n] = [
        [X0 + inset, Z1],
        [X0 + inset, Z0],
        [1, 0, 0],
      ];
      break;
    case "e":
      [l, r, n] = [
        [X1 - inset, Z0],
        [X1 - inset, Z1],
        [-1, 0, 0],
      ];
      break;
  }
  const u = (p: [number, number]) =>
    side === "n" || side === "s" ? p[0] : p[1];
  b.vertex([l[0], h0, l[1]], n, u(l), h0, s);
  b.vertex([r[0], h0, r[1]], n, u(r), h0, s);
  b.vertex([r[0], h1, r[1]], n, u(r), h1, s);
  b.vertex([l[0], h0, l[1]], n, u(l), h0, s);
  b.vertex([r[0], h1, r[1]], n, u(r), h1, s);
  b.vertex([l[0], h1, l[1]], n, u(l), h1, s);
}

/**
 * A horizontal quad over the floor rectangle `x0..x1` by `z0..z1` at height
 * `h`, facing up or down, with its world x and z as uv.
 */
function flatQuad(
  b: Builder,
  x0: number,
  x1: number,
  z0: number,
  z1: number,
  h: number,
  up: boolean,
  s: Surface,
) {
  // Counter-clockwise seen from above for the floor, from below for the
  // ceiling.
  const corners: [number, number][] = up
    ? [
        [x0, z1],
        [x1, z1],
        [x1, z0],
        [x0, z0],
      ]
    : [
        [x0, z0],
        [x1, z0],
        [x1, z1],
        [x0, z1],
      ];
  const n: V3 = [0, up ? 1 : -1, 0];
  for (const i of [0, 1, 2, 0, 2, 3]) {
    const c = corners[i];
    if (c !== undefined) b.vertex([c[0], h, c[1]], n, c[0], c[1], s);
  }
}

/**
 * A scaffold frame standing on `box`: a pole in each corner, a frame of
 * four rails round the top, and two rails halfway up that run along x on
 * the frame's north and south sides, all inside the box, so what is
 * drawn is exactly what `room.scaffold` makes the player walk around.
 */
function scaffold(b: Builder, box: Box, ceiling: number, s: Surface) {
  const top = ceiling - POLE_CLEARANCE;
  const xs = [box.x0 + POLE_INSET, box.x1 - POLE_INSET - POLE];
  const zs = [box.z0 + POLE_INSET, box.z1 - POLE_INSET - POLE];
  for (const x of xs)
    for (const z of zs) b.box([x, 0, z], [x + POLE, top, z + POLE], s);
  const [ax, bx] = [xs[0] ?? box.x0, (xs[1] ?? box.x1) + POLE];
  const [az, bz] = [zs[0] ?? box.z0, (zs[1] ?? box.z1) + POLE];
  const rail = Math.min(TOP_RAIL, top - POLE);
  // The top frame: two rails along x, two along z.
  for (const z of zs) b.box([ax, rail, z], [bx, rail + POLE, z + POLE], s);
  for (const x of xs) b.box([x, rail, az], [x + POLE, rail + POLE, bz], s);
  // Mid rails along x.
  for (const z of zs)
    b.box([ax, MID_RAIL, z], [bx, MID_RAIL + POLE, z + POLE], s);
}

/**
 * The whole room: the shell built on the grid, a lamp panel per light zone,
 * the hazard baseboards and scaffold frames of a room under construction,
 * and every fixture and piece of furniture as its detailed model. Text
 * quads take their layer and row from `layerPlan(room)`. Pure and
 * deterministic: the same room and look give the same floats.
 *
 * Shell rules: every floor cell gets a floor and a ceiling quad; every edge
 * of a floor cell whose neighbour is void or outside the grid gets a wall
 * quad from the floor to the ceiling, facing into the cell, so no wall ever
 * stands between two floor cells. Where a doorway cell (see
 * `doorwayColumns`) meets the hall, a bay or the corridor, a lintel runs
 * from `LINTEL` to the ceiling on both faces of the edge. A full wall quad
 * on an edge that is neither a fixture's slot nor the entrance's carries
 * the accent stripe, unless the room has fittings (`interior`). Lamps and
 * scaffolding share the hall's ceiling height, which bays and the corridor
 * share too.
 */
export function buildRoomMesh(room: RoomSpec, look: Look): RoomMesh {
  const b = createBuilder();
  const p = look.palette;
  const H = room.ceiling;
  const doorways = doorwayColumns(room);
  // A room with fittings (the console room) keeps its own colours in
  // every look (2.6e C4); every other room takes the look's.
  const fitted = room.interior !== undefined;
  const wall: Surface = {
    layer: LAYER.panel,
    tint: fitted ? CONSOLE_SHELL.wall : p.panel,
    flag: FLAG.lit,
  };
  const floor: Surface = {
    layer: LAYER.floor,
    tint: fitted ? CONSOLE_SHELL.floor : p.floor,
    flag: FLAG.lit,
  };
  const ceiling: Surface = {
    layer: LAYER.ceiling,
    tint: fitted ? CONSOLE_SHELL.ceiling : p.ceiling,
    flag: FLAG.lit,
  };
  const building = room.condition === "construction";
  const hazard: Surface = {
    layer: LAYER.hazard,
    tint: [1, 1, 1] as Rgb,
    flag: FLAG.lit,
  };
  // The accent stripe (2.7 C9) runs on the wall's own layer in the room's
  // accent, and skips every fixture's edge and the entrance's. A room with
  // fittings draws none.
  const stripe: Surface = {
    layer: wall.layer,
    tint: accentTint(1),
    flag: FLAG.lit,
  };
  const fixtureEdges = new Set(
    room.fixtures.map((f) => `${f.slot.x},${f.slot.y},${f.slot.side}`),
  );
  const entranceEdge = `${room.entrance.x},${room.entrance.y},s`;
  // Wall slots that are ways through keep their wall clear of baseboards.
  const openings = new Set(
    room.fixtures
      .filter(
        (f) => f.kind === "door" || f.kind === "portal" || f.kind === "hatch",
      )
      .map((f) => `${f.slot.x},${f.slot.y},${f.slot.side}`),
  );

  for (let y = 0; y < room.depth; y++) {
    for (let x = 0; x < room.width; x++) {
      if (!isFloor(room.grid, x, y)) continue;
      const X0 = x * CELL;
      const Z0 = y * CELL;
      flatQuad(b, X0, X0 + CELL, Z0, Z0 + CELL, 0, true, floor);
      flatQuad(b, X0, X0 + CELL, Z0, Z0 + CELL, H, false, ceiling);
      for (const side of SIDES) {
        const [dx, dy] = STEP[side];
        const nx = x + dx;
        const ny = y + dy;
        if (!isFloor(room.grid, nx, ny)) {
          edgeQuad(b, x, y, side, 0, H, 0, wall);
          const key = `${x},${y},${side}`;
          if (!fitted && !fixtureEdges.has(key) && key !== entranceEdge)
            edgeQuad(
              b,
              x,
              y,
              side,
              ACCENT_STRIPE.h0,
              ACCENT_STRIPE.h1,
              DECAL_LIFT,
              stripe,
            );
          if (building && !openings.has(`${x},${y},${side}`))
            edgeQuad(b, x, y, side, 0, BASEBOARD, BASEBOARD_INSET, hazard);
        } else if (doorways.has(x) !== doorways.has(nx) && H > LINTEL) {
          edgeQuad(b, x, y, side, LINTEL, H, 0, wall);
        }
      }
    }
  }

  // One lamp panel per light zone, just under the ceiling.
  for (const l of lampBoxes(room))
    flatQuad(b, l.x0, l.x1, l.z0, l.z1, H - LAMP_DROP, false, {
      layer: LAYER.ceiling,
      tint: p.lamp,
      flag: FLAG.lamp,
    });

  // Under construction: the scaffold frames the player walks around.
  const pole: Surface = { layer: LAYER.metal, tint: p.door, flag: FLAG.lit };
  for (const box of room.scaffold) scaffold(b, box, H, pole);

  const plan = layerPlan(room);
  const ctx: ModelContext = {
    look,
    ceiling: H,
    hall: room.hall,
    textLayer: (key) => plan.lookup(key),
  };
  const kitAt: KitAt = (f) => createKit(b, f);
  const movers: Mover[] = [];
  room.fixtures.forEach((fx, i) => {
    movers.push(...buildFixture(kitAt, fx, i, ctx));
  });
  room.heroes.forEach((h, i) => movers.push(...buildHeroMovers(h, i, look)));
  (room.interior ?? []).forEach((piece, i) =>
    movers.push(...buildInteriorMovers(piece, i, look)),
  );
  for (const d of room.decor) buildDecor(kitAt, d, ctx);
  return { static: b.build(), movers };
}
