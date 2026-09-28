/**
 * Using a room: what the player is facing, how its doors move, when a way
 * carries the player to the next room and where the player comes out.
 *
 * Everything here is pure: a room, a player and the doors' states go in, an
 * answer comes out, and the session (`session.ts`) applies it once a tick.
 * No DOM, no GPU and no clock, so every rule is tested on the canned rooms
 * as plain numbers.
 *
 * The geometry is the wall slot's. A fixture stands against one wall of its
 * cell, and the middle of that wall at floor level is its point
 * (`wallPoint`). From there `inward` points into the room and `along` runs
 * along the wall, the same frame the models are built in, so "0.4 m in front
 * of the door and inside its opening" means the same here as on screen.
 *
 * - A terminal is read, a hatch crawled through and a bulkhead or blast
 *   door opened or closed with Space, which is what `focusOf` offers.
 * - A sliding door opens by itself while the player is within `APPROACH`,
 *   and an unsealed portal needs nothing at all: walking into it is the
 *   action, so neither is offered for Space.
 * - A sealed door or portal is offered only to say why it is sealed.
 * - A lift is offered for Space (`SPACE LIFT`, M3 C26), which opens its
 *   list of stops; a station screen is only looked at, and an exit opens
 *   on approach like a sliding door (M3 C28), so neither is offered.
 *
 * A way that failed on travel (the session's `failed` map) is treated as
 * sealed: it is offered only to say why, heads shut and carries no one.
 *
 * A police box is not a fixture and leads nowhere the generator ever
 * built, so it is offered and stepped on its own: a police box's doors are
 * `world/box.ts`'s (`boxFocus`, `stepBoxDoors`), not this module's.
 */

import type { Player } from "./move";
import type {
  DoorStyle,
  Fixture,
  PlaceAddress,
  RoomSpec,
  WallSlot,
} from "./types";
import { CELL } from "./units";

/** How far away a fixture can be used, in metres. */
export const REACH = 2.2;

/**
 * How far off the view direction a fixture may lie and still be faced, in
 * radians.
 */
export const FACING = Math.PI / 4;

/** How close the player must be for a sliding door to open, in metres. */
export const APPROACH = 3.0;

/** How far a door moves towards open or shut in one 35 Hz tick. */
export const DOOR_STEP = 1 / 12;

/**
 * Half the clear opening of each door style, in metres, as the door models
 * cut it (`SLIDE_HALF`, `BULK_HALF` and `BLAST_HALF` in
 * `render/models/doors.ts`): the player travels only through the opening,
 * never through the jamb beside it.
 */
export const DOOR_HALF: Readonly<Record<DoorStyle, number>> = {
  sliding: 0.5,
  bulkhead: 0.5,
  blast: 0.8,
};

/**
 * How close to its wall the player must come for an open door to carry the
 * player through, in metres. The player's circle stops 0.35 m from a wall,
 * so this is a step into the doorway.
 */
export const DOOR_REACH = 0.6;

/**
 * How close to its wall the player must come for a portal to carry the
 * player through.
 */
export const PORTAL_REACH = 0.5;

/**
 * Half the portal's ring, in metres: the part of the wall that is the way
 * through.
 */
export const PORTAL_HALF = 0.75;

/** How far in front of the fixture the player arrives, in metres. */
export const ARRIVAL_DISTANCE = 1.6;

/** A door is open enough to walk through above this fraction. */
const OPEN_ENOUGH = 0.9;

/**
 * What the player is facing and can use.
 *
 * `index` is the fixture's index in `room.fixtures`, and `prompt` the line
 * the HUD shows for it:
 * - `SPACE READ <heading>` at a terminal;
 * - `SPACE OPEN <label>` or `SPACE CLOSE <label>` at a bulkhead or blast
 *   door, after where the door is heading;
 * - `SPACE CRAWL <label>` at a hatch;
 * - `SEALED <sealedLabel>` at a sealed door or portal, which does nothing;
 * - `SEALED <label>` at a way in `failed`;
 * - `SPACE LIFT` at a lift.
 */
export type Interactable = {
  kind: "terminal" | "door" | "hatch" | "portal" | "lift";
  index: number;
  prompt: string;
};

