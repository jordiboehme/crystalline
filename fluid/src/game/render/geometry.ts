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
 * - `decal`: a decal (2.7 C20), lit as `lit` but with no edge lines, and
 *   first alpha-tested against an ordered threshold.
 *
 * Every face is wound counter-clockwise seen from the side its normal points
 * to: the room shell faces inward, boxes face outward and wall panels face
 * into the room, so the renderer can cull back faces. The geometry test
 * checks every triangle's winding against its stored normal.
 *
 * The shell is built cell by cell: a floor and a ceiling quad per floor
 * cell and a wall quad on every edge where a floor cell meets void or the
 * grid's edge, the same rule `wallSlots` uses for the walls fixtures stand
 * against. The uv of every shell quad is its world position in metres times
 * its wall pattern's uv scale, so the seams run unbroken across cells of
 * the same pattern. A room with `interior` (the console room, 2.6e C4,
 * and the airlock, M3 C24) draws its shell in `CONSOLE_SHELL`'s fixed
 * tints on pattern 0's layer, whatever the look, with no accent stripe;
 * the console room's flush fittings cover its walls. Every other
 * room's walls, lintels and stripe read their cell's own wall pattern
 * (`wallPatternOf`, `WALL_PATTERN_LOOK`, 2.7 C11): the hall's, a bay's or
 * the corridor's, so a bay or the corridor always reads as a space of its
 * own. Every other room's full wall quads also carry the accent stripe
 * (`ACCENT_STRIPE`, 2.7 C9), a band in the accent mark (`accentTint`), off
 * fixture edges, the entrance edge, lintels and the edges of the wall
 * props and heroes that lie under it (`UNDER_STRIPE`, the saucer poster),
 * and in a hangar off the bay door's span and the gantry legs' edges
 * (`hangarEdges`, M3 C15). A hangar's bay door, pads and gantries
 * (`buildHangar` in `models/hangar.ts`) are built into the same static
 * array after the lamps and the scaffolding.
 * The models themselves are built by the recipes in `models/`, with the
 * modelling kit of `kit.ts`,
 * and always keep `LAYER.panel` whatever the room's wall patterns. Last
 * come the room's decals (`buildDecals` in `models/decals.ts`, 2.7 C20,
 * C21): quads on the decal atlas with `FLAG.decal`, `DECAL_LIFT` off their
 * wall, face or floor, still in the one static array.
 */

import { heroEdges } from "../world/heroes";
import { lampBoxes } from "../world/lamps";
import { STEP, doorwayColumns, isFloor } from "../world/layout";
import { edgeKey, edgeOf } from "../world/sites";
import type {
  Box,
  HeroKind,
  PropKind,
  Rect,
  RoomSpec,
  Side,
} from "../world/types";
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
import { buildDecals } from "./models/decals";
import { buildHangar, hangarEdges } from "./models/hangar";
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
 * - `decal`: a decal (2.7 C20), the first number past the blink groups:
 *   the shader discards its fragment where the texel's alpha is under the
 *   ordered threshold at its pixel, then lights it as `lit` with no edge
 *   lines.
 */
export const FLAG = {
  lit: 0,
  emissive: 1,
  portal: 2,
  frame: 3,
  lamp: 4,
  signal: 5,
  blink: 6,
  decal: 14,
} as const;

/** A blink group's flag: `FLAG.blink + group`, group 0 to 7. */
export type BlinkFlag = 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13;

/** Every flag a surface may carry: one of `FLAG` (the decal's among them), or a blink group's. */
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
 * bays and the corridor, except fixture edges, the entrance edge, lintels
 * and the edges `UNDER_STRIPE` covers. It carries the accent mark, so it
 * takes the room's accent in every look.
 */
export const ACCENT_STRIPE = { h0: 1.2, h1: 1.28 } as const;

/**
 * The wall props and wall heroes whose model lies under the accent stripe:
 * a face that looks into the room between the wall and `DECAL_LIFT`,
 * inside `ACCENT_STRIPE`'s band (the saucer poster's sheet and band). The
 * stripe skips the whole of every edge one of them hangs on, as it skips a
 * fixture's edge, so it never runs across a poster (2.7 C9). The geometry
 * test scans every wall prop and wall hero variant and holds these sets to
 * exactly the kinds that have such a face.
 */
export const UNDER_STRIPE: {
  props: ReadonlySet<PropKind>;
  heroes: ReadonlySet<HeroKind>;
} = {
  props: new Set<PropKind>(["saucer-poster"]),
  heroes: new Set<HeroKind>(),
};

