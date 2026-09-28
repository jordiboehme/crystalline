/**
 * The station's seams on a generated room (M3 C28), and `roomFor`, which
 * turns a loaded address into the room the player stands in.
 */
import { describe, expect, it } from "vitest";

import { seedFor } from "../core/seed";
import {
  CANNED_BRIDGE,
  CANNED_BRIDGE_DATA,
  CANNED_DECK,
  CANNED_HUB,
  CANNED_WORKSHOP,
} from "./canned";
import { withBridge } from "./bridge";
import { curioBox } from "./curios";
import { generateDeck } from "./deck";
import { isManifestPermalink, sectionsOf } from "./folders";
import { footprint, heroFootprint, propFootprint } from "./footprints";
import { generateRoom } from "./generate";
import { deckLabel } from "./lifts";
import { LANE_WIDTH, SHEET_LANE_DEPTH, SHEET_LANE_WIDTH } from "./props";
import { edgeKey, edgeOf, overlaps } from "./sites";
import { roomFor, withExit, type StationRoomInput } from "./station";
import type { Box, PlaceInput, RoomSpec, StationAddress } from "./types";
import { CELL } from "./units";

const STATUSES = ["stable", "draft", "deprecated", "archived"] as const;

/**
 * The entrance lane `dressingSites` keeps clear, worked out here on its
 * own rather than read back from the lanes, so a lane dropped from
 * `dressingSites` shows as a prop in it: `LANE_WIDTH` across the
 * entrance cell, from the hall's middle row to the entrance wall.
 */
function entranceLane(room: RoomSpec): Box {
  const ex = (room.entrance.x + 0.5) * CELL;
  return {
    x0: ex - LANE_WIDTH / 2,
    x1: ex + LANE_WIDTH / 2,
    z0: ((room.hall.y0 + room.hall.y1) / 2) * CELL,
    z1: (room.entrance.y + 1) * CELL,
  };
}

describe("withExit (M3 C28)", () => {
  it("puts one exit on the entrance edge and leaves every other fixture as it was", () => {
    // Mutation caught: the exit on the wrong edge, inserted before other fixtures (shifting door:<i> keys), added twice, its label or its seed not the ones asked for.
    const room = generateRoom(CANNED_WORKSHOP);
    const to = {
      kind: "deck",
      domain: room.domain,
      folder: "",
      section: null,
    } as const;
    const out = withExit(room, to, "DECK 1");
    expect(out.fixtures.slice(0, -1)).toEqual(room.fixtures);
    expect(out.fixtures.at(-1)).toMatchObject({
      kind: "exit",
      slot: { ...room.entrance, side: "s" },
      label: "DECK 1",
      to,
      seed: seedFor(room.seed, "exit"),
    });
    expect(withExit(out, to, "DECK 1")).toEqual(out);
  });

  it("finds the entrance clear of props, heroes and curios in every generated room", () => {
    // Mutation caught: the entrance lane dropped from `dressingSites`, or a
    // wall prop allowed on the entrance edge. Decals are paint and are not
    // checked: the entrance floor stencil and a large hall's arrow lie in the
    // entrance lane on purpose, to be read walking in.
    const places: PlaceInput[] = [];
    for (let i = 0; i < 100; i++)
      for (const base of [CANNED_BRIDGE, CANNED_WORKSHOP, CANNED_HUB])
        places.push({
          ...base,
          permalink: `p${String(i)}`,
          status: STATUSES[i % STATUSES.length] ?? "stable",
        });
    const rooms = places.map((p) => generateRoom(p));
    expect(rooms.length).toBe(300);
    for (const room of rooms) {
      const at = `${room.domain}/${room.permalink} ${room.archetype}`;
      const entrance = edgeKey({ ...room.entrance, side: "s" });
      const lane = entranceLane(room);
      for (const p of room.props) {
        if (p.anchor === "wall")
          expect(edgeKey(edgeOf(p)), `${at} ${p.kind}`).not.toBe(entrance);
        const box = propFootprint(p);
        if (box !== null)
          expect(overlaps(box, lane), `${at} ${p.kind}`).toBe(false);
      }
      for (const h of room.heroes)
        expect(overlaps(heroFootprint(h), lane), `${at} ${h.kind}`).toBe(false);
      for (const c of room.curios)
        expect(overlaps(curioBox(c), lane), `${at} ${c.kind}`).toBe(false);
    }
  }, 30_000);
});