/**
 * One door's state: how far open it stands (`open`, 0 shut to 1 open) and
 * where it is heading (`target`). `open` moves `DOOR_STEP` towards `target`
 * each tick, so a door takes twelve ticks, about a third of a second, to
 * open or shut.
 */
export type DoorState = { open: number; target: 0 | 1 };

/**
 * A way out of the room the player has just taken: through a door, a portal
 * or a hatch, which fixture it was, and the place it leads to.
 */
export type Travel = {
  via: "door" | "portal" | "hatch";
  fixture: number;
  address: PlaceAddress;
};

/**
 * How the player came into a room: the kind of way and the place it came
 * from. It decides where the player arrives (`arrivalSpawn`).
 */
export type Arrival = { via: "door" | "portal" | "hatch"; from: PlaceAddress };

/**
 * A fixture's wall in world metres: the middle of the wall at floor level
 * (`x`, `z`), the horizontal direction into the room (`inward`) and the one
 * along the wall (`along`). The same frame `frameForSlot` gives the models.
 */
export interface WallPoint {
  x: number;
  z: number;
  inward: readonly [number, number];
  along: readonly [number, number];
}

/** The wall point of a slot; see `WallPoint`. */
export function wallPoint(slot: WallSlot): WallPoint {
  const cx = (slot.x + 0.5) * CELL;
  const cz = (slot.y + 0.5) * CELL;
  switch (slot.side) {
    case "n":
      return { x: cx, z: slot.y * CELL, inward: [0, 1], along: [1, 0] };
    case "s":
      return { x: cx, z: (slot.y + 1) * CELL, inward: [0, -1], along: [-1, 0] };
    case "w":
      return { x: slot.x * CELL, z: cz, inward: [1, 0], along: [0, -1] };
    case "e":
      return { x: (slot.x + 1) * CELL, z: cz, inward: [-1, 0], along: [0, 1] };
  }
}

/**
 * A spawn in a slot's own cell, facing its wall: the cell centre, 1 m in
 * front of the wall point, with `yaw` turned so the player's forward
 * `(-sin yaw, -cos yaw)` points into the wall. `RoomSpec.spawn` uses the
 * same cell and yaw convention, so a dev view can put the player in front
 * of a fixture by spreading this into a room's `spawn`.
 */
export function wallFacingSpawn(slot: WallSlot): {
  x: number;
  y: number;
  yaw: number;
} {
  const w = wallPoint(slot);
  return { x: slot.x, y: slot.y, yaw: Math.atan2(w.inward[0], w.inward[1]) };
}

/**
 * Where the player stands relative to a wall: `depth` metres in front of it
 * (negative behind it) and `side` metres along it from its middle. Shared
 * with `box.ts`'s police box front, in the same terms.
 */
export function relative(w: WallPoint, x: number, z: number) {
  const dx = x - w.x;
  const dz = z - w.z;
  return {
    depth: dx * w.inward[0] + dz * w.inward[1],
    side: dx * w.along[0] + dz * w.along[1],
  };
}

/**
 * The yaw that faces away from `inward` (into the room, out of a wall or a
 * police box's front): `forwardOf(yaw)` is `[-sin, -cos]`, so this is
 * `atan2(-inward[0], -inward[1])`. The `+ 0` turns a -0 into 0.
 */
export function yawFacing(inward: readonly [number, number]): number {
  return Math.atan2(-inward[0], -inward[1]) + 0;
}

/** Whether two addresses name the same place. */
export function samePlace(a: PlaceAddress, b: PlaceAddress): boolean {
  return a.domain === b.domain && a.permalink === b.permalink;
}

/**
 * Whether the player is walking up to the way in `slot`: in front of its
 * wall (never behind it, where a bay or the backlink corridor can lie a
 * wall's thickness away) and within `APPROACH` of its wall point. It opens
 * a sliding door and warms the cache for the place behind a door or portal.
 */
export function approaches(slot: WallSlot, player: Player): boolean {
  const w = wallPoint(slot);
  if (relative(w, player.x, player.z).depth <= 0) return false;
  return Math.hypot(player.x - w.x, player.z - w.z) < APPROACH;
}

/**
 * What the HUD offers for a fixture, or null when Space does nothing there and
 * nothing needs saying: an unsealed sliding door and an exit open on
 * approach and an unsealed portal on contact, and machines, posters, the
 * placard and screens are only looked at. A lift is always offered: Space
 * opens its stops.
 */
