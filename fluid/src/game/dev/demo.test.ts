/**
 * `roomWithForcedHero`: the `?hero=` dev switch's engine. It forces the
 * hero pass to try one kind before anything else and never throws, whether
 * or not that kind finds a place in the room it is handed.
 */

import { describe, expect, it } from "vitest";

import { CANNED_BRIDGE, CANNED_WORKSHOP } from "../world/canned";
import {
  CURIO_KINDS,
  curioDrawsOf,
  placeCurios,
  rawCurios,
} from "../world/curios";
import { heroBlocker, propFootprint } from "../world/footprints";
import { generateRoom, nearFor, roomSeed } from "../world/generate";
import { RARE_PROP_KINDS } from "../world/props";
import { overlaps } from "../world/sites";
import type { PlaceInput } from "../world/types";
import {
  roomWithForcedCurio,
  roomWithForcedHero,
  roomWithForcedProp,
} from "./demo";

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

  it("forces every 2.6c and 2.6d hero into the canned workshop", () => {
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
      "slab-walker",
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
  // whose deck top hosts both under-desk kinds now. The two computers are
  // as wide as the laptop, so they find no host either, and the soot puffs
  // stand only at floor level, which the trolley's deck is not.
  const NO_HOST_IN_WORKSHOP: readonly string[] = [
    "beige-laptop",
    "breadbin-computer",
    "slim-computer",
    "soot-puffs",
  ];

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

describe("roomWithForcedProp", () => {
  it("forces every rare kind into the canned workshop, and leaves its heroes alone (2.6d C20, Review Focus 5)", () => {
    // Mutation caught: a forced draw that never reaches the rare step, or
    // the mark forced without its take.
    const base = generateRoom(CANNED_WORKSHOP);
    for (const kind of RARE_PROP_KINDS) {
      const { room, placed } = roomWithForcedProp(CANNED_WORKSHOP, kind);
      expect(placed, kind).toBe(kind);
      expect(
        room.props.filter((p) => p.kind === kind),
        kind,
      ).toHaveLength(1);
      expect(room.heroes, kind).toEqual(base.heroes);
    }
  });

  it("falls back to the room without it, never throwing, where the kind has no place", () => {
    // The canned bridge's cells beside its two terminals are taken (Baselines).
    expect(() =>
      roomWithForcedProp(CANNED_BRIDGE, "designer-tower"),
    ).not.toThrow();
    expect(
      roomWithForcedProp(CANNED_BRIDGE, "designer-tower").placed,
    ).toBeNull();
  });
});

describe("the neighbours in the seams (2.6f C9)", () => {
  it("lands a forced kind that a neighbour draws, and keeps the room's own skip in the seams (Review Focus 5)", () => {
    // boiler-room draws the hoverboard raw (2.6f baselines), so the
    // workshop's own pick skips it; the forced seam must not. Mutation
    // caught: a forced draw reading near, or a seam placing the curios
    // again without the room's near.
    expect(nearFor(CANNED_WORKSHOP).heroes.has("hoverboard")).toBe(true);
    const hover = roomWithForcedHero(CANNED_WORKSHOP, "hoverboard");
    expect(hover.placed).toBe("hoverboard");
    // `placed` reads only that a hero landed, so the kind is checked too.
    expect(hover.room.heroes.map((h) => h.kind)).toEqual(["hoverboard"]);
    // A lower-seeded hatch whose room draws the radar raw.
    const mine = roomSeed("station", "pipe-shop");
    let below: string | null = null;
    for (let i = 0; i < 50000 && below === null; i++) {
      const p = `radar-${String(i)}`;
      const s = roomSeed("station", p);
      if (
        s < mine &&
        rawCurios(curioDrawsOf(s), null).includes("treasure-radar")
      )
        below = p;
    }
    if (below === null) throw new Error("no lower radar neighbour in 50000");
    const place: PlaceInput = {
      ...CANNED_WORKSHOP,
      inbound: [
        ...CANNED_WORKSHOP.inbound,
        {
          address: { domain: "station", permalink: below },
          title: below,
          relType: "links_to",
        },
      ],
      inboundTotal: CANNED_WORKSHOP.inboundTotal + 1,
    };
    expect(nearFor(place).curiosBelow.has("treasure-radar")).toBe(true);
    const built = generateRoom(place);
    {
      // Without its neighbours the same room places a radar, so the line
      // below is not vacuous (the extra hatch could have moved its host).
      const { curios, ...rest } = built;
      void curios;
      expect(placeCurios(rest).map((c) => c.kind)).toContain("treasure-radar");
    }
    expect(built.curios.map((c) => c.kind)).not.toContain("treasure-radar");
    expect(roomWithForcedCurio(place, "treasure-radar").placed).toBe(
      "treasure-radar",
    );
    const { room } = roomWithForcedHero(place, "turret");
    const { curios, ...rest } = room;
    void curios;
    expect(placeCurios(rest).map((c) => c.kind)).toContain("treasure-radar");
    expect(room.curios.map((c) => c.kind)).not.toContain("treasure-radar");
    // The forced-prop seam places the curios again the same way.
    const kind = RARE_PROP_KINDS[0];
    if (kind === undefined) throw new Error("no rare prop kind");
    const withProp = roomWithForcedProp(place, kind);
    expect(withProp.placed).toBe(kind);
    {
      const { curios: own, ...bare } = withProp.room;
      void own;
      expect(placeCurios(bare).map((c) => c.kind)).toContain("treasure-radar");
    }
    expect(withProp.room.curios.map((c) => c.kind)).not.toContain(
      "treasure-radar",
    );
  });
});
