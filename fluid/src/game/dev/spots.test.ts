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
import {
  curioBox,
  curioOn,
  curioSize,
  CURIO_KINDS,
  DECOR_SURFACES,
  FIXTURE_SURFACES,
  PROP_SURFACES,
  hostSurfaces,
} from "../world/curios";
import { HERO_FRONT, heroFootprint, propFootprint } from "../world/footprints";
import { generateRoom } from "../world/generate";
import { wallFacingSpawn } from "../world/interact";
import { isFloor } from "../world/layout";
import {
  blockersFor,
  EYE_HEIGHT,
  MAX_PITCH,
  PLAYER_RADIUS,
  spawnPlayer,
} from "../world/move";
import { wallAnchor } from "../world/sites";
import type {
  Box,
  Curio,
  Fixture,
  Hero,
  Prop,
  RoomSpec,
  SurfaceSpec,
} from "../world/types";
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
    // Spawns captured at 963097ca, before the curio branch existed: a
    // fixture, two props and two heroes, each with pitch 0.
    const pinned = [
      [gallery, "door:4", { x: 9, y: 0, yaw: 0 }],
      [gallery, "machine:1", { x: 21, y: 0.225, yaw: 0 }],
      [gallery, "prop:crate:0", { x: 37, y: -0.25, yaw: Math.PI }],
      [gallery, "prop:storage-shelf:1", { x: 37, y: 3.75, yaw: Math.PI }],
      [hall, "prop:tube-bench:0", { x: 16, y: 1.4125, yaw: 0 }],
      [hall, "prop:arcade-cabinet:0", { x: 1.0375, y: 6, yaw: Math.PI / 2 }],
    ] as const;
    for (const [room, spot, want] of pinned) {
      const view = spotView(room, spot);
      if (view === null) throw new Error(`no spot for ${spot}`);
      expect(view.spawn.x, spot).toBeCloseTo(want.x, 9);
      expect(view.spawn.y, spot).toBeCloseTo(want.y, 9);
      expect(view.spawn.yaw, spot).toBeCloseTo(want.yaw, 9);
      expect(view.pitch, spot).toBe(0);
    }
  });

  it("reads a curio kind through the curio branch in a room with heroes and props", () => {
    // The hero hall holds every curio kind and every hero; a crate makes
    // it hold a prop as well. Every curio kind resolves to a close,
    // tilted framing (pitch below 0), which only the curio branch gives:
    // the hero and prop branches always give pitch 0.
    const crate: Prop = {
      kind: "crate",
      variant: 0,
      anchor: "floor",
      x: 11.5,
      y: 12.5,
      turn: 0,
      seed: 1,
    };
    const room: RoomSpec = { ...hall, props: [crate] };
    expect(spotView(room, "prop:crate:0")?.pitch).toBe(0);
    for (const kind of CURIO_KINDS) {
      const view = spotView(room, `prop:${kind}:0`);
      expect(view, kind).not.toBeNull();
      expect(view?.pitch ?? 0, kind).toBeLessThan(0);
    }
  });
});