/**
 * The three wall patterns (2.7 C11), by `RoomSpec.finish`'s pattern index:
 * the texture layer the shell's wall, lintel and stripe quads read, and the
 * uv scale (`edgeQuad`'s `scale`) that makes their seams run at the
 * pattern's own spacing rather than every metre.
 *
 * | # | name | layer | seams |
 * |---|---|---|---|
 * | 0 | panels | `LAYER.panel` | every 1 m |
 * | 1 | ribbed | `LAYER.ribbed` | every 0.5 m along, 2 m up |
 * | 2 | plated | `LAYER.plated` | every 2 m along, 1 m up |
 *
 * Every model keeps `LAYER.panel` whatever the room's patterns: this table
 * is read only by the shell.
 */
export const WALL_PATTERN_LOOK: readonly {
  layer: number;
  u: number;
  v: number;
}[] = [
  { layer: LAYER.panel, u: 1, v: 1 },
  { layer: LAYER.ribbed, u: 2, v: 0.5 },
  { layer: LAYER.plated, u: 0.5, v: 1 },
];

/**
 * The wall pattern index (into `WALL_PATTERN_LOOK`) of cell `(x, y)` (2.7
 * C11): the corridor's pattern inside the corridor, a bay's pattern inside
 * that bay, and the hall's pattern everywhere else, including a doorway
 * column's cell, which sits in none of `room.bays` or `room.corridor`.
 */
export function wallPatternOf(room: RoomSpec, x: number, y: number): number {
  const inside = (r: Rect) => x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1;
  if (room.corridor !== null && inside(room.corridor)) {
    return room.finish.corridorWalls;
  }
  const bay = room.bays.findIndex(inside);
  if (bay !== -1) return room.finish.bayWalls[bay] ?? room.finish.hallWalls;
  return room.finish.hallWalls;
}

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

/**
 * How tall the hazard-striped baseboards of a room under construction are,
 * in metres. The decal recipe cuts wall decals out of this band on a room
 * under construction (`buildDecals`), since the baseboard stands the same
 * `DECAL_LIFT` off the wall.
 */
export const BASEBOARD = 0.3;
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
 * the wall and the height, in metres, times `scale` (2.7 C11: a wall
 * pattern's own uv scale, default `{ u: 1, v: 1 }`, world metres
 * unscaled), so neighbouring quads of the same scale continue each other's
 * seams and a pattern's seams run at its own spacing. `scale` never moves a
 * vertex position, only the uv the shader samples the pattern's layer with.
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
  scale: { u: number; v: number } = { u: 1, v: 1 },
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
    (side === "n" || side === "s" ? p[0] : p[1]) * scale.u;
  const v0 = h0 * scale.v;
  const v1 = h1 * scale.v;
  b.vertex([l[0], h0, l[1]], n, u(l), v0, s);
  b.vertex([r[0], h0, r[1]], n, u(r), v0, s);
  b.vertex([r[0], h1, r[1]], n, u(r), v1, s);
  b.vertex([l[0], h0, l[1]], n, u(l), v0, s);
  b.vertex([r[0], h1, r[1]], n, u(r), v1, s);
  b.vertex([l[0], h1, l[1]], n, u(l), v1, s);
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
 * every fixture and piece of furniture as its detailed model, and last
 * the room's decals (`buildDecals`, 2.7 C21). Text
 * quads take their layer and row from `layerPlan(room)`. Pure and
 * deterministic: the same room and look give the same floats.
 *
 * Shell rules: every floor cell gets a floor and a ceiling quad; every edge
 * of a floor cell whose neighbour is void or outside the grid gets a wall
 * quad from the floor to the ceiling, facing into the cell, so no wall ever
 * stands between two floor cells. Where a doorway cell (see
 * `doorwayColumns`) meets the hall, a bay or the corridor, a lintel runs
 * from `LINTEL` to the ceiling on both faces of the edge. A full wall quad
 * on an edge that is neither a fixture's slot nor the entrance's, nor
 * covered by a wall prop under the stripe or by a hangar's bay door or
 * gantry leg, carries the accent stripe, unless the room has fittings
 * (`interior`). Every wall,
 * lintel and stripe quad reads its cell's wall pattern
 * (`wallPatternOf`, `WALL_PATTERN_LOOK`, 2.7 C11): the hall's, a bay's or
 * the corridor's, whichever rectangle the cell falls in, pattern 0 always
 * on a room with fittings. Lamps and scaffolding share the hall's ceiling
 * height, which bays and the corridor share too. Every light zone hangs a
 * lamp panel, except in the airlock (M3 C24), which is lit by the iris
 * light in its ceiling instead. A hangar (`room.hangar`) adds its bay door,
 * pads and gantries (`buildHangar`, M3 C15).
 */
