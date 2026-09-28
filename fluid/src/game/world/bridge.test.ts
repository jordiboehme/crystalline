/**
 * The bridge's own fittings (M3 C20, C21): `withBridge` and the wall edge
 * its screen goes on.
 */
import { describe, expect, it } from "vitest";

import { seedFor } from "../core/seed";
import { bridgeScreenEdge, withBridge, type BridgeInput } from "./bridge";
import { withArrivalBox } from "./arrival";
import { CANNED_BRIDGE, CANNED_BRIDGE_DATA } from "./canned";
import { curioBox } from "./curios";
import { footprint, heroFootprint, propFootprint } from "./footprints";
import { generateRoom } from "./generate";
import bridgeFittedGolden from "./golden/bridge-fitted.json?raw";
import { LIFT_WORDS, deckLabel, engramCount } from "./lifts";
import {
  LANE_DEPTH,
  LANE_WIDTH,
  SHEET_LANE_DEPTH,
  SHEET_LANE_WIDTH,
} from "./props";
import { dressingSites, edgeKey, edgeOf, inside, overlaps } from "./sites";
import type { Fixture, PlaceInput, RoomSpec } from "./types";

const STATUSES = ["stable", "draft", "deprecated", "archived"] as const;

/** The screen's viewing lane size, as `dressingSites` gives a screen. */
const SCREEN_LANE = { along: SHEET_LANE_WIDTH, out: SHEET_LANE_DEPTH };

/** The lift's and the exit's way lane size, as `dressingSites` gives them. */
const WAY_LANE = { along: LANE_WIDTH, out: LANE_DEPTH };

