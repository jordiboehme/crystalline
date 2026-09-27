/**
 * Dev-only spawn points: places the player in front of a gallery fixture,
 * hero, prop or curio, so browser shots start every malfunction, or
 * every model, already facing it.
 *
 * Four patterns. `<kind>:<n>` (`SPOT_KINDS` pins the fixture kinds it
 * accepts to `Fixture["kind"]`, so a new kind is a type error here until it
 * is added) is `wallFacingSpawn`'s cell-centre spot, backed off clear of
 * the fixture's own footprint when it has one (`spotFor`). `prop:<kind>:<n>`
 * (H16) frames the n-th hero or prop of that kind instead: a spot in front
 * of its face, at a distance that frames it, from `frameSpot`, pitched
 * toward a lifted or low hero's middle (`framesPitched`, C16). The same
 * pattern frames the n-th curio of that kind close and tilted down
 * instead (C18, 2.6b), when `kind` is neither a hero nor a prop kind (the
 * three families' names are disjoint, pinned in `world/curios.ts`).
 * `decor:<kind>:<n>` (2.7 Task 1) frames the n-th decor piece of that kind
 * (in `room.decor` order) instead, from its footprint (`decorFootprint`),
 * the same `frameSpot` a prop is framed with; null for a pipe run, whose
 * footprint is null (it hangs from the ceiling).
 *
 * The prop pattern takes an optional view suffix (`SpotView`, 2.6d C20),
 * for judging a model from more than its front: `:back` from behind,
 * `:side` from its right side, `:quarter` from halfway between its front
 * and its right, and `:close` a hero or prop from nearer (`CLOSE_BASE`,
 * `CLOSE_SCALE`) and a curio from the nearest side that sees it
 * (`CURIO_CLOSE_NEAR`). All three patterns give
 * null for a bad spot: an unknown kind or view, no such ordinal, no
 * ordinal at all, or a negative one.
 *
 * `spotView` is the seam that carries a curio's or hero's pitch along with
 * its spawn; `spotSpawn` is `spotView` with the pitch dropped, for every
 * caller that only ever wants the spawn. `heroSightClear` (C16) checks a
 * hero's own sight line the same way `curioSightClear` checks a curio's.
 */

import { TERMINAL_OCCLUDERS } from "../render/models/terminal";
import {
  curioBox,
  curioLift,
  curioMid,
  curioSize,
  hostSurfaces,
  type HostSurface,
} from "../world/curios";
import {
  HERO_FRONT,
  decorFootprint,
  footprint,
  footprintOf,
  heroFootprint,
  heroLift,
  heroTurn,
  propFootprint,
  turnedBox,
} from "../world/footprints";
import { HERO_CATALOGUE, HERO_KINDS } from "../world/heroes";
import { wallFacingSpawn, wallPoint } from "../world/interact";
import { isFloor } from "../world/layout";
import {
  blockersFor,
  EYE_HEIGHT,
  MAX_PITCH,
  PLAYER_RADIUS,
} from "../world/move";
import { PROP_KINDS } from "../world/props";
import { edgeOf, wallAnchor } from "../world/sites";
import type {
  Box,
  Curio,
  Fixture,
  Hero,
  HeroKind,
  RoomSpec,
} from "../world/types";
import { CELL } from "../world/units";

/**
 * The fixture kinds `spotSpawn` accepts, one entry per kind of `Fixture`:
 * the `satisfies` makes a new fixture kind a type error here until it is
 * added, so the dev seam can never silently miss one.
 */
export const SPOT_KINDS = {
  terminal: true,
  door: true,
  portal: true,
  hatch: true,
  machine: true,
  poster: true,
  placard: true,
} as const satisfies Record<Fixture["kind"], true>;

const FIXTURE_SPOT = new RegExp(
  `^(${Object.keys(SPOT_KINDS).join("|")}):(\\d+)$`,
);

/**
 * How clear of its own footprint a backed-off spot keeps the player's
 * circle, in metres, past `PLAYER_RADIUS`.
 */
const SPOT_CLEARANCE = 0.2;

