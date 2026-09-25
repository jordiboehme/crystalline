/**
 * Dev-only spawn points: places the player in front of a gallery fixture,
 * hero or prop, so the controller's browser shots start every malfunction,
 * or every model, already facing it.
 *
 * Two patterns. `<kind>:<n>` (`SPOT_KINDS` pins the fixture kinds it
 * accepts to `Fixture["kind"]`, so a new kind is a type error here until it
 * is added) is `wallFacingSpawn`'s cell-centre spot, backed off clear of
 * the fixture's own footprint when it has one (`spotFor`). `prop:<kind>:<n>`
 * (H16) frames the n-th hero or prop of that kind instead: a spot in front
 * of its face, at a distance that frames it, from `frameSpot`. Both give
 * null for a bad spot: an unknown kind, no such ordinal, no ordinal at all,
 * or a negative one.
 */

import {
  HERO_FRONT,
  footprint,
  footprintOf,
  heroFootprint,
  propFootprint,
} from "../world/footprints";
import { HERO_KINDS } from "../world/heroes";
import { wallFacingSpawn, wallPoint } from "../world/interact";
import { isFloor } from "../world/layout";
import { blockersFor, PLAYER_RADIUS } from "../world/move";
import { PROP_KINDS } from "../world/props";
import { edgeOf } from "../world/sites";
import type { Box, Fixture, RoomSpec } from "../world/types";
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

const PROP_SPOT = /^prop:([a-z-]+):(\d+)$/;

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
  const cx = (box.x0 + box.x1) / 2;
  const cz = (box.z0 + box.z1) / 2;
  const halfX = (box.x1 - box.x0) / 2;
  const halfZ = (box.z1 - box.z0) / 2;
  const longer = Math.max(box.x1 - box.x0, box.z1 - box.z0);
  const sides: readonly (readonly [number, number])[] = [
    front,
    [front[1] * -1, front[0]],
    [front[1], -front[0]],
    [-front[0], -front[1]],
  ];
  const onFloor = (px: number, pz: number) =>
    [px - PLAYER_RADIUS, px + PLAYER_RADIUS].every((x) =>
      [pz - PLAYER_RADIUS, pz + PLAYER_RADIUS].every((z) =>
        isFloor(room.grid, Math.floor(x / CELL), Math.floor(z / CELL)),
      ),
    );
  for (const dir of sides) {
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

/**
 * Dev-only spawn points for judging a fixture, hero or prop up close.
 * `spotSpawn(room, "<kind>:<n>")` is the n-th fixture of that kind (in
 * `room.fixtures` order, from 0) seen from its own cell, facing its wall
 * and backed off clear of its own footprint when it has one (`spotFor`).
 * `spotSpawn(room, "prop:<kind>:<n>")` (H16) is the n-th hero of that kind
 * (in `room.heroes` order) when `HERO_KINDS` holds it, else the n-th prop
 * of that kind (in `room.props` order) when `PROP_KINDS` holds it, framed
 * from its front (`frameSpot`, `HERO_FRONT[turn]`): a wall or ceiling prop,
 * which has no `propFootprint`, is framed by the box of its own edge
 * (`EDGE_PROP_SIZE`). `n` is the ordinal a hero or prop of that kind holds
 * in the room's own order (`HERO_ORDER` for a hero), not its variant: the
 * two usually line up, but the field pack is one kind where they do not
 * (its variant 1 sorts before variant 0 in the hero hall). Null for a bad
 * spot: an unknown kind, no such ordinal, no ordinal at all, or a negative
 * one. The gallery reads both from `?at=`; the controller's browser shots
 * start every malfunction or every model there. Development only, like
 * everything in `dev/`.
 */
export function spotSpawn(
  room: RoomSpec,
  spot: string,
): RoomSpec["spawn"] | null {
  const fixture = FIXTURE_SPOT.exec(spot);
  if (fixture !== null) {
    const [, kind, n] = fixture;
    const f = room.fixtures.filter((x) => x.kind === kind)[Number(n)];
    return f === undefined ? null : spotFor(f);
  }
  const prop = PROP_SPOT.exec(spot);
  if (prop === null) return null;
  const [, kind, n] = prop;
  if (kind === undefined || n === undefined) return null;
  const i = Number(n);
  if ((HERO_KINDS as readonly string[]).includes(kind)) {
    const h = room.heroes.filter((x) => x.kind === kind)[i];
    if (h === undefined) return null;
    return frameSpot(room, heroFootprint(h), HERO_FRONT[h.turn] ?? [0, -1]);
  }
  if ((PROP_KINDS as readonly string[]).includes(kind)) {
    const p = room.props.filter((x) => x.kind === kind)[i];
    if (p === undefined) return null;
    const box = propFootprint(p) ?? footprint(edgeOf(p), EDGE_PROP_SIZE);
    return frameSpot(room, box, HERO_FRONT[p.turn] ?? [0, -1]);
  }
  return null;
}
