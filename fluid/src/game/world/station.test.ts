/**
 * The station's seams on a generated room (M3 C28): `withExit` and the
 * entrance edge it puts the exit on.
 */
import { describe, expect, it } from "vitest";

import { seedFor } from "../core/seed";
import { CANNED_BRIDGE, CANNED_HUB, CANNED_WORKSHOP } from "./canned";
import { curioBox } from "./curios";
import { heroFootprint, propFootprint } from "./footprints";
import { generateRoom } from "./generate";
import { LANE_WIDTH } from "./props";
import { edgeKey, edgeOf, overlaps } from "./sites";
import { withExit } from "./station";
import type { Box, PlaceInput, RoomSpec } from "./types";
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