/**
 * `wallFacingSpawn`'s cell-centre spot for `fixture`, backed off along the
 * slot's inward direction when the fixture has a `footprintOf` box (a
 * terminal or a machine): the cell-centre depth is `CELL / 2`, so it is
 * raised to the box's own reach from the wall plus `PLAYER_RADIUS` plus
 * `SPOT_CLEARANCE` whenever that is deeper, never backed off less than the
 * cell centre. The yaw stays the wall-facing one. A door, a portal, a
 * hatch, a poster and the placard take no floor (`footprintOf` gives null
 * for them), so they keep the plain cell-centre spot.
 */
function spotFor(fixture: Fixture): RoomSpec["spawn"] {
  const base = wallFacingSpawn(fixture.slot);
  const box = footprintOf(fixture);
  if (box === null) return base;
  const w = wallPoint(fixture.slot);
  const out = w.inward[0] !== 0 ? box.x1 - box.x0 : box.z1 - box.z0;
  const extra = Math.max(0, out + PLAYER_RADIUS + SPOT_CLEARANCE - CELL / 2);
  return {
    x: base.x + (extra / CELL) * w.inward[0],
    y: base.y + (extra / CELL) * w.inward[1],
    yaw: base.yaw,
  };
}

/**
 * How a `?at=` spot looks at its thing: from the front (the default),
 * behind, the right side, halfway between the front and the right, or
 * close (2.6d C20).
 */
export type SpotView = "front" | "back" | "side" | "quarter" | "close";

const PROP_SPOT = /^prop:([a-z-]+):(\d+)(?::(back|side|quarter|close))?$/;

/** `decor:<kind>:<n>` (2.7 Task 1): the n-th decor piece of that kind. */
const DECOR_SPOT = /^decor:([a-z-]+):(\d+)$/;

/** How far a close frame starts from a hero's or prop's face, in metres, before its size is added (2.6d C20). */
export const CLOSE_BASE = 0.6;
/** How much of the longer side a close frame adds. */
export const CLOSE_SCALE = 0.25;

/**
 * True when a circle of the player's radius at (x, z) overlaps the box: the
 * same clamp-and-distance check `move.ts` collides the player with. Moved
 * here from `spots.test.ts` (H16), which now imports it back, since
 * `frameSpot` needs it too and a dev-only helper keeps one copy.
 */
export function circleOverlapsBox(x: number, z: number, b: Box): boolean {
  const nx = Math.max(b.x0, Math.min(x, b.x1));
  const nz = Math.max(b.z0, Math.min(z, b.z1));
  const dx = x - nx;
  const dz = z - nz;
  return dx * dx + dz * dz < PLAYER_RADIUS * PLAYER_RADIUS;
}

/** The box `frameSpot` frames a wall or ceiling prop with, which has no `propFootprint`. */
const EDGE_PROP_SIZE = { along: 1.8, out: 0.3 };

/** Metres from a framed thing's face at the least, before its size is added. */
const FRAME_BASE = 1.5;
/** How much of a framed thing's longer side is added to the distance. */
const FRAME_SCALE = 0.75;

/** How close `frameSpot`'s stepping is allowed to bring the player, in metres. */
const FRAME_FLOOR = PLAYER_RADIUS + SPOT_CLEARANCE;

/**
 * True when a player's circle centred at `(px, pz)` stands entirely on
 * floor cells: the same four-corner `isFloor` check `frameSpot` and
 * `frameCurio` both frame a spot with.
 */
function onFloorAt(room: RoomSpec): (px: number, pz: number) => boolean {
  return (px, pz) =>
    [px - PLAYER_RADIUS, px + PLAYER_RADIUS].every((x) =>
      [pz - PLAYER_RADIUS, pz + PLAYER_RADIUS].every((z) =>
        isFloor(room.grid, Math.floor(x / CELL), Math.floor(z / CELL)),
      ),
    );
}

/**
 * The four directions a framing spot is tried from, front first: `front`
 * itself, then right, left and back (a quarter turn each), as `[x, z]`
 * unit vectors. `frameSpot` and `frameCurio` (C18) both search these in
 * order (`frameCurio` reorders them by its view).
 */
