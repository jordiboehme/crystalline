/**
 * Dev-only spawn points: places the player in front of a gallery fixture
 * for the gallery's `?at=` parameter, so the controller's browser shots
 * start every malfunction already facing it.
 */

import { wallFacingSpawn } from "../world/interact";
import type { RoomSpec } from "../world/types";

/**
 * Dev-only spawn points for judging a fixture up close: `spotSpawn(room,
 * "<kind>:<n>")` is the n-th fixture of that kind (in `room.fixtures`
 * order, from 0) seen from its own cell, facing its wall
 * (`wallFacingSpawn`), or null when the room has no such fixture. The
 * gallery reads it from `?at=`; the controller's browser shots start
 * every malfunction there. Development only, like everything in `dev/`.
 */
export function spotSpawn(
  room: RoomSpec,
  spot: string,
): RoomSpec["spawn"] | null {
  const m = /^(terminal|door|portal|hatch|machine|poster|placard):(\d+)$/.exec(
    spot,
  );
  if (m === null) return null;
  const [, kind, n] = m;
  const f = room.fixtures.filter((x) => x.kind === kind)[Number(n)];
  return f === undefined ? null : wallFacingSpawn(f.slot);
}
