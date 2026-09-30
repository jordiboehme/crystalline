/**
 * The hangar's structure (M3 C15, C19): the bay door across the north
 * wall's span, the two landing pads on the floor and the two gantries
 * overhead, drawn from the room's `hangar` data straight into the room's
 * static mesh. It is structure, not props: no instancing, no collision
 * (the frame and the legs stand on the wall line, the pads are paint on a
 * plate 0.02 m proud, the beams are overhead), and nothing here moves.
 *
 * - **The bay door** fills the span `bayDoor.x0..x1` from the floor to
 *   `bayDoor.h`: six horizontal steel leaves with a dark groove between
 *   each two, set `BAY_LEAVES.recess` behind the face of a frame
 *   `BAY_FRAME.depth` deep. The frame's jambs stand inside the span, so
 *   the door never reaches the deck's screen on the edge beside it nor
 *   the door slot past it; its lintel runs `BAY_FRAME.head` over the
 *   opening. A hazard-striped band runs round the frame's inner edge, a
 *   lit strip in the room's accent along the lintel and an amber warning
 *   lamp at each of the frame's top corners; both glow steady, so the
 *   door reads from the lift at the far end of the hall (C19).
 * - **A pad** is a plate `PAD_PLATE` proud over its cells under a flat
 *   dark coat (`PAD_COAT` of the look's floor colour, painted with no
 *   edge lines, so the pad stands apart from a gridded floor), with a
 *   hazard-striped rim `PAD_PAINT.rim` wide, a white ring `PAD_PAINT.ring`
 *   wide of radius `PAD_PAINT.radius` and a white cross of two bars round
 *   its centre, and a small lamp housing in each corner whose lens glows.
 *   A floor decal on a pad (the pad's stencil) lies on the coat
 *   (`floorTop`, `PAD_TOP`), not under it.
 * - **A gantry** is a box truss `2 * GANTRY_BEAM.half` square along its
 *   row's centre line from the west wall to the east, its underside at
 *   the gantry's `h`: four chords, a post and a diagonal on each side
 *   every `TRUSS_BAY`, and a cross member under each post. A legged frame
 *   stands at each wall end inside its leg box (`gantryLegs`): two posts
 *   `LEG.post` square from the floor to the truss, cross-braced in the
 *   wall's plane. A catwalk `GANTRY_BEAM.catwalk` wide runs along the
 *   truss's south side at its underside's height, with a rail on each
 *   side. Truss, catwalk and rails stay inside the beam's plan box
 *   (`gantryBeams`), where no ceiling span hangs. Everything of a gantry
 *   but its top rails (in the room's accent) is dark steel
 *   (`gantrySteel`), so it stands out against the pale walls and ceiling
 *   in every look; the legs' frames fill their leg boxes (`GANTRY_LEG`).
 *
 * Everything is built in one world-aligned kit frame (`along` +x, `inward`
 * +z) plus the legs' wall frames, so every face winds as the kit winds
 * it.
 */

import {
  GANTRY_BEAM,
  GANTRY_LEG,
  gantryLegEdges,
  padBox,
} from "../../world/hangarShape";
import { edgeKey } from "../../world/sites";
import type { RoomSpec, WallSlot } from "../../world/types";
import { CELL } from "../../world/units";
import {
  FLAG,
  accentTint,
  type Builder,
  type Surface,
  type V3,
} from "../geometry";
import { DECAL_LIFT, createKit, frameAt, frameForSlot, type Kit } from "../kit";
import { LAYER } from "../layers";
import { DECAL_TILES, tileRect } from "../textures";
import type { Look, Rgb } from "../looks";
import {
  LAMP_TINT,
  RECESS,
  discOutline,
  shade,
  surfaces,
  tiltedBar,
} from "./common";

/** How far a pad's plate stands proud of the floor, in metres. */
export const PAD_PLATE = 0.02;

/**
 * The top of a pad's painted plate, in metres: the dark coat lies
 * `DECAL_LIFT` over the plate's box, and the marks on it (the rim, the
 * ring, the cross and the pad's stencil) lie `DECAL_LIFT` above this.
 */
export const PAD_TOP = PAD_PLATE + DECAL_LIFT;

/**
 * How dark a pad's coat is against the look's floor colour. The coat is
 * lit as a decal (no edge lines), so the plate reads as one flat dark
 * square on a gridded floor in every look.
 */