describe("withBridge (M3 C20, C21)", () => {
  it("puts the lift on the entrance edge and the screen on the north wall nearest the centre", () => {
    // Mutation caught: the screen on a fixture's edge, the key on a public domain.
    const room = generateRoom(CANNED_BRIDGE);
    const fitted = withBridge(CANNED_BRIDGE, room, CANNED_BRIDGE_DATA);
    expect(fitted.fixtures.slice(0, -2)).toEqual(room.fixtures);
    const lift = fitted.fixtures.at(-2);
    const screen = fitted.fixtures.at(-1);
    expect(lift).toMatchObject({
      kind: "lift",
      slot: { ...room.entrance, side: "s" },
      note: null,
      seed: seedFor(room.seed, "lift"),
    });
    expect(lift?.kind === "lift" ? lift.stops.map((s) => s.label) : []).toEqual(
      [
        LIFT_WORDS.airlock,
        deckLabel("station", ""),
        deckLabel("station", "engineering"),
        deckLabel("station", "logs"),
      ],
    );
    expect(screen).toMatchObject({
      kind: "screen",
      slot: bridgeScreenEdge(room),
      lines: ["station", engramCount(5)],
      keys: [],
      seed: seedFor(room.seed, "screen"),
    });
    // The screen's edge is free of every other fixture's slot.
    for (const f of room.fixtures)
      expect(screen?.kind === "screen" ? screen.slot : null).not.toEqual(
        f.slot,
      );
    expect(fitted.decor).toEqual(room.decor);
    expect(fitted.scaffold).toEqual(room.scaffold);
    expect(fitted.heroes).toEqual(room.heroes);
    expect(fitted.finish).toEqual(room.finish);
    expect(fitted.lights).toEqual(room.lights);

    // A private domain draws the key on the name (index 0).
    const priv: BridgeInput = { ...CANNED_BRIDGE_DATA, private: true };
    const privFitted = withBridge(CANNED_BRIDGE, room, priv);
    expect(privFitted.fixtures.at(-1)).toMatchObject({ keys: [0] });
  });

  it("leaves a room with no bridge data as it was", () => {
    // Mutation caught: a re-dress run with no data.
    const room = generateRoom(CANNED_BRIDGE);
    expect(withBridge(CANNED_BRIDGE, room, null)).toBe(room);
  });

  it("puts no screen where no wall edge is free", () => {
    // Mutation caught: a screen forced onto a taken edge.
    const room = generateRoom(CANNED_BRIDGE);
    const sites = dressingSites(room);
    const filled: Fixture[] = [...room.fixtures];
    let i = 0;
    for (const run of sites.runs)
      for (const edge of run) {
        if (edge.side === "s") continue;
        if (!inside(room.hall, edge.x, edge.y)) continue;
        const key = edgeKey(edge);
        if (!sites.free.has(key)) continue;
        filled.push({
          kind: "poster",
          slot: edge,
          category: `filler-${String(i)}`,
          lines: [],
          seed: seedFor(room.seed, "filler", i),
        });
        i++;
      }
    expect(i).toBeGreaterThan(0);
    const packed: RoomSpec = { ...room, fixtures: filled };
    expect(bridgeScreenEdge(packed)).toBeNull();
    const fitted = withBridge(CANNED_BRIDGE, packed, CANNED_BRIDGE_DATA);
    expect(fitted.fixtures.some((f) => f.kind === "screen")).toBe(false);
    expect(fitted.fixtures.some((f) => f.kind === "lift")).toBe(true);
  });

  it("marks a failed tree and an empty domain on the panel", () => {
    // Mutation caught: the two notes swapped, stops beyond AIRLOCK on a failed tree.
    const room = generateRoom(CANNED_BRIDGE);
    const failed = withBridge(CANNED_BRIDGE, room, {
      ...CANNED_BRIDGE_DATA,
      folders: null,
    });
    const failedLift = failed.fixtures.find((f) => f.kind === "lift");
    expect(failedLift?.kind === "lift" ? failedLift.note : null).toBe(
      LIFT_WORDS.deckError,
    );
    expect(
      failedLift?.kind === "lift" ? failedLift.stops.map((s) => s.label) : [],
    ).toEqual([LIFT_WORDS.airlock]);

    const empty = withBridge(CANNED_BRIDGE, room, {
      ...CANNED_BRIDGE_DATA,
      folders: [],
      rootDeck: false,
    });
    const emptyLift = empty.fixtures.find((f) => f.kind === "lift");
    expect(emptyLift?.kind === "lift" ? emptyLift.note : null).toBe(
      LIFT_WORDS.noDecks,
    );
    expect(
      emptyLift?.kind === "lift" ? emptyLift.stops.map((s) => s.label) : [],
    ).toEqual([LIFT_WORDS.airlock]);
  });

  it("keeps the screen's lane clear and composes with the arrival box (Review Focus 5)", () => {
    // Mutation caught: the re-dress skipped (a prop stays in the new
    // lane), withArrivalBox applied before withBridge (the box ignores
    // the screen).
    const places: PlaceInput[] = Array.from({ length: 200 }, (_, i) => ({
      ...CANNED_BRIDGE,
      permalink: `m${String(i)}`,
      status: STATUSES[i % STATUSES.length] ?? "stable",
      salience: i === 0 ? 0 : CANNED_BRIDGE.salience,
    }));
    expect(places.length).toBe(200);
    const fitted = places.map((p) =>
      withBridge(p, generateRoom(p), CANNED_BRIDGE_DATA),
    );
    expect(fitted.length).toBeGreaterThan(0);
    const withScreens = fitted.filter((r) =>
      r.fixtures.some((f) => f.kind === "screen"),
    );
    expect(withScreens.length).toBeGreaterThanOrEqual(150);
    for (const room of withScreens) {
      const screen = room.fixtures.find((f) => f.kind === "screen");
      if (screen === undefined || screen.kind !== "screen") continue;
      const at = `${room.domain}/${room.permalink}`;
      const lane = footprint(screen.slot, SCREEN_LANE);
      for (const p of room.props) {
        if (p.anchor === "wall")
          expect(edgeKey(edgeOf(p)), `${at} ${p.kind}`).not.toBe(
            edgeKey(screen.slot),
          );
        const box = propFootprint(p);
        if (box !== null)
          expect(overlaps(box, lane), `${at} ${p.kind}`).toBe(false);
      }
      for (const h of room.heroes)
        expect(overlaps(heroFootprint(h), lane), `${at} ${h.kind}`).toBe(false);
      for (const c of room.curios)
        expect(overlaps(curioBox(c), lane), `${at} ${c.kind}`).toBe(false);
    }

    let checkedBox = 0;
    for (const place of places) {
      const arrived = withArrivalBox(
        place,
        withBridge(place, generateRoom(place), CANNED_BRIDGE_DATA),
      );
      if (arrived.box === null) continue;
      const boxHero = arrived.room.heroes[arrived.box];
      if (boxHero === undefined) continue;
      checkedBox++;
      const boxFoot = heroFootprint(boxHero);
      const at = `${place.domain}/${place.permalink}`;
      const screen = arrived.room.fixtures.find((f) => f.kind === "screen");
      if (screen !== undefined && screen.kind === "screen")
        expect(overlaps(boxFoot, footprint(screen.slot, SCREEN_LANE)), at).toBe(
          false,
        );
      const lift = arrived.room.fixtures.find((f) => f.kind === "lift");
      if (lift !== undefined && lift.kind === "lift")
        expect(overlaps(boxFoot, footprint(lift.slot, WAY_LANE)), at).toBe(
          false,
        );
    }
    expect(checkedBox).toBeGreaterThan(0);
  }, 30_000);

  it("builds the fitted bridge and matches the golden byte for byte", () => {
    // Mutation caught: any change to the builder's output, key order included.
    const fitted = withBridge(
      CANNED_BRIDGE,
      generateRoom(CANNED_BRIDGE),
      CANNED_BRIDGE_DATA,
    );
    expect(JSON.stringify(fitted, null, 2) + "\n").toBe(bridgeFittedGolden);
  });
});