function offer(
  fixture: Fixture,
  index: number,
  doors: ReadonlyMap<number, DoorState>,
  failed: ReadonlyMap<number, string>,
): Interactable | null {
  switch (fixture.kind) {
    case "terminal":
      return {
        kind: "terminal",
        index,
        prompt: `SPACE READ ${fixture.heading}`,
      };
    case "hatch": {
      const seal = failed.get(index);
      if (seal !== undefined) {
        return { kind: "hatch", index, prompt: `SEALED ${seal}` };
      }
      return { kind: "hatch", index, prompt: `SPACE CRAWL ${fixture.label}` };
    }
    case "door": {
      const seal = failed.get(index) ?? fixture.sealedLabel;
      if (seal !== null || fixture.address === null) {
        return { kind: "door", index, prompt: `SEALED ${seal ?? ""}` };
      }
      if (fixture.style === "sliding") return null;
      const verb = doors.get(index)?.target === 1 ? "CLOSE" : "OPEN";
      return { kind: "door", index, prompt: `SPACE ${verb} ${fixture.label}` };
    }
    case "portal": {
      const seal = failed.get(index) ?? fixture.sealedLabel;
      if (seal !== null || fixture.address === null) {
        return { kind: "portal", index, prompt: `SEALED ${seal ?? ""}` };
      }
      return null;
    }
    case "lift":
      return { kind: "lift", index, prompt: "SPACE LIFT" };
    case "exit":
    case "machine":
    case "poster":
    case "placard":
    case "screen":
      return null;
  }
}

/**
 * The fixture the player is facing and can use, or null.
 *
 * Of the fixtures `offer` has something to say about, the nearest one whose
 * wall point is within `REACH`, whose wall faces the player (the player
 * stands in front of it, not behind it) and which lies within `FACING` of
 * the view direction. `doors` decides whether a door is offered to open or
 * to close; without it every door is taken to be shut.
 */
export function focusOf(
  room: RoomSpec,
  player: Player,
  doors: ReadonlyMap<number, DoorState> = new Map(),
  failed: ReadonlyMap<number, string> = new Map(),
): Interactable | null {
  const fx = -Math.sin(player.yaw);
  const fz = -Math.cos(player.yaw);
  const cosFacing = Math.cos(FACING);
  let best: Interactable | null = null;
  let bestDistance = Infinity;
  for (const [index, fixture] of room.fixtures.entries()) {
    const w = wallPoint(fixture.slot);
    if (relative(w, player.x, player.z).depth <= 0) continue;
    const dx = w.x - player.x;
    const dz = w.z - player.z;
    const distance = Math.hypot(dx, dz);
    if (distance > REACH || distance >= bestDistance) continue;
    if (distance > 0 && (dx * fx + dz * fz) / distance < cosFacing) continue;
    const candidate = offer(fixture, index, doors, failed);
    if (candidate === null) continue;
    best = candidate;
    bestDistance = distance;
  }
  return best;
}

/**
 * The doors one tick later.
 *
 * Every door of the room gets a state (a door not in `doors` starts shut).
 * A sliding door heads open while the player `approaches` it (in front
 * of its wall and within `APPROACH` of its wall point) and shut otherwise; a
 * bulkhead or blast door keeps heading where it was until `pressed`, the
 * index of the fixture Space was pressed at
 * this tick, names it, which turns it round. A sealed door always heads
 * shut, and so does a door in `failed`, whatever the player does. Then each
 * door moves `DOOR_STEP` towards where it is heading. Returns a new map;
 * `doors` is left as it was.
 */
export function stepDoors(
  room: RoomSpec,
  player: Player,
  doors: ReadonlyMap<number, DoorState>,
  pressed: number | null,
  failed: ReadonlyMap<number, string> = new Map(),
): Map<number, DoorState> {
  const out = new Map<number, DoorState>();
  room.fixtures.forEach((fixture, index) => {
    if (fixture.kind !== "door") return;
    const was = doors.get(index) ?? { open: 0, target: 0 };
    let target: 0 | 1;
    if (fixture.address === null || failed.has(index)) {
      target = 0;
    } else if (fixture.style === "sliding") {
      target = approaches(fixture.slot, player) ? 1 : 0;
    } else {
      target = pressed === index ? (was.target === 1 ? 0 : 1) : was.target;
    }
    const open =
      target === 1
        ? Math.min(1, was.open + DOOR_STEP)
        : Math.max(0, was.open - DOOR_STEP);
    // Twelve steps of a twelfth land a hair off 1 in floating point.
    out.set(index, {
      open: Math.abs(open - target) < 1e-9 ? target : open,
      target,
    });
  });
  return out;
}