export const PAD_COAT = 0.55;

/**
 * How dark the gantries' steel is against the look's bare metal: dark
 * enough that the truss and its legs stand out against the pale walls and
 * ceiling, the flat palette's among them.
 */
export const GANTRY_SHADE = 0.4;

/** The gantries' steel in `look` (`GANTRY_SHADE` of its bare metal). */
export function gantrySteel(look: Look): Surface {
  return {
    layer: LAYER.metal,
    tint: shade(look.palette.metal, GANTRY_SHADE),
    flag: FLAG.lit,
  };
}

/**
 * The bay door's frame, in metres: each jamb `jamb` wide inside the span,
 * `depth` out from the wall (no more than `FLUSH_DEPTH`, so the player
 * walking along the wall never reaches into it) and the lintel `head`
 * tall over the opening.
 */
export const BAY_FRAME = { jamb: 0.4, depth: 0.28, head: 0.6 } as const;

/**
 * The bay door's leaves: `count` of them stacked from the floor to the
 * opening's top with a `groove` between each two, their faces `recess`
 * behind the frame's face.
 */
export const BAY_LEAVES = { count: 6, groove: 0.05, recess: 0.15 } as const;

/**
 * The paint on a pad, in metres: the hazard rim's width, the white ring's
 * width and radius (to the ring's middle), and the cross's two bars.
 */
export const PAD_PAINT = {
  rim: 0.3,
  ring: 0.25,
  radius: 5,
  bar: { length: 6, width: 0.4 },
} as const;

/** How far apart the truss's posts and diagonals stand, in metres. */
export const TRUSS_BAY = 2;

/** A leg's posts' side and the bracing bars' width, in metres. */
const LEG = { post: 0.25, brace: 0.06, stage: 1.5 } as const;

/** The truss's chord and web members' side, in metres. */
const CHORD = 0.08;
const WEB = 0.05;

/** The catwalk's deck thickness, rail height and rail bars' side. */
const CATWALK = { deck: 0.05, rail: 1.0, mid: 0.5, bar: 0.05 } as const;

/** The pad lamps' housing side and height, and their lens's colour. */
const PAD_LAMP = { side: 0.3, h: 0.12, lens: 0.06 } as const;
const PAD_LENS: Rgb = [0.8, 0.95, 1.0];

/** The white paint's colour, the same in every look. */
const PAINT_WHITE: Rgb = [1, 1, 0.97];

/** How many pieces the pad's ring is cut into. */
const RING_SEGMENTS = 48;

/**
 * Where the floor's top is at world `(x, z)` for what lies on it: the
 * painted plate's `PAD_TOP` on a pad, 0 anywhere else. A floor decal lies
 * `DECAL_LIFT` above this (`buildDecals`).
 */
export function floorTop(room: RoomSpec, x: number, z: number): number {
  const pads = room.hangar?.pads ?? [];
  const on = pads.some((p) => {
    const b = padBox(p);
    return x >= b.x0 && x <= b.x1 && z >= b.z0 && z <= b.z1;
  });
  return on ? PAD_TOP : 0;
}

/**
 * The wall edges a hangar's structure covers over the accent stripe's
 * band (M3 C15), as edge keys: every north wall edge of the bay door's
 * span and every gantry leg's edge. The room mesh runs no stripe on them.
 * None for a room with no `hangar`.
 */
export function hangarEdges(
  room: Pick<RoomSpec, "width" | "hangar">,
): string[] {
  const hangar = room.hangar;
  if (hangar === undefined) return [];
  const span = Array.from(
    { length: hangar.bayDoor.x1 - hangar.bayDoor.x0 },
    (_, i): WallSlot => ({ x: hangar.bayDoor.x0 + i, y: 0, side: "n" }),
  );
  return [...span, ...gantryLegEdges(room)].map(edgeKey);
}

/**
 * One flat quad facing up at height `y` over `x0..x1` by `z0..z1`, its uv
 * the world x and z in metres (or one point `uv` for a plain colour),
 * wound counter-clockwise seen from above.
 */
function paint(
  b: Builder,
  x0: number,
  x1: number,
  z0: number,
  z1: number,
  y: number,
  s: Surface,
  uv?: readonly [number, number],
): void {
  const up: V3 = [0, 1, 0];
  const corners: [number, number][] = [
    [x0, z1],
    [x1, z1],
    [x1, z0],
    [x0, z0],
  ];
  for (const i of [0, 1, 2, 0, 2, 3]) {
    const c = corners[i];
    if (c) b.vertex([c[0], y, c[1]], up, uv?.[0] ?? c[0], uv?.[1] ?? c[1], s);
  }
}

