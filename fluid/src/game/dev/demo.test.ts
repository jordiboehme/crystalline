/**
 * `roomWithForcedHero`: the `?hero=` dev switch's engine. It forces the
 * hero pass to try one kind before anything else and never throws, whether
 * or not that kind finds a place in the room it is handed.
 */

import { describe, expect, it } from "vitest";

import { CANNED_BRIDGE, CANNED_WORKSHOP } from "../world/canned";
import { CURIO_KINDS } from "../world/curios";
import { heroBlocker, propFootprint } from "../world/footprints";
import { generateRoom } from "../world/generate";
import { overlaps } from "../world/sites";
import { roomWithForcedCurio, roomWithForcedHero } from "./demo";

describe("roomWithForcedHero", () => {
  it("places the turret in a workshop, reporting it placed", () => {
    const { room, placed } = roomWithForcedHero(CANNED_WORKSHOP, "turret");
    expect(placed).toBe("turret");
    expect(room.heroes).toHaveLength(1);
    expect(room.heroes[0]?.kind).toBe("turret");
  });

  it("places a pool kind of the room's own archetype", () => {
    // The canned bridge's archetype (bridge) pools the eye panel.
    const { room, placed } = roomWithForcedHero(CANNED_BRIDGE, "eye-panel");
    expect(placed).toBe("eye-panel");
    expect(room.heroes).toHaveLength(1);
    expect(room.heroes[0]?.kind).toBe("eye-panel");
  });

  it("forces a kind foreign to the room's own archetype, drawn from the pool that holds it", () => {
    // The mess table is council only; the workshop's archetype is
    // engineering. It still stands, since the ruling forces it into the
    // room shown, not only into a room of its own kind of archetype.
    const { room, placed } = roomWithForcedHero(CANNED_WORKSHOP, "mess-table");
    expect(placed).toBe("mess-table");
    expect(room.heroes).toHaveLength(1);
    expect(room.heroes[0]?.kind).toBe("mess-table");
  });

  it("falls back to no hero, without throwing, when the room's hall has no place for it", () => {
    // The canned bridge's hall is too small (7x6) for the mess table (5x2.6)
    // in its interior band, whatever archetype it is forced under.
    expect(() => roomWithForcedHero(CANNED_BRIDGE, "mess-table")).not.toThrow();
    const { room, placed } = roomWithForcedHero(CANNED_BRIDGE, "mess-table");
    expect(placed).toBeNull();
    expect(room.heroes.some((h) => h.kind === "mess-table")).toBe(false);
  });

  it("forces every new kind into the canned workshop", () => {
    // Review Focus 5. Mutation caught: a forced any-archetype kind not
    // reaching the free-slot draw, or the block not forced.
    for (const kind of [
      "stone-hand",
      "question-block",
      "mech-head",
      "red-bike",
      "hoverboard",
      "flying-cloud",
      "spider-tank",
      "garden-robot",
      "moon-rocket",
      "thunder-hammer",
      "police-box",
    ] as const) {
      const { room, placed } = roomWithForcedHero(CANNED_WORKSHOP, kind);
      expect(placed, kind).toBe(kind);
      expect(
        room.heroes.map((h) => h.kind),
        kind,
      ).toEqual([kind]);
    }
  });

  it("dresses the room's props clear of the forced hero's blocker", () => {
    const { room } = roomWithForcedHero(CANNED_WORKSHOP, "turret");
    const hero = room.heroes[0];
    if (hero === undefined) throw new Error("no hero placed");
    const box = heroBlocker(hero);
    if (box === null) throw new Error("the turret collides");
    for (const p of room.props) {
      const propBox = propFootprint(p);
      if (propBox === null) continue;
      expect(overlaps(propBox, box), p.kind).toBe(false);
    }
  });
});

describe("roomWithForcedCurio", () => {
  // The workshop's terminals have desk ends, but they are too narrow for
  // the laptop (C8), and its default archetype and condition (engineering,
  // construction) draws no table, no bench, no workbench, no hydroponics
  // trough and no hero with an under spot, so the laptop has no host
  // here. Its `engineering` dressing does draw a service trolley, though,
  // whose deck top hosts both under-desk kinds now.
  const NO_HOST_IN_WORKSHOP: readonly string[] = ["beige-laptop"];

  it("forces every curio kind into the workshop where it has a host, and leaves heroes and props alone", () => {
    const base = generateRoom(CANNED_WORKSHOP);
    for (const kind of CURIO_KINDS) {
      const { room, placed } = roomWithForcedCurio(CANNED_WORKSHOP, kind);
      expect(room.heroes, kind).toEqual(base.heroes);
      expect(room.props, kind).toEqual(base.props);
      if (NO_HOST_IN_WORKSHOP.includes(kind)) {
        expect(placed, kind).toBeNull();
        expect(
          room.curios.some((c) => c.kind === kind),
          kind,
        ).toBe(false);
      } else {
        expect(placed, kind).toBe(kind);
        expect(
          room.curios.some((c) => c.kind === kind),
          kind,
        ).toBe(true);
      }
    }
  });
});