export function buildRoomMesh(room: RoomSpec, look: Look): RoomMesh {
  const b = createBuilder();
  const p = look.palette;
  const H = room.ceiling;
  const doorways = doorwayColumns(room);
  // A room with fittings (the console room, the airlock) keeps its own
  // colours and pattern 0 in every look (2.6e C4); every other room takes the look's
  // colours and each cell's own wall pattern (2.7 C11).
  const fitted = room.interior !== undefined;
  const wallTint = fitted ? CONSOLE_SHELL.wall : p.panel;
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
  const fixtureEdges = new Set(
    room.fixtures.map((f) => `${f.slot.x},${f.slot.y},${f.slot.side}`),
  );
  const entranceEdge = `${room.entrance.x},${room.entrance.y},s`;
  // The edges a wall prop or wall hero lying under the stripe hangs on
  // (`UNDER_STRIPE`): the stripe skips them whole.
  // So do the bay door's span of the north wall and the gantry legs' edges
  // in a hangar (M3 C15): the door and the legs stand over the stripe's
  // band.
  const coveredEdges = new Set([
    ...hangarEdges(room),
    ...room.props
      .filter((p) => UNDER_STRIPE.props.has(p.kind))
      .map((p) => edgeKey(edgeOf(p))),
    ...room.heroes
      .filter((h) => UNDER_STRIPE.heroes.has(h.kind))
      .flatMap(heroEdges)
      .map(edgeKey),
  ]);
  // Wall slots that are ways through (a lift and an exit among them, M3
  // C28) keep their wall clear of baseboards.
  const openings = new Set(
    room.fixtures
      .filter(
        (f) =>
          f.kind === "door" ||
          f.kind === "portal" ||
          f.kind === "hatch" ||
          f.kind === "lift" ||
          f.kind === "exit",
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
      // The cell's own wall pattern (2.7 C11): pattern 0 (the bevelled
      // panels) on a room with fittings, whichever the hall, its bay or the
      // corridor picked otherwise.
      const patternLook =
        WALL_PATTERN_LOOK[fitted ? 0 : wallPatternOf(room, x, y)]!;
      const scale = { u: patternLook.u, v: patternLook.v };
      const wall: Surface = {
        layer: patternLook.layer,
        tint: wallTint,
        flag: FLAG.lit,
      };
      // The accent stripe (2.7 C9) runs on the cell's own wall layer in
      // the room's accent, and skips every fixture's edge, the entrance's
      // and every edge under a poster (`coveredEdges`). A room with
      // fittings draws none.
      const stripe: Surface = {
        layer: patternLook.layer,
        tint: accentTint(1),
        flag: FLAG.lit,
      };
      for (const side of SIDES) {
        const [dx, dy] = STEP[side];
        const nx = x + dx;
        const ny = y + dy;
        if (!isFloor(room.grid, nx, ny)) {
          edgeQuad(b, x, y, side, 0, H, 0, wall, scale);
          const key = `${x},${y},${side}`;
          if (
            !fitted &&
            !fixtureEdges.has(key) &&
            !coveredEdges.has(key) &&
            key !== entranceEdge
          )
            edgeQuad(
              b,
              x,
              y,
              side,
              ACCENT_STRIPE.h0,
              ACCENT_STRIPE.h1,
              DECAL_LIFT,
              stripe,
              scale,
            );
          if (building && !openings.has(`${x},${y},${side}`))
            edgeQuad(b, x, y, side, 0, BASEBOARD, BASEBOARD_INSET, hazard);
        } else if (doorways.has(x) !== doorways.has(nx) && H > LINTEL) {
          edgeQuad(b, x, y, side, LINTEL, H, 0, wall, scale);
        }
      }
    }
  }

  // One lamp panel per light zone, just under the ceiling; none in the
  // airlock, whose light is its iris light (M3 C24).
  for (const l of room.space === "airlock" ? [] : lampBoxes(room))
    flatQuad(b, l.x0, l.x1, l.z0, l.z1, H - LAMP_DROP, false, {
      layer: LAYER.ceiling,
      tint: p.lamp,
      flag: FLAG.lamp,
    });

  // Under construction: the scaffold frames the player walks around.
  const pole: Surface = { layer: LAYER.metal, tint: p.door, flag: FLAG.lit };
  for (const box of room.scaffold) scaffold(b, box, H, pole);

  // A hangar's bay door, pads and gantries (M3 C15).
  buildHangar(b, room, look);

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
  buildDecals(kitAt, b, room);
  return { static: b.build(), movers };
}