describe("frameCurio's sight line", () => {
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

    // The front side's own first candidate, which a search with no sight
    // line check would pick, is not where the player ends up, and is
    // indeed blocked by the panel.
    expect(curioSightClear(room, { x: 10, z: 9.2 }, curio)).toBe(false);
  });

  it("moves the spot on when another curio stands on the first candidate's sight line", () => {
    // A star ball at cell (5, 5) (world 10, 10), turn 0 (front north, -z).
    const ball: Curio = {
      kind: "star-ball",
      variant: 0,
      x: 5,
      y: 5,
      h: 0.7,
      turn: 0,
      seed: 1,
    };
    // An upright lit sword 0.3 m north of it at the same height: its 0.14
    // m box runs from 0.7 up to 1.92, over the eye, so every front
    // candidate (straight north of the ball) looks through it.
    const sword: Curio = {
      kind: "light-sword",
      variant: 1,
      x: 5,
      y: 4.85,
      h: 0.7,
      turn: 0,
      seed: 2,
    };

    const alone = bareRoom([ball]);
    const first = spotView(alone, "prop:star-ball:0");
    if (first === null) throw new Error("no spot for the ball alone");
    const p0 = spawnPlayer({ ...alone, spawn: first.spawn });
    expect(p0.x).toBeCloseTo(10, 6);
    expect(p0.z).toBeCloseTo(9.2, 6);

    const room = bareRoom([ball, sword]);
    expect(curioSightClear(room, { x: 10, z: 9.2 }, ball)).toBe(false);
    const view = spotView(room, "prop:star-ball:0");
    expect(view).not.toBeNull();
    if (view === null) return;
    const player = spawnPlayer({ ...room, spawn: view.spawn });
    // The right side's first candidate, 0.8 m east of the ball.
    expect(player.x).toBeCloseTo(10.8, 6);
    expect(player.z).toBeCloseTo(10, 6);
    expect(curioSightClear(room, player, ball)).toBe(true);
  });

  it("hides a curio under its host's top from a line through the top, and frames it from the first spot that sees under the edge", () => {
    // A gun bench backed against a north wall edge (its front is +z), with
    // the fuel case on its under spot: a shelf at h 0.31 with 0.52 of free
    // height, so the host rule (`occludersFor`) stands the bench's whole
    // 1.9 by 0.9 m footprint up from 0.83 to the eye, so the bench's own
    // top hides the case from a line through it.
    const a = wallAnchor({ x: 5, y: 2, side: "n" });
    const bench: Hero = {
      kind: "gun-bench",
      variant: 0,
      x: a.x,
      y: a.y,
      turn: a.turn,
      seed: 3,
    };
    const under = hostSurfaces(bareRoom([], [bench])).find(
      (s) => s.key[2] === "hero-gun-bench-under-0",
    );
    if (under === undefined) throw new Error("no gun-bench under spot");
    const c = curioOn(under, "fuel-case", 0, 0.5, 0.5, 4);
    const room = bareRoom([c], [bench]);
    const box = curioBox(c);
    const cx = (box.x0 + box.x1) / 2;
    const cz = (box.z0 + box.z1) / 2;
    const [fx, fz] = HERO_FRONT[a.turn] ?? [0, -1];
    const out = (d: number) => ({ x: cx + fx * d, z: cz + fz * d });

    // The case's middle is 0.31 + 0.24 / 2 = 0.43 m up, 0.45 m in from the
    // bench's front edge (d 0.45 of 0.9). A line from the eye (1.6) passes
    // under the edge at 0.83 only when 0.43 + 1.17 * 0.45 / dist < 0.83,
    // that is dist > 1.316 m. Every front candidate from `CURIO_NEAR` (0.8,
    // the first one clear of the bench's blocker) to 1.3 looks through
    // the top; 1.4 is the first that sees under it.
    expect(curioSightClear(room, out(0.8), c)).toBe(false);
    expect(curioSightClear(room, out(1.3), c)).toBe(false);
    expect(curioSightClear(room, out(1.4), c)).toBe(true);

    const view = spotView(room, "prop:fuel-case:0");
    expect(view).not.toBeNull();
    if (view === null) return;
    const player = spawnPlayer({ ...room, spawn: view.spawn });
    expect(player.x).toBeCloseTo(out(1.4).x, 6);
    expect(player.z).toBeCloseTo(out(1.4).z, 6);
  });
});