function sidesOf(
  front: readonly [number, number],
): readonly (readonly [number, number])[] {
  return [
    front,
    [front[1] * -1, front[0]],
    [front[1], -front[0]],
    [-front[0], -front[1]],
  ];
}

/**
 * The direction a view starts from, as `[x, z]`: `front` turned to the
 * view (the side is `sidesOf`'s right, the quarter halfway between the
 * front and that right, normalised; `close` looks from the front).
 */
function viewDir(
  front: readonly [number, number],
  view: SpotView,
): readonly [number, number] {
  const right: readonly [number, number] = [-front[1], front[0]];
  switch (view) {
    case "back":
      return [-front[0], -front[1]];
    case "side":
      return right;
    case "quarter": {
      const x = front[0] + right[0];
      const z = front[1] + right[1];
      const n = Math.hypot(x, z);
      return [x / n, z / n];
    }
    default:
      return front;
  }
}

/**
 * A spot that frames `box` from `dir` (H16, 2.6d C20): the player stands
 * out along `dir` from the box's centre, facing it, past the box's half
 * extent along `dir` (`|dir.x| * halfX + |dir.z| * halfZ`, exactly the
 * half width or half depth on an axis direction) by `FRAME_BASE +
 * FRAME_SCALE * longer side`, or by `CLOSE_BASE + CLOSE_SCALE * longer
 * side` when `close`, stepping 0.25 m closer while the circle is off the
 * floor or on a blocker, down to `PLAYER_RADIUS + SPOT_CLEARANCE`; if no
 * distance works, the other three quarter turns of `dir` are tried
 * (`sidesOf`: right, left, back). Null when none works.
 */
function frameSpot(
  room: RoomSpec,
  box: Box,
  dir0: readonly [number, number],
  close = false,
): RoomSpec["spawn"] | null {
  const blockers = blockersFor(room);
  const onFloor = onFloorAt(room);
  const cx = (box.x0 + box.x1) / 2;
  const cz = (box.z0 + box.z1) / 2;
  const halfX = (box.x1 - box.x0) / 2;
  const halfZ = (box.z1 - box.z0) / 2;
  const longer = Math.max(box.x1 - box.x0, box.z1 - box.z0);
  for (const dir of sidesOf(dir0)) {
    const half = Math.abs(dir[0]) * halfX + Math.abs(dir[1]) * halfZ;
    let dist = close
      ? CLOSE_BASE + CLOSE_SCALE * longer
      : FRAME_BASE + FRAME_SCALE * longer;
    for (;;) {
      const px = cx + dir[0] * (half + dist);
      const pz = cz + dir[1] * (half + dist);
      if (
        onFloor(px, pz) &&
        !blockers.some((b) => circleOverlapsBox(px, pz, b))
      ) {
        return {
          x: px / CELL - 0.5,
          y: pz / CELL - 0.5,
          yaw: Math.atan2(dir[0], dir[1]),
        };
      }
      if (dist <= FRAME_FLOOR) break;
      dist = Math.max(FRAME_FLOOR, dist - 0.25);
    }
  }
  return null;
}

/** The closest a curio-framing spot starts from its centre, in metres (C18). */
export const CURIO_NEAR = 0.8;
/** The farthest a curio-framing spot searches to, in metres (C18). */
export const CURIO_FAR = 3.0;
/** The curio-framing search's step, in metres (C18). */
export const CURIO_STEP = 0.1;

/**
 * The closest a `:close` curio frame starts from the curio's centre, in
 * metres (2.6d): nearer than `CURIO_NEAR`, for judging a small desk curio's
 * marks, and searched round all four sides for the nearest spot.
 */
export const CURIO_CLOSE_NEAR = 0.4;

/** An axis-aligned 3D box: `Box` (world x/z) plus a floor-to-ceiling height range. */
interface Volume extends Box {
  y0: number;
  y1: number;
}