/**
 * The hangar's bay door, pads and gantries into `b` (see the module doc).
 * Nothing for a room with no `hangar`.
 */
export function buildHangar(b: Builder, room: RoomSpec, look: Look): void {
  const hangar = room.hangar;
  if (hangar === undefined) return;
  // The world frame: a = x, d = z, h = y.
  const k = createKit(b, frameAt([0, 0, 0], 2));
  const s = surfaces(look);
  bayDoor(k, hangar.bayDoor, s);
  for (const p of hangar.pads) pad(b, k, padBox(p), s, look);
  const steel = gantrySteel(look);
  for (const g of hangar.gantries)
    truss(k, room.width * CELL, g, steel, s.accent(0.9));
  gantryLegEdges(room).forEach((edge) => {
    const g = hangar.gantries.find((x) => x.y === edge.y);
    if (g !== undefined) leg(createKit(b, frameForSlot(edge)), g.h, steel);
  });
}

type Surfaces = ReturnType<typeof surfaces>;

/** The bay door: backing, leaves, frame, hazard band, strip and lamps. */
function bayDoor(
  k: Kit,
  bay: { x0: number; x1: number; h: number },
  s: Surfaces,
): void {
  const x0 = bay.x0 * CELL;
  const x1 = bay.x1 * CELL;
  const top = bay.h;
  const f = BAY_FRAME;
  const i0 = x0 + f.jamb;
  const i1 = x1 - f.jamb;
  // The dark behind the grooves.
  k.panel(i0, i1, DECAL_LIFT, 0, top, {
    layer: LAYER.panel,
    tint: RECESS,
    flag: FLAG.lit,
  });
  // The leaves, their faces `recess` behind the frame's face, tucked into
  // the jambs at their ends.
  const face = f.depth - BAY_LEAVES.recess;
  const n = BAY_LEAVES.count;
  const leafH = (top - (n - 1) * BAY_LEAVES.groove) / n;
  for (let i = 0; i < n; i++) {
    const h0 = i * (leafH + BAY_LEAVES.groove);
    k.box(i0 - 0.02, i1 + 0.02, face - 0.08, face, h0, h0 + leafH, s.metal);
    // A rib along each leaf, so the leaves read as heavy plates.
    k.box(
      i0,
      i1,
      face,
      face + 0.02,
      h0 + leafH / 2 - 0.05,
      h0 + leafH / 2 + 0.05,
      s.dark,
    );
  }
  // The frame: two jambs and the lintel.
  const frame = s.body;
  k.bevelBox(x0, i0, 0, f.depth, 0, top + f.head, 0.02, frame);
  k.bevelBox(i1, x1, 0, f.depth, 0, top + f.head, 0.02, frame);
  k.bevelBox(i0, i1, 0, f.depth, top, top + f.head, 0.02, frame);
  // The hazard band round the frame's inner edge, on its face.
  const band = 0.2;
  const d = f.depth;
  k.box(i0 - band, i0, d, d + DECAL_LIFT, 0, top, s.hazard);
  k.box(i1, i1 + band, d, d + DECAL_LIFT, 0, top, s.hazard);
  k.box(i0 - band, i1 + band, d, d + DECAL_LIFT, top, top + band, s.hazard);
  // The lit strip along the lintel, in the room's accent, steady.
  const strip: Surface = {
    layer: LAYER.panel,
    tint: accentTint(1),
    flag: FLAG.emissive,
  };
  k.box(i0 + 0.3, i1 - 0.3, d, d + 0.02, top + 0.32, top + 0.42, strip);
  // The amber warning lamps at the frame's top corners, steady.
  const lamp = s.signal(LAMP_TINT);
  for (const a of [x0 + f.jamb / 2, x1 - f.jamb / 2]) {
    const h = top + f.head / 2 + 0.05;
    k.box(a - 0.14, a + 0.14, d, d + 0.03, h - 0.14, h + 0.14, s.dark);
    k.extrude(discOutline(a, h, 0.1, 12), d + 0.03, d + 0.06, lamp);
  }
}