describe("frameCurio's sight line in the hero hall and the gallery", () => {
  /**
   * Every curio of `room` with its within-kind ordinal, the `n` of
   * `?at=prop:<kind>:<n>`: the gallery holds two lit swords.
   */
  function withOrdinals(room: RoomSpec) {
    const counts = new Map<string, number>();
    return room.curios.map((c) => {
      const n = counts.get(c.kind) ?? 0;
      counts.set(c.kind, n + 1);
      return { c, spot: `prop:${c.kind}:${String(n)}` };
    });
  }

  it("finds exactly one host surface under every curio, the one the host rule reads", () => {
    for (const room of [heroHallRoom(), galleryRoom()]) {
      const surfaces = hostSurfaces(room);
      for (const { c, spot } of withOrdinals(room)) {
        const box = curioBox(c);
        const matches = surfaces.filter(
          (s) =>
            Math.abs(s.h - c.h) < 1e-6 &&
            box.x0 >= s.box.x0 - 1e-6 &&
            box.x1 <= s.box.x1 + 1e-6 &&
            box.z0 >= s.box.z0 - 1e-6 &&
            box.z1 <= s.box.z1 + 1e-6,
        );
        expect(matches.length, `${room.permalink} ${spot}`).toBe(1);
      }
    }
  });

  /**
   * `row` (`world/canned.ts`) lays a hero surface's curios along the host's
   * local `a` axis, so no curio of the hall stands behind another on the
   * only line a player can stand, and every curio of both rooms has a
   * clear spot.
   */
  it("frames every curio of both rooms from a spot with a clear sight line, other curios included", () => {
    for (const room of [heroHallRoom(), galleryRoom()]) {
      for (const { c, spot } of withOrdinals(room)) {
        const name = `${room.permalink} ${spot}`;
        const view = spotView(room, spot);
        expect(view, name).not.toBeNull();
        if (view === null) continue;
        const player = spawnPlayer({ ...room, spawn: view.spawn });
        expect(
          curioSightClear(room, { x: player.x, z: player.z }, c),
          name,
        ).toBe(true);
      }
    }
  });

  it("frames the hero hall's fuel case and trap from past the edge of their hosts' tops", () => {
    // Pinned apart from the sweep above, since these two are the ones the
    // host rule moves: both stand under a hero's top, which hides them
    // from every spot closer to the host. The distances are the first
    // `CURIO_STEP` past each top's edge line (the fuel case's is derived
    // in the gun-bench case above).
    const hall = heroHallRoom();
    for (const [kind, dist] of [
      ["fuel-case", 1.4],
      ["trap-box", 1.4],
    ] as const) {
      const c = hall.curios.find((x) => x.kind === kind);
      if (c === undefined) throw new Error(`no ${kind} in the hero hall`);
      const view = spotView(hall, `prop:${kind}:0`);
      if (view === null) throw new Error(`no spot for ${kind}`);
      const player = spawnPlayer({ ...hall, spawn: view.spawn });
      const box = curioBox(c);
      const got = Math.hypot(
        player.x - (box.x0 + box.x1) / 2,
        player.z - (box.z0 + box.z1) / 2,
      );
      expect(got, kind).toBeCloseTo(dist, 6);
      expect(curioSightClear(hall, player, c), kind).toBe(true);
    }
  });
});

