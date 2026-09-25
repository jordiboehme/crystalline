/**
 * `spotSpawn` picks the n-th fixture of a kind, in `room.fixtures` order,
 * and faces it as `wallFacingSpawn` would; a bad spot (unknown kind, no
 * such ordinal, no ordinal at all, or a negative one) gives null. Every
 * spot it accepts on the gallery room, including a terminal or a machine
 * backed off clear of its own footprint, leaves the player's circle
 * overlapping none of the room's blockers.
 */

import { describe, expect, it } from "vitest";

import { galleryRoom } from "../world/canned";
import { wallFacingSpawn } from "../world/interact";
import { PLAYER_RADIUS, blockersFor, spawnPlayer } from "../world/move";
import type { Box, Fixture, RoomSpec } from "../world/types";
import { spotSpawn } from "./spots";

/** The slot of the n-th fixture of `kind`, in fixture order. */
function slotOf(room: RoomSpec, kind: Fixture["kind"], n: number) {
  const f = room.fixtures.filter((x) => x.kind === kind)[n];
  if (f === undefined) throw new Error(`no ${kind} ${String(n)}`);
  return f.slot;
}

/**
 * True when a circle of the player's radius at (x, z) overlaps the box, the
 * same clamp-and-distance check `move.ts` collides the player with.
 */
function circleOverlapsBox(x: number, z: number, b: Box): boolean {
  const nx = Math.max(b.x0, Math.min(x, b.x1));
  const nz = Math.max(b.z0, Math.min(z, b.z1));
  const dx = x - nx;
  const dz = z - nz;
  return dx * dx + dz * dz < PLAYER_RADIUS * PLAYER_RADIUS;
}

/** Every kind `spotSpawn`'s regex accepts. */
const SPOT_KINDS: readonly Fixture["kind"][] = [
  "terminal",
  "door",
  "portal",
  "hatch",
  "machine",
  "poster",
  "placard",
];

describe("spotSpawn", () => {
  it("faces the n-th door of the room, from its own cell", () => {
    const room = galleryRoom();
    expect(spotSpawn(room, "door:4")).toEqual(
      wallFacingSpawn(slotOf(room, "door", 4)),
    );
  });

  it("faces the n-th hatch the same way", () => {
    const room = galleryRoom();
    expect(spotSpawn(room, "hatch:0")).toEqual(
      wallFacingSpawn(slotOf(room, "hatch", 0)),
    );
  });

  it("faces the n-th portal the same way", () => {
    const room = galleryRoom();
    expect(spotSpawn(room, "portal:2")).toEqual(
      wallFacingSpawn(slotOf(room, "portal", 2)),
    );
  });

  it("gives null past the last fixture of a kind", () => {
    expect(spotSpawn(galleryRoom(), "door:9")).toBeNull();
  });

  it("gives null for a kind the room has no fixture of", () => {
    expect(spotSpawn(galleryRoom(), "lift:0")).toBeNull();
  });

  it("gives null with no ordinal", () => {
    expect(spotSpawn(galleryRoom(), "door")).toBeNull();
  });

  it("gives null for a negative ordinal", () => {
    expect(spotSpawn(galleryRoom(), "door:-1")).toBeNull();
  });

  it("keeps every spot it accepts clear of every blocker in the room", () => {
    const room = galleryRoom();
    const blockers = blockersFor(room);
    const bad: string[] = [];
    for (const kind of SPOT_KINDS) {
      const count = room.fixtures.filter((f) => f.kind === kind).length;
      for (let n = 0; n < count; n++) {
        const spot = `${kind}:${n}`;
        const spawn = spotSpawn(room, spot);
        if (spawn === null) throw new Error(`spotSpawn rejected ${spot}`);
        const player = spawnPlayer({ ...room, spawn });
        if (blockers.some((b) => circleOverlapsBox(player.x, player.z, b))) {
          bad.push(spot);
        }
      }
    }
    expect(bad).toEqual([]);
  });
});