/** A pad: plate, hazard rim, ring, cross and four corner lamps. */
function pad(
  b: Builder,
  k: Kit,
  box: { x0: number; x1: number; z0: number; z1: number },
  s: Surfaces,
  look: Look,
): void {
  const { x0, x1, z0, z1 } = box;
  const plate: Surface = {
    layer: LAYER.plated,
    tint: shade(look.palette.floor, PAD_COAT),
    flag: FLAG.lit,
  };
  k.box(x0, x1, z0, z1, 0, PAD_PLATE, plate);
  // The paint samples one point of the decal atlas's solid tile, as a
  // stencil's pixels do, and is lit as a decal: flat paint with no edge
  // lines across it. First the dark coat over the whole plate.
  const solid = tileRect(DECAL_TILES.solid[0] ?? 15);
  const dot = [(solid.u0 + solid.u1) / 2, (solid.v0 + solid.v1) / 2] as const;
  const coat: Surface = {
    layer: LAYER.decal,
    tint: shade(look.palette.floor, PAD_COAT),
    flag: FLAG.decal,
  };
  paint(b, x0, x1, z0, z1, PAD_TOP, coat, dot);
  const y = PAD_TOP + DECAL_LIFT;
  const r = PAD_PAINT.rim;
  // The rim: north and south bars full width, west and east between them.
  paint(b, x0, x1, z0, z0 + r, y, s.hazard);
  paint(b, x0, x1, z1 - r, z1, y, s.hazard);
  paint(b, x0, x0 + r, z0 + r, z1 - r, y, s.hazard);
  paint(b, x1 - r, x1, z0 + r, z1 - r, y, s.hazard);
  // Then the white ring and cross on it.
  const white: Surface = {
    layer: LAYER.decal,
    tint: PAINT_WHITE,
    flag: FLAG.decal,
  };
  const cx = (x0 + x1) / 2;
  const cz = (z0 + z1) / 2;
  // The ring, cut into flat pieces round the centre.
  const inner = PAD_PAINT.radius - PAD_PAINT.ring / 2;
  const outer = PAD_PAINT.radius + PAD_PAINT.ring / 2;
  const up: V3 = [0, 1, 0];
  for (let i = 0; i < RING_SEGMENTS; i++) {
    const t0 = (2 * Math.PI * i) / RING_SEGMENTS;
    const t1 = (2 * Math.PI * (i + 1)) / RING_SEGMENTS;
    const at = (rr: number, t: number): V3 => [
      cx + rr * Math.cos(t),
      y,
      cz - rr * Math.sin(t),
    ];
    // Counter-clockwise seen from above: angle grows towards -z.
    const q = [at(inner, t0), at(outer, t0), at(outer, t1), at(inner, t1)];
    for (const j of [0, 1, 2, 0, 2, 3]) {
      const p = q[j];
      if (p) b.vertex(p, up, dot[0], dot[1], white);
    }
  }
  // The cross.
  const half = PAD_PAINT.bar.length / 2;
  const w = PAD_PAINT.bar.width / 2;
  paint(b, cx - half, cx + half, cz - w, cz + w, y, white, dot);
  paint(b, cx - w, cx + w, cz - half, cz - w, y, white, dot);
  paint(b, cx - w, cx + w, cz + w, cz + half, y, white, dot);
  // A lamp housing in each corner, on the rim, its lens glowing.
  const lens = s.signal(PAD_LENS);
  const L = PAD_LAMP;
  for (const [ax, az] of [
    [x0, z0],
    [x1 - L.side, z0],
    [x0, z1 - L.side],
    [x1 - L.side, z1 - L.side],
  ] as const) {
    k.bevelBox(
      ax,
      ax + L.side,
      az,
      az + L.side,
      PAD_PLATE,
      PAD_PLATE + L.h,
      0.02,
      s.dark,
    );
    k.box(
      ax + L.lens,
      ax + L.side - L.lens,
      az + L.lens,
      az + L.side - L.lens,
      PAD_PLATE + L.h,
      PAD_PLATE + L.h + 0.03,
      lens,
    );
  }
}