describe("frameCurio's sight line on real rooms", () => {
  /** The forced curio's own spot, the world point `spotView` sent the player to. */
  function forcedSpot(room: RoomSpec, kind: string) {
    const view = spotView(room, `prop:${kind}:0`);
    if (view === null) throw new Error(`no spot for ${kind}`);
    const player = spawnPlayer({ ...room, spawn: view.spawn });
    return { x: player.x, z: player.z };
  }

  it("finds a clear spot for a forced green-pistol on the workshop's guide terminal", () => {
    const place = { ...CANNED_WORKSHOP, type: "guide" };
    const { room, placed } = roomWithForcedCurio(place, "green-pistol");
    expect(placed).toBe("green-pistol");
    const c = room.curios.find((x) => x.kind === "green-pistol");
    if (c === undefined) throw new Error("no green-pistol placed");
    expect(curioSightClear(room, forcedSpot(room, "green-pistol"), c)).toBe(
      true,
    );
  });

  it("finds a clear spot for a forced fuel-case in the hub", () => {
    const { room, placed } = roomWithForcedCurio(CANNED_HUB, "fuel-case");
    expect(placed).toBe("fuel-case");
    const c = room.curios.find((x) => x.kind === "fuel-case");
    if (c === undefined) throw new Error("no fuel-case placed");
    expect(curioSightClear(room, forcedSpot(room, "fuel-case"), c)).toBe(true);
  });

  // A terminal carries no under spot (`world/curios.ts`'s
  // `FIXTURE_SURFACES.terminal`): its knee space holds the swivel chair,
  // so no standing player could ever see past it. The bridge and the
  // canned workshop, at their own default archetype and condition, draw no
  // other host with an under spot, so a forced under-desk kind finds no
  // host in these rooms: the slot stays empty and the room is drawn
  // without it, exactly the "room with no host for the drawn kind" case
  // (`placeCurios` never throws on it).

  it("draws no host at all for a forced trap-box in the canned bridge", () => {
    const { room, placed } = roomWithForcedCurio(CANNED_BRIDGE, "trap-box");
    expect(placed).toBeNull();
    expect(room.curios.some((c) => c.kind === "trap-box")).toBe(false);
  });

  it("draws no host at all for a forced trap-box in the canned workshop", () => {
    const { room, placed } = roomWithForcedCurio(CANNED_WORKSHOP, "trap-box");
    expect(placed).toBeNull();
    expect(room.curios.some((c) => c.kind === "trap-box")).toBe(false);
  });

  it("draws no host at all for a forced fuel-case in the canned workshop", () => {
    const { room, placed } = roomWithForcedCurio(CANNED_WORKSHOP, "fuel-case");
    expect(placed).toBeNull();
    expect(room.curios.some((c) => c.kind === "fuel-case")).toBe(false);
  });
});

