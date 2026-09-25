/**
 * Dev-only spawn points: places the player in front of a gallery fixture,
 * hero, prop or curio, so the controller's browser shots start every
 * malfunction, or every model, already facing it.
 *
 * Three patterns. `<kind>:<n>` (`SPOT_KINDS` pins the fixture kinds it
 * accepts to `Fixture["kind"]`, so a new kind is a type error here until it
 * is added) is `wallFacingSpawn`'s cell-centre spot, backed off clear of
 * the fixture's own footprint when it has one (`spotFor`). `prop:<kind>:<n>`
 * (H16) frames the n-th hero or prop of that kind instead: a spot in front
 * of its face, at a distance that frames it, from `frameSpot`.
 * `prop:<kind>:<n>[:back]` (C18, 2.6b) frames the n-th curio of that kind
 * close and tilted down instead, when `kind` is neither a hero nor a prop
 * kind (the three families' names are disjoint, pinned in `world/curios.ts`).
 * All three give null for a bad spot: an unknown kind, no such ordinal, no
 * ordinal at all, or a negative one.
 *
 * `spotView` is the seam that carries a curio's pitch along with its spawn;
 * `spotSpawn` is `spotView` with the pitch dropped, for every caller that
 * only ever wants the fixture, hero and prop spots it always gave (their
 * pitch is always 0).
 */