/**
 * The volume of `c`'s own host that occludes it (the host rule): the host's
 * footprint `fp` from `c`'s own surface's ceiling (`s.h + s.clear`, the free
 * height the surface promises a curio standing on it) up to `EYE_HEIGHT`, or
 * null when that ceiling is already at or over the eye. The same rule for
 * every host, fixture, decor, prop or hero: under a hero's or a workbench's
 * top it is the slab above the curio, over a shelf's lower level it is the
 * level above it, and over an open top (`OPEN_CLEAR`, 1.3 m past a top well
 * under a metre) it is nothing, since no sight line from the eye reaches
 * that high. The null case matters: `segmentHitsBox` sorts each axis's two
 * plane hits, so an inverted volume would count as a real box over the eye
 * rather than as none.
 */
function ceilingVolume(fp: Box, s: HostSurface): Volume | null {
  const y0 = s.h + s.clear;
  return y0 < EYE_HEIGHT ? { ...fp, y0, y1: EYE_HEIGHT } : null;
}

/**
 * The host surface curio `c` stands on, found from its box and height: the
 * `hostSurfaces` entry at `c.h` whose box holds `c`'s box (`Curio` records
 * no surface of its own; `canned.test.ts`'s `hostOf` finds it the same
 * way, and `spots.test.ts` pins that exactly one surface matches every
 * curio of the gallery and the hero hall). Undefined for a curio that
 * stands on no surface at all, which `placeCurios` never gives.
 */
function surfaceOf(
  surfaces: readonly HostSurface[],
  c: Curio,
): HostSurface | undefined {
  const box = curioBox(c);
  return surfaces.find(
    (s) =>
      Math.abs(s.h - c.h) < 1e-6 &&
      box.x0 >= s.box.x0 - 1e-6 &&
      box.x1 <= s.box.x1 + 1e-6 &&
      box.z0 >= s.box.z0 - 1e-6 &&
      box.z1 <= s.box.z1 + 1e-6,
  );
}

/**
 * Every solid volume in `room` a curio-framing sight line must not cross:
 * every fixture's, decor piece's and floor prop's footprint (`footprintOf`,
 * `decorFootprint`, `propFootprint`) stood up from the floor to
 * `EYE_HEIGHT` (the room model carries no taller per-kind height for these,
 * and the sight line never rises over the eye), and every hero's footprint
 * stood from its own `heroLift` up to its `HERO_CATALOGUE[kind].top` (C16):
 * a lifted hero's column starts at its lift, not the floor, so a sight line
 * that passes under it counts as clear. A host's surfaces lie inside its
 * footprint, below that top, so they occlude as part of it and need no
 * volume of their own.
 *
 * The one host `c` stands on or under (`surfaceOf`) is neither solid nor
 * skipped: it gives only `ceilingVolume`, its footprint from `c`'s own
 * surface's ceiling up to the eye, one rule for fixture, decor, prop and
 * hero hosts alike. Skipping the host whole would count the top of a
 * bench as glass over the case below it. A `"terminal"` host adds, on top
 * of the rule, every part `TERMINAL_OCCLUDERS` names
 * (`render/models/terminal.ts`'s own box and cylinder calls), turned into
 * a world volume (`turnedBox`, the fixture's own `wallAnchor`): its desk
 * ends are open tops, so the rule alone leaves the terminal empty, while
 * the pedestals, the keyboard deck, the CRT and the chair are drawn there
 * and hide a curio on a desk end from the far side.
 *
 * Every other curio in the room occludes too: its own plan box
 * (`curioBox`) from its lift over its surface (`h + curioLift(o.kind)`,
 * so the gap under the hovering drone stays open) up to its top (`h +
 * curioSize(o).top`), so a sword standing in front of another on the same
 * bench, or a gadget in front of a cradle, moves the search on to a spot
 * that sees the framed curio itself. Only `c` is left out.
 *
 * The legs and end slabs a host stands on below an under spot (a bench's,
 * a workbench's, the hydroponics trough's) are in no volume, so a sight
 * line through a host's end counts as clear.
 */