/**
 * The way the player is walking through this tick, or null.
 *
 * A door carries the player through once it stands more than 0.9 open and
 * the player stands in front of its wall, within `DOOR_REACH` of it, and
 * inside its opening
 * (`DOOR_HALF`). An unsealed portal carries the player through on contact:
 * in front of its wall, within `PORTAL_REACH` of it, and inside its ring
 * (`PORTAL_HALF`). Nothing carries the player from behind a wall, where a
 * bay or the backlink corridor may lie. A way in `failed` carries no one,
 * whatever its `DoorState` says.
 * Hatches are crawled through on Space instead (`hatchTravel`).
 */
export function travelOf(
  room: RoomSpec,
  player: Player,
  doors: ReadonlyMap<number, DoorState>,
  failed: ReadonlyMap<number, string> = new Map(),
): Travel | null {
  for (const [index, fixture] of room.fixtures.entries()) {
    if (failed.has(index)) continue;
    if (fixture.kind === "door" && fixture.address !== null) {
      if ((doors.get(index)?.open ?? 0) <= OPEN_ENOUGH) continue;
      const r = relative(wallPoint(fixture.slot), player.x, player.z);
      if (
        r.depth >= 0 &&
        r.depth < DOOR_REACH &&
        Math.abs(r.side) < DOOR_HALF[fixture.style]
      ) {
        return { via: "door", fixture: index, address: fixture.address };
      }
    } else if (fixture.kind === "portal" && fixture.address !== null) {
      const r = relative(wallPoint(fixture.slot), player.x, player.z);
      if (
        r.depth >= 0 &&
        r.depth < PORTAL_REACH &&
        Math.abs(r.side) < PORTAL_HALF
      ) {
        return { via: "portal", fixture: index, address: fixture.address };
      }
    }
  }
  return null;
}

/**
 * The way through hatch `index`, or null when that fixture is not a hatch
 * or is in `failed`. The session calls it when Space is pressed at a hatch.
 */
export function hatchTravel(
  room: RoomSpec,
  index: number,
  failed: ReadonlyMap<number, string> = new Map(),
): Travel | null {
  const fixture = room.fixtures[index];
  if (fixture?.kind !== "hatch" || failed.has(index)) return null;
  return { via: "hatch", fixture: index, address: fixture.address };
}

/**
 * Where the player comes out in `room`, and facing where.
 *
 * - Through a door or portal from place A, the player arrives in front of
 *   this room's hatch that leads back to A.
 * - Through a hatch back to A, the player arrives in front of this room's
 *   door or portal that leads to A.
 * - Otherwise (no arrival, or no fixture matches) at the entrance.
 *
 * In front means `ARRIVAL_DISTANCE` out from the fixture's wall point,
 * facing into the room, away from the wall: the way back is right behind
 * the player.
 */
export function arrivalSpawn(
  room: RoomSpec,
  arrival: Arrival | null,
): { x: number; z: number; yaw: number } {
  const match =
    arrival === null
      ? undefined
      : room.fixtures.find((f) => {
          if (arrival.via === "hatch") {
            return (
              (f.kind === "door" || f.kind === "portal") &&
              f.address !== null &&
              samePlace(f.address, arrival.from)
            );
          }
          return f.kind === "hatch" && samePlace(f.address, arrival.from);
        });
  if (match === undefined) {
    return {
      x: (room.spawn.x + 0.5) * CELL,
      z: (room.spawn.y + 0.5) * CELL,
      yaw: room.spawn.yaw,
    };
  }
  const w = wallPoint(match.slot);
  return {
    x: w.x + w.inward[0] * ARRIVAL_DISTANCE,
    z: w.z + w.inward[1] * ARRIVAL_DISTANCE,
    yaw: yawFacing(w.inward),
  };
}