import { TERMINAL_OCCLUDERS } from "../render/models/terminal";
import { curioBox, curioSize, hostSurfaces } from "../world/curios";
import {
  HERO_FRONT,
  decorFootprint,
  footprint,
  footprintOf,
  heroFootprint,
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
import type { Box, Curio, Fixture, RoomSpec } from "../world/types";
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

const PROP_SPOT = /^prop:([a-z-]+):(\d+)(:back)?$/;

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
 * order (`frameCurio` reorders back first with `:back`).
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
 * A spot that frames `box` from `front` (H16): the player stands in front of
 * the box's face, facing it, `FRAME_BASE + FRAME_SCALE * longer side` out,
 * stepping 0.25 m closer while the circle is off the floor or on a blocker,
 * down to `PLAYER_RADIUS + SPOT_CLEARANCE`; if no distance works, the other
 * three sides are tried (right, left, back). Null when none works.
 */
function frameSpot(
  room: RoomSpec,
  box: Box,
  front: readonly [number, number],
): RoomSpec["spawn"] | null {
  const blockers = blockersFor(room);
  const onFloor = onFloorAt(room);
  const cx = (box.x0 + box.x1) / 2;
  const cz = (box.z0 + box.z1) / 2;
  const halfX = (box.x1 - box.x0) / 2;
  const halfZ = (box.z1 - box.z0) / 2;
  const longer = Math.max(box.x1 - box.x0, box.z1 - box.z0);
  for (const dir of sidesOf(front)) {
    const half = dir[0] !== 0 ? halfX : halfZ;
    let dist = FRAME_BASE + FRAME_SCALE * longer;
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

/** How many `CURIO_STEP` steps `frameCurio` tries, from `CURIO_NEAR` to `CURIO_FAR` inclusive. */
const CURIO_STEPS = Math.round((CURIO_FAR - CURIO_NEAR) / CURIO_STEP);

/** An axis-aligned 3D box: `Box` (world x/z) plus a floor-to-ceiling height range. */
interface Volume extends Box {
  y0: number;
  y1: number;
}

/**
 * How thick a host's own top surface is treated as, in metres, for the
 * sight-line check (`occludersFor`): thin enough that it never reaches into
 * the space a curio standing on it occupies, thick enough to be a real 3D
 * box a line can be tested against rather than a degenerate plane.
 */
const SLAB_THICKNESS = 0.05;

/**
 * Every solid volume in `room` a curio-framing sight line must not cross,
 * other than `c`'s own box (2.6b's browser-shots review, item 4): every
 * fixture's, decor piece's and floor prop's footprint (`footprintOf`,
 * `decorFootprint`, `propFootprint`) stood up from the floor to
 * `EYE_HEIGHT` (the room model carries no taller per-kind height for these,
 * and nothing here needs to see over furniture taller than a standing
 * eye), every hero's footprint stood up to its own `HERO_CATALOGUE[kind]
 * .top`, and every host surface's own box as a thin slab at its own `h`
 * (`hostSurfaces`, `SLAB_THICKNESS`) - the desktop a curio sits under
 * counts as an occluder this way, without a new geometry table, since it
 * is already in the room model curios are placed from.
 *
 * The one host `c` itself stands on or under (found the way
 * `canned.test.ts`'s own `hostOf` does, by its box and height) is not
 * skipped wholesale any more (review round 1's bug): the chair, monitor
 * and keyboard the review's browser pass found hiding a curio are part of
 * that very fixture's own model, so excluding its whole footprint dropped
 * them too, leaving only the thin top slab behind. Instead, for a
 * `"terminal"` fixture (the only host the review's five URLs implicate),
 * every part `TERMINAL_OCCLUDERS` names (`render/models/terminal.ts`'s own
 * box and cylinder calls, read from the model rather than guessed) is
 * turned into a world volume (`turnedBox`, the fixture's own `wallAnchor`)
 * and added instead of the fixture's plain footprint: the pedestals, the
 * keyboard deck, the CRT and the chair all still occlude, while the desk
 * top, which `TERMINAL_OCCLUDERS` never covers, stays clear for a curio
 * standing on it. (The knee space the chair fills carries no curio host
 * any more, fix round 3, `world/curios.ts`'s `FIXTURE_SURFACES.terminal`;
 * this check would still find it occluded by the chair if it did.) A
 * fixture kind with no parts table (every other kind, for now) keeps the
 * old whole-footprint skip. `c`'s own surface is still skipped from the
 * slab pass on top of this, so `c` never occludes the line drawn to its
 * own centre.
 */
function occludersFor(room: RoomSpec, c: Curio): Volume[] {
  const surfaces = hostSurfaces(room);
  const box = curioBox(c);
  const ownSurface = surfaces.find(
    (s) =>
      Math.abs(s.h - c.h) < 1e-6 &&
      box.x0 >= s.box.x0 - 1e-6 &&
      box.x1 <= s.box.x1 + 1e-6 &&
      box.z0 >= s.box.z0 - 1e-6 &&
      box.z1 <= s.box.z1 + 1e-6,
  );
  const ownAnchor: object | undefined = ownSurface?.anchorOf;
  const out: Volume[] = [];
  for (const f of room.fixtures) {
    if ((f as object) === ownAnchor) {
      if (f.kind === "terminal") {
        const at = wallAnchor(f.slot);
        for (const part of TERMINAL_OCCLUDERS) {
          out.push({
            ...turnedBox(at.x, at.y, at.turn, part),
            y0: part.h0,
            y1: part.h1,
          });
        }
      }
      continue;
    }
    const fp = footprintOf(f);
    if (fp !== null) out.push({ ...fp, y0: 0, y1: EYE_HEIGHT });
  }
  for (const d of room.decor) {
    if ((d as object) === ownAnchor) continue;
    const fp = decorFootprint(d);
    if (fp !== null) out.push({ ...fp, y0: 0, y1: EYE_HEIGHT });
  }
  for (const p of room.props) {
    if ((p as object) === ownAnchor) continue;
    const fp = propFootprint(p);
    if (fp !== null) out.push({ ...fp, y0: 0, y1: EYE_HEIGHT });
  }
  for (const h of room.heroes) {
    if ((h as object) === ownAnchor) continue;
    out.push({
      ...heroFootprint(h),
      y0: 0,
      y1: HERO_CATALOGUE[h.kind].top,
    });
  }
  for (const s of surfaces) {
    if (s === ownSurface) continue;
    out.push({ ...s.box, y0: s.h, y1: s.h + SLAB_THICKNESS });
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
 * `c`'s middle crosses no occluding volume (`occludersFor`): the exact
 * predicate `frameCurio`'s own search applies at every candidate, exported
 * so `spots.test.ts` can check a forced curio's chosen spot with it
 * directly (2.6b's browser-shots review, item 4).
 */
export function curioSightClear(
  room: RoomSpec,
  from: { x: number; z: number },
  c: Curio,
): boolean {
  const box = curioBox(c);
  const cx = (box.x0 + box.x1) / 2;
  const cz = (box.z0 + box.z1) / 2;
  const midY = c.h + curioSize(c).top / 2;
  return sightClear(
    { x: from.x, y: EYE_HEIGHT, z: from.z },
    { x: cx, y: midY, z: cz },
    occludersFor(room, c),
  );
}

/**
 * A spot that frames curio `c` close and tilted down (C18), with a clear
 * sight line to it (2.6b's browser-shots review, item 4): along each of
 * `sidesOf(c)`'s four directions (front, right, left, back; back first
 * when `back`), the distance from the curio's centre grows from
 * `CURIO_NEAR` by `CURIO_STEP` up to `CURIO_FAR`. Among the spots whose
 * player circle is on the floor and clear of every blocker, the first one
 * whose sight line (eye height to the curio's middle) crosses no volume of
 * `occludersFor` wins (`sightClear`); when none does, the search's first
 * standable spot wins instead, exactly as it did before this ruling, so
 * the seam never gives up a spot it used to find. Its pitch looks at the
 * curio's middle (`c.h` plus half its top height), clamped to `MAX_PITCH`.
 * Null when no side and distance stands the player at all.
 */
function frameCurio(
  room: RoomSpec,
  c: Curio,
  back: boolean,
): { spawn: RoomSpec["spawn"]; pitch: number } | null {
  const blockers = blockersFor(room);
  const onFloor = onFloorAt(room);
  const box = curioBox(c);
  const cx = (box.x0 + box.x1) / 2;
  const cz = (box.z0 + box.z1) / 2;
  const front = HERO_FRONT[c.turn] ?? [0, -1];
  const all = sidesOf(front);
  const order = back ? [3, 0, 1, 2] : [0, 1, 2, 3];
  const top = curioSize(c).top;
  const midY = c.h + top / 2;
  const target: Point3 = { x: cx, y: midY, z: cz };
  const occluders = occludersFor(room, c);
  let fallback: { spawn: RoomSpec["spawn"]; pitch: number } | null = null;
  for (const i of order) {
    const dir = all[i];
    if (dir === undefined) continue;
    for (let step = 0; step <= CURIO_STEPS; step++) {
      const dist = CURIO_NEAR + step * CURIO_STEP;
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
      if (sightClear(eye, target, occluders)) return found;
    }
  }
  return fallback;
}

/**
 * Dev-only spawn points, with a pitch, for judging a fixture, hero, prop or
 * curio up close. `spotView(room, "<kind>:<n>")` is the n-th fixture of
 * that kind (in `room.fixtures` order, from 0) seen from its own cell,
 * facing its wall and backed off clear of its own footprint when it has
 * one (`spotFor`), pitch 0. `spotView(room, "prop:<kind>:<n>")` (H16) is
 * the n-th hero of that kind (in `room.heroes` order) when `HERO_KINDS`
 * holds it, else the n-th prop of that kind (in `room.props` order) when
 * `PROP_KINDS` holds it, framed from its front (`frameSpot`,
 * `HERO_FRONT[turn]`), pitch 0: a wall or ceiling prop, which has no
 * `propFootprint`, is framed by the box of its own edge (`EDGE_PROP_SIZE`).
 * `n` is the ordinal a hero or prop of that kind holds in the room's own
 * order (`HERO_ORDER` for a hero), not its variant: the two usually line
 * up, but the field pack is one kind where they do not (its variant 1
 * sorts before variant 0 in the hero hall).
 *
 * `spotView(room, "prop:<kind>:<n>[:back]")` (C18, 2.6b), when `kind` is
 * neither a hero nor a prop kind, is the n-th curio of that kind (in
 * `room.curios` order), framed close and tilted down (`frameCurio`), with
 * a real pitch; `:back` frames it from its `-front` side first. Null for a
 * bad spot: an unknown kind in every one of the three families, no such
 * ordinal, no ordinal at all, or a negative one. The gallery reads this
 * from `?at=`; the controller's browser shots start every malfunction,
 * every model or every curio there. Development only, like everything in
 * `dev/`.
 *
 * `spotSpawn` is `spotView` with the pitch dropped, for every caller that
 * only ever reads the fixture, hero and prop spots (their pitch is always
 * 0).
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
  const prop = PROP_SPOT.exec(spot);
  if (prop === null) return null;
  const [, kind, n, back] = prop;
  if (kind === undefined || n === undefined) return null;
  const i = Number(n);
  if ((HERO_KINDS as readonly string[]).includes(kind)) {
    const h = room.heroes.filter((x) => x.kind === kind)[i];
    if (h === undefined) return null;
    const spawn = frameSpot(
      room,
      heroFootprint(h),
      HERO_FRONT[heroTurn(h)] ?? [0, -1],
    );
    return spawn === null ? null : { spawn, pitch: 0 };
  }
  if ((PROP_KINDS as readonly string[]).includes(kind)) {
    const p = room.props.filter((x) => x.kind === kind)[i];
    if (p === undefined) return null;
    const box = propFootprint(p) ?? footprint(edgeOf(p), EDGE_PROP_SIZE);
    const spawn = frameSpot(room, box, HERO_FRONT[p.turn] ?? [0, -1]);
    return spawn === null ? null : { spawn, pitch: 0 };
  }
  const c = room.curios.filter((x) => x.kind === kind)[i];
  if (c === undefined) return null;
  return frameCurio(room, c, back !== undefined);
}

/**
 * `spotView`'s spawn alone, pitch dropped: every caller that only ever
 * wanted the fixture, hero and prop spots (whose pitch is always 0) keeps
 * this signature.
 */
export function spotSpawn(
  room: RoomSpec,
  spot: string,
): RoomSpec["spawn"] | null {
  return spotView(room, spot)?.spawn ?? null;
}