function occludersFor(room: RoomSpec, c: Curio): Volume[] {
  const own = surfaceOf(hostSurfaces(room), c);
  const ownAnchor: object | undefined = own?.anchorOf;
  const out: Volume[] = [];
  const add = (anchor: object, fp: Box | null, top: number, y0 = 0) => {
    if (fp === null) return;
    if (own === undefined || anchor !== ownAnchor) {
      out.push({ ...fp, y0, y1: top });
      return;
    }
    const v = ceilingVolume(fp, own);
    if (v !== null) out.push(v);
  };
  for (const f of room.fixtures) {
    add(f, footprintOf(f), EYE_HEIGHT);
    if ((f as object) === ownAnchor && f.kind === "terminal") {
      const at = wallAnchor(f.slot);
      for (const part of TERMINAL_OCCLUDERS) {
        out.push({
          ...turnedBox(at.x, at.y, at.turn, part),
          y0: part.h0,
          y1: part.h1,
        });
      }
    }
  }
  for (const d of room.decor) add(d, decorFootprint(d), EYE_HEIGHT);
  for (const p of room.props) add(p, propFootprint(p), EYE_HEIGHT);
  for (const h of room.heroes)
    add(h, heroFootprint(h), HERO_CATALOGUE[h.kind].top, heroLift(h.kind));
  for (const o of room.curios) {
    if (o === c) continue;
    out.push({
      ...curioBox(o),
      y0: o.h + curioLift(o.kind),
      y1: o.h + curioSize(o).top,
    });
  }
  return out;
}

/** The tolerance `segmentHitsBox` treats a near-parallel or near-zero span as exactly zero. */
const EPS = 1e-9;

/** A point in world metres, `y` up. */
interface Point3 {
  x: number;
  y: number;
  z: number;
}

/**
 * Shrinks `[tMin, tMax]` (a segment's own parameter range, `P(t) = p0 + t *
 * (p1 - p0)`) to where the segment lies between `lo` and `hi` on one axis,
 * given that axis's own two coordinates: `[NaN, NaN]` when the segment runs
 * parallel to the axis and outside the slab there (never crosses on any
 * axis), the unchanged range when parallel and inside it (that axis never
 * narrows the range), else the ordinary two-plane intersection. One call
 * per axis is `segmentHitsBox`'s slab method.
 */
function narrowRange(
  tMin: number,
  tMax: number,
  p0: number,
  p1: number,
  lo: number,
  hi: number,
): readonly [number, number] {
  const d = p1 - p0;
  if (Math.abs(d) < EPS) {
    return p0 < lo || p0 > hi ? [NaN, NaN] : [tMin, tMax];
  }
  const a = (lo - p0) / d;
  const b = (hi - p0) / d;
  const t0 = Math.min(a, b);
  const t1 = Math.max(a, b);
  return [Math.max(tMin, t0), Math.min(tMax, t1)];
}

/**
 * True when the segment from `p0` to `p1` (world metres) crosses `v`'s
 * interior: the standard slab method, narrowing `[tMin, tMax]` on each axis
 * in turn (`narrowRange`). `tMin < tMax` (not `<=`) at the end means merely
 * touching a face, as a segment ending exactly on a volume's boundary does,
 * does not count as crossing it.
 */
function segmentHitsBox(p0: Point3, p1: Point3, v: Volume): boolean {
  let [tMin, tMax] = narrowRange(0, 1, p0.x, p1.x, v.x0, v.x1);
  [tMin, tMax] = narrowRange(tMin, tMax, p0.y, p1.y, v.y0, v.y1);
  [tMin, tMax] = narrowRange(tMin, tMax, p0.z, p1.z, v.z0, v.z1);
  return tMin < tMax;
}

/**
 * True when the segment from `eye` to `target` crosses none of `occluders`:
 * the shared predicate both `frameCurio`'s own search (given the occluders
 * it computed once for its curio) and `curioSightClear` (given a fresh
 * `occludersFor`, for a test's one-off check) apply.
 */
function sightClear(
  eye: Point3,
  target: Point3,
  occluders: readonly Volume[],
): boolean {
  return !occluders.some((v) => segmentHitsBox(eye, target, v));
}

/**
 * True when the sight line from world point `from` at eye height to curio
 * `c`'s middle (`curioMid`, halfway from its lift to its top) crosses no
 * occluding volume (`occludersFor`): the exact predicate `frameCurio`'s
 * own search applies at every candidate, exported so `spots.test.ts` can
 * check a forced curio's chosen spot with it directly.
 */