describe("roomFor (M3 C1, C20, C21, C28)", () => {
  it("gives an engram exactly one exit to its unresolved deck", () => {
    // Mutation caught: the exit missing or doubled, its label read from the
    // wrong folder, or its target section resolved instead of left null.
    const loaded: StationRoomInput = {
      kind: "engram",
      place: CANNED_WORKSHOP,
      folder: "crew",
    };
    const result = roomFor(loaded, null, null);
    const exits = result.room.fixtures.filter((f) => f.kind === "exit");
    expect(exits).toHaveLength(1);
    expect(exits[0]).toMatchObject({
      label: deckLabel(CANNED_WORKSHOP.domain, "crew"),
      to: {
        kind: "deck",
        domain: CANNED_WORKSHOP.domain,
        folder: "crew",
        section: null,
      },
    });
    expect(result.address).toEqual({
      kind: "engram",
      domain: CANNED_WORKSHOP.domain,
      permalink: CANNED_WORKSHOP.permalink,
    });
    expect(result.place).toBe(loaded.place);
    expect(result.spawn).toBeNull();
    expect(result.box).toBeNull();
  });

  it("resolves an unresolved deck's section to the one holding the engram the player walked up from", () => {
    // Mutation caught: the arrival's engram ignored (always the first
    // section), or an arrival from another domain honoured (matching by
    // permalink alone).
    const sections = sectionsOf(
      CANNED_DECK.rows.filter((r) => !isManifestPermalink(r.permalink)),
    );
    expect(sections.length).toBe(2);
    const secondRow = sections[1]?.[0];
    expect(secondRow).toBeDefined();
    if (secondRow === undefined) return;
    const loaded: StationRoomInput = {
      kind: "deck",
      input: CANNED_DECK,
      section: null,
    };
    const from: StationAddress = {
      kind: "engram",
      domain: CANNED_DECK.domain,
      permalink: secondRow.permalink,
    };
    const result = roomFor(loaded, { from }, null);
    expect(result.address).toEqual({
      kind: "deck",
      domain: CANNED_DECK.domain,
      folder: CANNED_DECK.folder,
      section: 1,
    });
    expect(result.room).toEqual(generateDeck(CANNED_DECK, 1));
    expect(result.place).toBeNull();
    expect(result.spawn).toBeNull();
    expect(result.box).toBeNull();

    // The same engram, named as if it lived in a different domain, is not
    // located there: the request stays unresolved and clamps to the first.
    const crossDomain = roomFor(
      loaded,
      {
        from: {
          kind: "engram",
          domain: "elsewhere",
          permalink: secondRow.permalink,
        },
      },
      null,
    );
    expect(crossDomain.address).toMatchObject({ section: 0 });
  });

  it("stands the arrival box after the bridge's own fittings, clear of the screen's lane (M3 C21)", () => {
    // Mutation caught: `landing === "box"` ignored (the box never stood up
    // at all, `box` stays null). `withBridge` first, `withArrivalBox` after
    // is still the composition built here (matching C21 and the order
    // `withArrivalBox`'s own module doc calls for), but M3 A5's task 5
    // report found this canned room's box and screen cannot be made to
    // conflict by swapping that order either way (the arrival box is
    // always free-standing, clear of any wall's viewing lane by
    // construction) - confirmed again here by hand, see the report.
    const loaded: StationRoomInput = {
      kind: "bridge",
      place: CANNED_BRIDGE,
      bridge: CANNED_BRIDGE_DATA,
    };
    const result = roomFor(loaded, null, "box");
    expect(result.box).not.toBeNull();
    if (result.box === null) return;
    const screen = result.room.fixtures.find((f) => f.kind === "screen");
    expect(screen).toBeDefined();
    const boxHero = result.room.heroes[result.box];
    expect(boxHero).toBeDefined();
    if (
      screen === undefined ||
      screen.kind !== "screen" ||
      boxHero === undefined
    )
      return;
    const lane = footprint(screen.slot, {
      along: SHEET_LANE_WIDTH,
      out: SHEET_LANE_DEPTH,
    });
    expect(overlaps(heroFootprint(boxHero), lane)).toBe(false);
    expect(result.address).toEqual({
      kind: "bridge",
      domain: CANNED_BRIDGE_DATA.domain,
    });
    expect(result.place).toBe(loaded.place);
  });

  it("leaves the bridge plain when not entered through the console room's exit", () => {
    // Mutation caught: the arrival box stood in every bridge regardless of `landing`.
    const loaded: StationRoomInput = {
      kind: "bridge",
      place: CANNED_BRIDGE,
      bridge: CANNED_BRIDGE_DATA,
    };
    const result = roomFor(loaded, null, null);
    expect(result.spawn).toBeNull();
    expect(result.box).toBeNull();
    expect(result.room).toEqual(
      withBridge(
        CANNED_BRIDGE,
        generateRoom(CANNED_BRIDGE),
        CANNED_BRIDGE_DATA,
      ),
    );
  });

  it("marks the airlock's lift stop the domain the player came from, never its own address", () => {
    // Mutation caught: `here` never set, or the airlock's own address read
    // as an origin instead of null.
    const loaded: StationRoomInput = {
      kind: "airlock",
      input: {
        domains: [
          { name: "eng", private: false },
          { name: "cargo", private: true },
        ],
        here: null,
      },
    };
    const fromBridge = roomFor(
      loaded,
      { from: { kind: "bridge", domain: "cargo" } },
      null,
    );
    const lift = fromBridge.room.fixtures.find((f) => f.kind === "lift");
    expect(
      lift?.kind === "lift"
        ? lift.stops.find(
            (s) => s.to.kind === "bridge" && s.to.domain === "cargo",
          )?.here
        : null,
    ).toBe(true);

    const fromAirlock = roomFor(loaded, { from: { kind: "airlock" } }, null);
    const lift2 = fromAirlock.room.fixtures.find((f) => f.kind === "lift");
    expect(
      lift2?.kind === "lift" ? lift2.stops.every((s) => !s.here) : null,
    ).toBe(true);
    expect(fromAirlock.address).toEqual({ kind: "airlock" });
    expect(fromAirlock.place).toBeNull();
  });
});