describe("every catalogued under spot seen from a standing player", () => {
  /**
   * The sight-line check for the under spots of the fixture (terminal and
   * machine), decor and floor prop tables (`world/curios.ts`): each spot, found in generated
   * rooms, takes the trap and the case at its centre and its four corners
   * wherever `curioOn` fits them, and some spot a player can stand on (the
   * circle on floor and clear of every blocker, on a 0.25 m grid within
   * 4 m) must see the curio's middle past every occluder
   * (`curioSightClear`). A host whose legs stand at its `a` ends (a bench,
   * a workbench, the hydroponics trough) has them in no occluder, so its
   * spots must be seen from its open front or back: the standing spot's
   * offset from the curio runs more along the host's `d` than its `a`.
   * The round table is open all round.
   */
  const OPEN_ALL_ROUND: ReadonlySet<string> = new Set(["decor:round-table"]);
  const ARCHETYPE_TYPES = [
    "manifest",
    "decision",
    "runbook",
    "reference",
    "guide",
  ] as const;

  /** Every under spot the tables catalogue, as `<host> v<variant> <j>`. */
  function catalogued(): Set<string> {
    const out = new Set<string>();
    const terminal: readonly SurfaceSpec[] = FIXTURE_SURFACES.terminal;
    for (const [j, s] of terminal.entries())
      if (s.cls === "under") out.add(`terminal v0 ${String(j)}`);
    const machines: Partial<Record<string, readonly SurfaceSpec[]>> =
      FIXTURE_SURFACES.machine;
    for (const [kind, specs] of Object.entries(machines))
      for (const [j, s] of (specs ?? []).entries())
        if (s.cls === "under") out.add(`machine:${kind} v0 ${String(j)}`);
    const decor: Partial<Record<string, readonly SurfaceSpec[]>> =
      DECOR_SURFACES;
    for (const [kind, specs] of Object.entries(decor))
      for (const [j, s] of (specs ?? []).entries())
        if (s.cls === "under") out.add(`decor:${kind} v0 ${String(j)}`);
    const props: Partial<Record<string, readonly (readonly SurfaceSpec[])[]>> =
      PROP_SURFACES;
    for (const [kind, variants] of Object.entries(props))
      for (const [v, specs] of (variants ?? []).entries())
        for (const [j, s] of specs.entries())
          if (s.cls === "under")
            out.add(`prop:${kind} v${String(v)} ${String(j)}`);
    return out;
  }

  /** The standable points within 4 m of `(cx, cz)`, world metres. */
  function standable(room: RoomSpec, cx: number, cz: number) {
    const blockers = blockersFor(room);
    const out: { x: number; z: number }[] = [];
    for (let ix = -16; ix <= 16; ix++)
      for (let iz = -16; iz <= 16; iz++) {
        const x = cx + ix * 0.25;
        const z = cz + iz * 0.25;
        const onFloor = [x - PLAYER_RADIUS, x + PLAYER_RADIUS].every((px) =>
          [z - PLAYER_RADIUS, z + PLAYER_RADIUS].every((pz) =>
            isFloor(room.grid, Math.floor(px / CELL), Math.floor(pz / CELL)),
          ),
        );
        if (onFloor && !blockers.some((b) => circleOverlapsBox(x, z, b)))
          out.push({ x, z });
      }
    return out;
  }

  it("sees every trap and case on every under spot of the host tables from a spot on the host's open side", () => {
    const want = catalogued();
    expect(want.size).toBeGreaterThan(0);
    const checked = new Map<string, number>();
    for (let i = 0; i < 60; i++) {
      const place = [CANNED_WORKSHOP, CANNED_BRIDGE, CANNED_HUB][i % 3];
      if (place === undefined) throw new Error("places");
      const tags = Array.from(
        { length: i % 6 },
        (_, j) => `sight-${String(i)}-${String(j)}`,
      );
      for (const type of ARCHETYPE_TYPES) {
        const generated = generateRoom({
          ...place,
          permalink: `under-sight-${String(i)}-${type}`,
          tags,
          type,
        });
        const room: RoomSpec = { ...generated, curios: [] };
        for (const s of hostSurfaces(room)) {
          if (s.cls !== "under" || s.host.startsWith("hero:")) continue;
          const variant =
            "variant" in s.anchorOf && typeof s.anchorOf.variant === "number"
              ? s.anchorOf.variant
              : 0;
          const j = s.key[2].split("-").pop() ?? "";
          const key = `${s.host} v${String(variant)} ${j}`;
          if ((checked.get(key) ?? 0) >= 2) continue;
          let fitted = 0;
          for (const kind of ["trap-box", "fuel-case"] as const)
            for (const [u, v] of [
              [0.5, 0.5],
              [0, 0],
              [1, 1],
              [0, 1],
              [1, 0],
            ] as const) {
              let c: Curio;
              try {
                c = curioOn(s, kind, 0, u, v, 1);
              } catch {
                continue;
              }
              fitted++;
              const withCurio: RoomSpec = { ...room, curios: [c] };
              const box = curioBox(c);
              const cx = (box.x0 + box.x1) / 2;
              const cz = (box.z0 + box.z1) / 2;
              const [fx, fz] = HERO_FRONT[s.turn] ?? [0, -1];
              const seen = standable(withCurio, cx, cz).some((p) => {
                const along = Math.abs(fz * (p.x - cx) - fx * (p.z - cz));
                const out = Math.abs(fx * (p.x - cx) + fz * (p.z - cz));
                if (!OPEN_ALL_ROUND.has(s.host) && out < along) return false;
                return curioSightClear(withCurio, p, c);
              });
              expect(
                seen,
                `${generated.permalink} ${key} ${kind} at ${String(u)},${String(v)}`,
              ).toBe(true);
            }
          expect(fitted, `${generated.permalink} ${key}`).toBeGreaterThan(0);
          checked.set(key, (checked.get(key) ?? 0) + 1);
        }
      }
    }
    expect([...checked.keys()].sort()).toEqual([...want].sort());
  });
});