/** A gantry's truss and its catwalk, wall to wall. */
function truss(
  k: Kit,
  width: number,
  g: { y: number; h: number },
  steel: Surface,
  rail: Surface,
): void {
  const zc = (g.y + 0.5) * CELL;
  const half = GANTRY_BEAM.half;
  const zn = zc - half;
  const zs = zc + half;
  const h0 = g.h;
  const h1 = g.h + 2 * half;
  // Four chords.
  for (const [za, zb] of [
    [zn, zn + CHORD],
    [zs - CHORD, zs],
  ] as const)
    for (const [ha, hb] of [
      [h0, h0 + CHORD],
      [h1 - CHORD, h1],
    ] as const)
      k.box(0, width, za, zb, ha, hb, steel);
  // A post and a diagonal on each side every bay, a cross member under
  // each post.
  const bays = Math.round(width / TRUSS_BAY);
  for (let i = 0; i <= bays; i++) {
    const x = Math.min(width - WEB, Math.max(0, i * TRUSS_BAY - WEB / 2));
    for (const [za, zb] of [
      [zn, zn + WEB],
      [zs - WEB, zs],
    ] as const)
      k.box(x, x + WEB, za, zb, h0 + CHORD, h1 - CHORD, steel);
    k.box(x, x + WEB, zn + CHORD, zs - CHORD, h0, h0 + WEB, steel);
  }
  for (let i = 0; i < bays; i++) {
    const xa = i * TRUSS_BAY + WEB;
    const xb = (i + 1) * TRUSS_BAY - WEB;
    const [ha, hb] = [h0 + CHORD, h1 - CHORD];
    const rising = i % 2 === 0;
    const angle = Math.atan2(rising ? hb - ha : ha - hb, xb - xa);
    const length = Math.hypot(xb - xa, hb - ha);
    const bar = tiltedBar((xa + xb) / 2, (ha + hb) / 2, angle, length, WEB);
    k.extrude(bar, zn + 0.005, zn + WEB - 0.005, steel);
    k.extrude(bar, zs - WEB + 0.005, zs - 0.005, steel);
  }
  // The catwalk along the truss's south side: deck, posts and rails.
  const c0 = zs;
  const c1 = zs + GANTRY_BEAM.catwalk;
  const C = CATWALK;
  k.box(0, width, c0, c1, h0, h0 + C.deck, steel);
  for (const [za, zb] of [
    [c0, c0 + C.bar],
    [c1 - C.bar, c1],
  ] as const) {
    for (let i = 0; i <= bays; i++) {
      const x = Math.min(width - C.bar, Math.max(0, i * TRUSS_BAY - C.bar / 2));
      k.box(x, x + C.bar, za, zb, h0 + C.deck, h0 + C.rail, steel);
    }
    k.box(0, width, za, zb, h0 + C.rail - C.bar, h0 + C.rail, rail);
    k.box(0, width, za, zb, h0 + C.mid, h0 + C.mid + C.bar, steel);
  }
}

/**
 * A gantry's legged frame in its wall's frame (`frameForSlot` of its leg
 * edge, `a` along the wall from the row's centre, `d` out from it): two
 * posts from the floor to the truss, cross-braced in stages in the wall's
 * plane, and a cap under the truss.
 */
function leg(k: Kit, h: number, steel: Surface): void {
  const out0 = 0.05;
  const out1 = out0 + LEG.post;
  // The frame fills its leg box (`GANTRY_LEG`) along the wall and stands
  // inside it out from the wall.
  const edge = GANTRY_LEG.along / 2;
  const inner = edge - LEG.post;
  k.box(-edge, -inner, out0, out1, 0, h, steel);
  k.box(inner, edge, out0, out1, 0, h, steel);
  // The cap the truss rests on.
  k.box(-edge, edge, out0, out1, h - 0.12, h, steel);
  // Cross-bracing in stages between the posts.
  const stages = Math.max(1, Math.floor((h - 0.3) / LEG.stage));
  const step = (h - 0.3) / stages;
  const dMid = (out0 + out1) / 2;
  for (let i = 0; i < stages; i++) {
    const ha = 0.15 + i * step;
    const hb = ha + step;
    const angle = Math.atan2(hb - ha, 2 * inner);
    const length = Math.hypot(2 * inner, hb - ha);
    // The two bars of an X at depths of their own, so they never share
    // a plane where they cross.
    for (const sign of [1, -1]) {
      const bar = tiltedBar(0, (ha + hb) / 2, sign * angle, length, LEG.brace);
      k.extrude(
        bar,
        dMid + (sign > 0 ? -0.04 : 0),
        dMid + (sign > 0 ? 0 : 0.04),
        steel,
      );
    }
  }
}