export function curioSightClear(
  room: RoomSpec,
  from: { x: number; z: number },
  c: Curio,
): boolean {
  const box = curioBox(c);
  const cx = (box.x0 + box.x1) / 2;
  const cz = (box.z0 + box.z1) / 2;
  const midY = curioMid(c);
  return sightClear(
    { x: from.x, y: EYE_HEIGHT, z: from.z },
    { x: cx, y: midY, z: cz },
    occludersFor(room, c),
  );
}

/** A hero whose top is under this is low: framed pitched down to its middle (C16). */
export const LOW_HERO_TOP = 1.0;

/**
 * Whether `?at=` frames a hero kind with a pitch (C16): a hovering one
 * (`heroLift` over 0) or a low one (top under `LOW_HERO_TOP`). Every
 * other hero keeps pitch 0.
 */
export function framesPitched(kind: HeroKind): boolean {
  return heroLift(kind) > 0 || HERO_CATALOGUE[kind].top < LOW_HERO_TOP;
}

/** The pitch from a spawn to a hero's middle, clamped to `MAX_PITCH`. */
function pitchTo(spawn: RoomSpec["spawn"], h: Hero): number {
  const box = heroFootprint(h);
  const px = (spawn.x + 0.5) * CELL;
  const pz = (spawn.y + 0.5) * CELL;
  const dist = Math.hypot(
    (box.x0 + box.x1) / 2 - px,
    (box.z0 + box.z1) / 2 - pz,
  );
  const mid = (heroLift(h.kind) + HERO_CATALOGUE[h.kind].top) / 2;
  return Math.max(
    -MAX_PITCH,
    Math.min(MAX_PITCH, -Math.atan2(EYE_HEIGHT - mid, dist)),
  );
}

/**
 * Whether the eye at `from` (metres, at `EYE_HEIGHT`) sees hero `h`'s
 * middle (its box's centre, halfway from its lift to its top): no other
 * hero (from its lift to its top), fixture, decor piece or floor prop
 * (from the floor to the eye) stands on the line. The hero hall's layout
 * is held to it (C16).
 */
export function heroSightClear(
  room: RoomSpec,
  from: { x: number; z: number },
  h: Hero,
): boolean {
  const box = heroFootprint(h);
  const target: Point3 = {
    x: (box.x0 + box.x1) / 2,
    y: (heroLift(h.kind) + HERO_CATALOGUE[h.kind].top) / 2,
    z: (box.z0 + box.z1) / 2,
  };
  const out: Volume[] = [];
  const add = (fp: Box | null) => {
    if (fp !== null) out.push({ ...fp, y0: 0, y1: EYE_HEIGHT });
  };
  for (const f of room.fixtures) add(footprintOf(f));
  for (const d of room.decor) add(decorFootprint(d));
  for (const p of room.props) add(propFootprint(p));
  for (const o of room.heroes)
    if (o !== h)
      out.push({
        ...heroFootprint(o),
        y0: heroLift(o.kind),
        y1: HERO_CATALOGUE[o.kind].top,
      });
  return sightClear({ x: from.x, y: EYE_HEIGHT, z: from.z }, target, out);
}

/**
 * A spot that frames curio `c` close and tilted down (C18), with a clear
 * sight line to it: along each of `sidesOf(c)`'s four directions, in the
 * order `view` gives (front, right, left, back for `front` and `close`;
 * back, front, right, left for `back`; right, front, left, back for
 * `side`; and for `quarter` the diagonal between the front and the right
 * first, then front, right, left, back, 2.6d C20), the distance from the
 * curio's centre grows from `CURIO_NEAR` by `CURIO_STEP` up to
 * `CURIO_FAR`. Among the spots whose player circle is on the floor and
 * clear of every blocker, the first one whose sight line (eye height to
 * the curio's middle) crosses no volume of `occludersFor` wins
 * (`sightClear`). `close` starts at `CURIO_CLOSE_NEAR` instead, takes each
 * side's first such spot and keeps the nearest of the four, so a curio at
 * a long table's end is seen from the end rather than across the table
 * (2.6d). When none sees the curio, the
 * search's first standable spot wins instead, so the seam never gives up a
 * spot to stand on. Its pitch looks at the curio's middle (`curioMid`:
 * halfway from its lift to its top, so a hovering curio is looked at, not
 * the gap under it), clamped to `MAX_PITCH`. Null when no side and distance
 * stands the player at all.
 */
