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

import {
  CANNED_BRIDGE,
  CANNED_HUB,
  CANNED_WORKSHOP,
  galleryRoom,
  heroHallRoom,
} from "../world/canned";
import { curioBox, curioSize, CURIO_KINDS } from "../world/curios";
import { HERO_FRONT, heroFootprint, propFootprint } from "../world/footprints";
import { HERO_KINDS } from "../world/heroes";
import { wallFacingSpawn } from "../world/interact";
import { isFloor } from "../world/layout";
import { blockersFor, EYE_HEIGHT, MAX_PITCH, spawnPlayer } from "../world/move";
import { PROP_KINDS } from "../world/props";
import type { Box, Curio, Fixture, Hero, RoomSpec } from "../world/types";
import { CELL } from "../world/units";
import { GAME_VERSION } from "../version";
import { roomWithForcedCurio } from "./demo";
import {
  CURIO_FAR,
  SPOT_KINDS,
  circleOverlapsBox,
  curioSightClear,
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

describe("frameCurio's sight line (browser-shots review item 4)", () => {
  /**
   * A bare 10 by 10 floor, one curio at its centre and, unless overridden,
   * nothing else: enough of a `RoomSpec` for `spotView`'s curio branch,
   * `blockersFor` and `occludersFor` to run on.
   */
  function bareRoom(curios: Curio[], heroes: Hero[] = []): RoomSpec {
    return {
      version: GAME_VERSION,
      seed: 0,
      domain: "test",
      permalink: "sight-line",
      title: "Sight Line",
      archetype: "engineering",
      condition: "clean",
      width: 10,
      depth: 10,
      grid: Array.from({ length: 10 }, () => ".".repeat(10)),
      hall: { x0: 0, y0: 0, x1: 10, y1: 10 },
      bays: [],
      corridor: null,
      entrance: { x: 5, y: 9 },
      ceiling: 4,
      spawn: { x: 5, y: 9, yaw: 0 },
      fixtures: [],
      decor: [],
      scaffold: [],
      heroes,
      props: [],
      curios,
      lights: [],
      dropped: 0,
      inboundMore: 0,
    };
  }

  it("moves the spot on when a box sits on the first candidate's sight line", () => {
    // A star ball at cell (5, 5) (world 10, 10), turn 0 (front north, -z).
    const curio: Curio = {
      kind: "star-ball",
      variant: 0,
      x: 5,
      y: 5,
      h: 0.7,
      turn: 0,
      seed: 1,
    };
    // A flush hero (no movement collision, `heroBlocker` null for
    // `HERO_FOOTING["eye-panel"] === "flush"`) whose footprint still
    // stands in `occludersFor`: a 0.9 by 0.25 m panel spanning world z
    // 9.30 to 9.55, squarely between the curio (z 10) and every one of the
    // front side's candidates (z 9.2 down to 7.0, `CURIO_NEAR` to
    // `CURIO_FAR` north of it), so the whole front side is blocked and the
    // search must move to the right side instead, where nothing stands.
    const panel: Hero = {
      kind: "eye-panel",
      variant: 0,
      x: 5,
      y: 4.775,
      turn: 0,
      seed: 2,
    };
    const room = bareRoom([curio], [panel]);

    const view = spotView(room, "prop:star-ball:0");
    expect(view).not.toBeNull();
    if (view === null) return;
    const player = spawnPlayer({ ...room, spawn: view.spawn });
    // The right side's first candidate, 0.8 m east of the curio's centre.
    expect(player.x).toBeCloseTo(10.8, 6);
    expect(player.z).toBeCloseTo(10, 6);
    expect(curioSightClear(room, { x: player.x, z: player.z }, curio)).toBe(
      true,
    );

    // The front side's own first candidate (the pre-fix answer) is not
    // where the player ends up, and is indeed blocked by the panel.
    expect(curioSightClear(room, { x: 10, z: 9.2 }, curio)).toBe(false);
  });
});

describe("frameCurio's sight line on real rooms (browser-shots review item 4)", () => {
  /** The forced curio's own spot, the world point `spotView` sent the player to. */
  function forcedSpot(room: RoomSpec, kind: string) {
    const view = spotView(room, `prop:${kind}:0`);
    if (view === null) throw new Error(`no spot for ${kind}`);
    const player = spawnPlayer({ ...room, spawn: view.spawn });
    return { x: player.x, z: player.z };
  }

  it("finds a clear spot for a forced trap-box in the canned bridge", () => {
    const { room, placed } = roomWithForcedCurio(CANNED_BRIDGE, "trap-box");
    expect(placed).toBe("trap-box");
    const c = room.curios.find((x) => x.kind === "trap-box");
    if (c === undefined) throw new Error("no trap-box placed");
    expect(curioSightClear(room, forcedSpot(room, "trap-box"), c)).toBe(true);
  });

  it("finds a clear spot for a forced trap-box in the canned workshop", () => {
    const { room, placed } = roomWithForcedCurio(CANNED_WORKSHOP, "trap-box");
    expect(placed).toBe("trap-box");
    const c = room.curios.find((x) => x.kind === "trap-box");
    if (c === undefined) throw new Error("no trap-box placed");
    expect(curioSightClear(room, forcedSpot(room, "trap-box"), c)).toBe(true);
  });

  it("finds a clear spot for a forced fuel-case in the hub", () => {
    const { room, placed } = roomWithForcedCurio(CANNED_HUB, "fuel-case");
    expect(placed).toBe("fuel-case");
    const c = room.curios.find((x) => x.kind === "fuel-case");
    if (c === undefined) throw new Error("no fuel-case placed");
    expect(curioSightClear(room, forcedSpot(room, "fuel-case"), c)).toBe(true);
  });
});
