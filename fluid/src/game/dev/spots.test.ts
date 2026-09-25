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
import { curioBox, curioSize, CURIO_KINDS } from "../world/curios";
import { HERO_FRONT, heroFootprint, propFootprint } from "../world/footprints";
import { HERO_KINDS } from "../world/heroes";
import { wallFacingSpawn } from "../world/interact";
import { isFloor } from "../world/layout";
import { blockersFor, EYE_HEIGHT, MAX_PITCH, spawnPlayer } from "../world/move";
import { PROP_KINDS } from "../world/props";
import type { Box, Fixture, RoomSpec } from "../world/types";
import { CELL } from "../world/units";
import {
  CURIO_FAR,
  SPOT_KINDS,
  circleOverlapsBox,
  spotSpawn,
  spotView,
} from "./spots";

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

describe("spotView (C18)", () => {
  const hall = heroHallRoom();
  const gallery = galleryRoom();

  it("frames every curio of the hero hall close and tilted down", () => {
    const blockers = blockersFor(hall);
    const counts = new Map<string, number>();
    for (const c of hall.curios) {
      const n = counts.get(c.kind) ?? 0;
      counts.set(c.kind, n + 1);
      const spot = `prop:${c.kind}:${String(n)}`;
      const view = spotView(hall, spot);
      expect(view, spot).not.toBeNull();
      if (view === null) continue;

      const player = spawnPlayer({ ...hall, spawn: view.spawn });
      expect(
        blockers.some((b) => circleOverlapsBox(player.x, player.z, b)),
        spot,
      ).toBe(false);
      const onFloor = [player.x - 0.35, player.x + 0.35].every((x) =>
        [player.z - 0.35, player.z + 0.35].every((z) =>
          isFloor(hall.grid, Math.floor(x / CELL), Math.floor(z / CELL)),
        ),
      );
      expect(onFloor, spot).toBe(true);

      const box = curioBox(c);
      const cx = (box.x0 + box.x1) / 2;
      const cz = (box.z0 + box.z1) / 2;
      const dist = Math.hypot(player.x - cx, player.z - cz);
      expect(dist, spot).toBeLessThanOrEqual(CURIO_FAR + 1e-6);

      expect(view.pitch, spot).toBeGreaterThanOrEqual(-MAX_PITCH - 1e-9);
      const top = curioSize(c).top;
      if (Math.abs(c.h + top / 2 - EYE_HEIGHT) < 1e-9) {
        expect(view.pitch, spot).toBeCloseTo(0, 9);
      } else {
        expect(view.pitch, spot).toBeLessThan(0);
      }

      // Facing the curio: the player's yaw is exactly the angle of the
      // direction it stands away from the curio's centre (yaw 0 looks
      // north, forward = (-sin(yaw), -cos(yaw))), the same formula
      // `frameSpot` and `frameCurio` both build the spawn's yaw from.
      const expectedYaw = Math.atan2(player.x - cx, player.z - cz);
      expect(player.yaw, spot).toBeCloseTo(expectedYaw, 6);
    }
  });

  it("frames the laptop from behind with :back", () => {
    const laptop = hall.curios.find((c) => c.kind === "beige-laptop");
    if (laptop === undefined) throw new Error("no laptop in the hero hall");
    const view = spotView(hall, "prop:beige-laptop:0:back");
    expect(view).not.toBeNull();
    if (view === null) return;
    const player = spawnPlayer({ ...hall, spawn: view.spawn });
    const front = HERO_FRONT[laptop.turn] ?? [0, -1];
    const box = curioBox(laptop);
    const cx = (box.x0 + box.x1) / 2;
    const cz = (box.z0 + box.z1) / 2;
    // The player stands on the curio's -front side: its offset from the
    // curio's centre points opposite the curio's own front.
    const dx = player.x - cx;
    const dz = player.z - cz;
    expect(dx * -front[0] + dz * -front[1]).toBeGreaterThan(0);
  });

  it("keeps hero and prop spots as they were", () => {
    for (const spot of ["door:4", "hatch:0", "portal:2", "prop:crate:0"]) {
      const view = spotView(gallery, spot);
      expect(view?.spawn, spot).toEqual(spotSpawn(gallery, spot));
      expect(view?.pitch, spot).toBe(0);
    }
    const counts = new Map<string, number>();
    for (const h of hall.heroes) {
      const n = counts.get(h.kind) ?? 0;
      counts.set(h.kind, n + 1);
      const spot = `prop:${h.kind}:${String(n)}`;
      const view = spotView(hall, spot);
      expect(view?.spawn, spot).toEqual(spotSpawn(hall, spot));
      expect(view?.pitch, spot).toBe(0);
    }
  });

  it("never matches a curio kind name against the prop or hero branch first", () => {
    for (const kind of CURIO_KINDS) {
      expect((HERO_KINDS as readonly string[]).includes(kind), kind).toBe(
        false,
      );
      expect((PROP_KINDS as readonly string[]).includes(kind), kind).toBe(
        false,
      );
    }
  });
});