function frameCurio(
  room: RoomSpec,
  c: Curio,
  view: SpotView,
): { spawn: RoomSpec["spawn"]; pitch: number } | null {
  const blockers = blockersFor(room);
  const onFloor = onFloorAt(room);
  const box = curioBox(c);
  const cx = (box.x0 + box.x1) / 2;
  const cz = (box.z0 + box.z1) / 2;
  const front = HERO_FRONT[c.turn] ?? [0, -1];
  const all = sidesOf(front);
  const pick = (order: readonly number[]) =>
    order.flatMap((i) => {
      const d = all[i];
      return d === undefined ? [] : [d];
    });
  const dirs: readonly (readonly [number, number])[] =
    view === "back"
      ? pick([3, 0, 1, 2])
      : view === "side"
        ? pick([1, 0, 2, 3])
        : view === "quarter"
          ? [viewDir(front, "quarter"), ...all]
          : all;
  const midY = curioMid(c);
  const target: Point3 = { x: cx, y: midY, z: cz };
  const occluders = occludersFor(room, c);
  let fallback: { spawn: RoomSpec["spawn"]; pitch: number } | null = null;
  const near = view === "close" ? CURIO_CLOSE_NEAR : CURIO_NEAR;
  const steps = Math.round((CURIO_FAR - near) / CURIO_STEP);
  let closest: {
    found: { spawn: RoomSpec["spawn"]; pitch: number };
    dist: number;
  } | null = null;
  for (const dir of dirs) {
    for (let step = 0; step <= steps; step++) {
      const dist = near + step * CURIO_STEP;
      const px = cx + dir[0] * dist;
      const pz = cz + dir[1] * dist;
      if (
        !onFloor(px, pz) ||
        blockers.some((b) => circleOverlapsBox(px, pz, b))
      ) {
        continue;
      }
      const pitch = Math.max(
        -MAX_PITCH,
        Math.min(MAX_PITCH, -Math.atan2(EYE_HEIGHT - midY, dist)),
      );
      const found = {
        spawn: {
          x: px / CELL - 0.5,
          y: pz / CELL - 0.5,
          yaw: Math.atan2(dir[0], dir[1]),
        },
        pitch,
      };
      fallback ??= found;
      const eye: Point3 = { x: px, y: EYE_HEIGHT, z: pz };
      if (!sightClear(eye, target, occluders)) continue;
      if (view !== "close") return found;
      if (closest === null || dist < closest.dist) closest = { found, dist };
      break;
    }
  }
  return closest?.found ?? fallback;
}

