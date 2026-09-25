/**
 * Dev-only spawn points: places the player in front of a gallery fixture
 * for the gallery's `?at=` parameter, so the controller's browser shots
 * start every malfunction already facing it, clear of the fixture's own
 * footprint when it has one. `SPOT_KINDS` pins the fixture kinds
 * `spotSpawn` accepts to `Fixture["kind"]`, so a new kind is a type error
 * here until it is added.
 */

import { footprintOf } from "../world/footprints";
import { wallFacingSpawn, wallPoint } from "../world/interact";
import { PLAYER_RADIUS } from "../world/move";
import type { Fixture, RoomSpec } from "../world/types";
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
 * Dev-only spawn points for judging a fixture up close: `spotSpawn(room,
 * "<kind>:<n>")` is the n-th fixture of that kind (in `room.fixtures`
 * order, from 0) seen from its own cell, facing its wall and backed off
 * clear of its own footprint when it has one (`spotFor`), or null when the
 * room has no such fixture. The gallery reads it from `?at=`; the
 * controller's browser shots start every malfunction there. Development
 * only, like everything in `dev/`.
 */
export function spotSpawn(
  room: RoomSpec,
  spot: string,
): RoomSpec["spawn"] | null {
  const m = FIXTURE_SPOT.exec(spot);
  if (m === null) return null;
  const [, kind, n] = m;
  const f = room.fixtures.filter((x) => x.kind === kind)[Number(n)];
  return f === undefined ? null : spotFor(f);
}
