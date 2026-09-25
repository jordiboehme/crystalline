/**
 * `spotSpawn` picks the n-th fixture of a kind, in `room.fixtures` order,
 * and faces it as `wallFacingSpawn` would; a bad spot (unknown kind, no
 * such ordinal, no ordinal at all, or a negative one) gives null. Every
 * spot it accepts on the gallery room, including a terminal or a machine
 * backed off clear of its own footprint, leaves the player's circle
 * overlapping none of the room's blockers. `prop:<kind>:<n>` (H16) frames a
 * hero or a prop instead, from a spot clear of every blocker.
 */

import { describe, expect, it } from "vitest";

import { galleryRoom, heroHallRoom } from "../world/canned";
import { heroFootprint, propFootprint } from "../world/footprints";
import { wallFacingSpawn } from "../world/interact";
import { blockersFor, spawnPlayer } from "../world/move";
import type { Box, Fixture, RoomSpec } from "../world/types";
import { SPOT_KINDS, circleOverlapsBox, spotSpawn } from "./spots";

/** The slot of the n-th fixture of `kind`, in fixture order. */
function slotOf(room: RoomSpec, kind: Fixture["kind"], n: number) {
  const f = room.fixtures.filter((x) => x.kind === kind)[n];
  if (f === undefined) throw new Error(`no ${kind} ${String(n)}`);
  return f.slot;
}

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
    for (const kind of Object.keys(SPOT_KINDS) as Fixture["kind"][]) {
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

  it("accepts exactly the fixture kinds", () => {
    const room = galleryRoom();
    const kinds = new Set(room.fixtures.map((f) => f.kind));
    expect(Object.keys(SPOT_KINDS).sort()).toEqual([...kinds].sort());
  });
});

describe("prop spots (H16)", () => {
  const hall = heroHallRoom();
  const gallery = galleryRoom();

  const looksAt = (room: RoomSpec, spot: string, box: Box) => {
    const spawn = spotSpawn(room, spot);
    if (spawn === null) throw new Error(`no spot for ${spot}`);
    const player = spawnPlayer({ ...room, spawn });
    const cx = (box.x0 + box.x1) / 2 - player.x;
    const cz = (box.z0 + box.z1) / 2 - player.z;
    const len = Math.hypot(cx, cz);
    // yaw 0 looks north, (-sin yaw, -cos yaw)
    const dot = (-Math.sin(player.yaw) * cx - Math.cos(player.yaw) * cz) / len;
    return { player, dot };
  };

  it("frames every hero of the hero hall, from a spot clear of every blocker", () => {
    const blockers = blockersFor(hall);
    const counts = new Map<string, number>();
    for (const h of hall.heroes) {
      const n = counts.get(h.kind) ?? 0;
      counts.set(h.kind, n + 1);
      const { player, dot } = looksAt(
        hall,
        `prop:${h.kind}:${String(n)}`,
        heroFootprint(h),
      );
      expect(dot, h.kind).toBeGreaterThan(0.99);
      expect(
        blockers.some((b) => circleOverlapsBox(player.x, player.z, b)),
        h.kind,
      ).toBe(false);
    }
  });

  it("frames a floor prop of the gallery the same way", () => {
    const crate = gallery.props.find((p) => p.kind === "crate");
    if (crate === undefined) throw new Error("the gallery has a crate");
    const box = propFootprint(crate);
    if (box === null) throw new Error("a crate has a box");
    const { dot } = looksAt(gallery, "prop:crate:0", box);
    expect(dot).toBeGreaterThan(0.99);
  });

  it("gives null for a kind the room lacks, past the last one, or a bad name", () => {
    expect(spotSpawn(gallery, "prop:turret:0")).toBeNull();
    expect(spotSpawn(hall, "prop:turret:1")).toBeNull();
    expect(spotSpawn(hall, "prop:nothing:0")).toBeNull();
    expect(spotSpawn(hall, "prop:turret")).toBeNull();
  });
});