/**
 * Dev-only spawn points, with a pitch, for judging a fixture, hero, prop or
 * curio up close. `spotView(room, "<kind>:<n>")` is the n-th fixture of that
 * kind (in `room.fixtures` order, from 0) seen from its own cell, facing its
 * wall and backed off clear of its own footprint when it has one
 * (`spotFor`), pitch 0. `spotView(room, "prop:<kind>:<n>")` (H16) is the
 * n-th hero of that kind (in `room.heroes` order) when `HERO_KINDS` holds
 * it, else the n-th prop of that kind (in `room.props` order) when
 * `PROP_KINDS` holds it, framed from its front (`frameSpot`,
 * `HERO_FRONT[turn]`), or from the direction its view suffix names
 * (`viewDir`: `:back`, `:side`, `:quarter`; `:close` from the front but
 * nearer, 2.6d C20): pitch 0, or for a lifted or low hero a pitch toward
 * its middle (C16), clamped to `MAX_PITCH` (`framesPitched`, `pitchTo`); a
 * prop always keeps pitch 0. A wall or ceiling prop, which has no
 * `propFootprint`, is framed by the box of its own edge (`EDGE_PROP_SIZE`).
 * `n` is the ordinal a hero or prop of that kind holds in the room's own
 * order (`HERO_ORDER` for a hero), not its variant: the two usually line up,
 * but the field pack is one kind where they do not (its variant 1 sorts
 * before variant 0 in the hero hall).
 *
 * `spotView(room, "prop:<kind>:<n>")` (C18, 2.6b), when `kind` is
 * neither a hero nor a prop kind, is the n-th curio of that kind (in
 * `room.curios` order), framed close and tilted down (`frameCurio`), with a
 * real pitch; a view suffix changes the order its sides are tried in
 * (`:back` its `-front` side first, `:side` its right first, `:quarter`
 * the diagonal between its front and its right first). `spotView(room,
 * "decor:<kind>:<n>")` (2.7 Task 1) is the n-th decor piece of that kind (in
 * `room.decor` order), framed from its front (`frameSpot` on
 * `decorFootprint`), pitch 0; null for a piece with no footprint (a pipe
 * run, which hangs from the ceiling). Null for a bad
 * spot: an unknown kind in every one of the four families, an unknown
 * view, no such ordinal, no ordinal at all, or a negative one. The gallery reads this from `?at=`;
 * browser shots start every malfunction, every model or every curio there.
 * Development only, like everything in `dev/`.
 *
 * `spotSpawn` is `spotView` with the pitch dropped, for every caller that
 * only ever reads the spawn.
 */
export function spotView(
  room: RoomSpec,
  spot: string,
): { spawn: RoomSpec["spawn"]; pitch: number } | null {
  const fixture = FIXTURE_SPOT.exec(spot);
  if (fixture !== null) {
    const [, kind, n] = fixture;
    const f = room.fixtures.filter((x) => x.kind === kind)[Number(n)];
    return f === undefined ? null : { spawn: spotFor(f), pitch: 0 };
  }
  const decor = DECOR_SPOT.exec(spot);
  if (decor !== null) {
    const [, kind, n] = decor;
    const d = room.decor.filter((x) => x.kind === kind)[Number(n)];
    if (d === undefined) return null;
    const box = decorFootprint(d);
    if (box === null) return null;
    const spawn = frameSpot(room, box, HERO_FRONT[d.turn] ?? [0, -1]);
    return spawn === null ? null : { spawn, pitch: 0 };
  }
  const prop = PROP_SPOT.exec(spot);
  if (prop === null) return null;
  const [, kind, n, suffix] = prop;
  if (kind === undefined || n === undefined) return null;
  const i = Number(n);
  const view = (suffix ?? "front") as SpotView;
  const close = view === "close";
  if ((HERO_KINDS as readonly string[]).includes(kind)) {
    const h = room.heroes.filter((x) => x.kind === kind)[i];
    if (h === undefined) return null;
    const spawn = frameSpot(
      room,
      heroFootprint(h),
      viewDir(HERO_FRONT[heroTurn(h)] ?? [0, -1], view),
      close,
    );
    return spawn === null
      ? null
      : { spawn, pitch: framesPitched(h.kind) ? pitchTo(spawn, h) : 0 };
  }
  if ((PROP_KINDS as readonly string[]).includes(kind)) {
    const p = room.props.filter((x) => x.kind === kind)[i];
    if (p === undefined) return null;
    const box = propFootprint(p) ?? footprint(edgeOf(p), EDGE_PROP_SIZE);
    const spawn = frameSpot(
      room,
      box,
      viewDir(HERO_FRONT[p.turn] ?? [0, -1], view),
      close,
    );
    return spawn === null ? null : { spawn, pitch: 0 };
  }
  const c = room.curios.filter((x) => x.kind === kind)[i];
  if (c === undefined) return null;
  return frameCurio(room, c, view);
}

/**
 * `spotView`'s spawn alone, its pitch dropped: for a caller that only reads
 * where the player stands.
 */
export function spotSpawn(
  room: RoomSpec,
  spot: string,
): RoomSpec["spawn"] | null {
  return spotView(room, spot)?.spawn ?? null;
}
