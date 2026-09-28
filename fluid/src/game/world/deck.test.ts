import { describe, expect, it } from "vitest";

import { CANNED_DECK } from "./canned";
import { deckRoomSection, generateDeck, type DeckInput } from "./deck";
import { isManifestPermalink } from "./folders";
import deckGolden from "./golden/deck.json?raw";
import { deckLabel } from "./lifts";
import type { RoomSpec } from "./types";

/**
 * A root deck in domain `station`: `n` engrams `e000`, `e001`, ... and the
 * MANIFEST, the whole level (`total` n + 1, not truncated).
 */
function deckOf(n: number): DeckInput {
  const rows = Array.from({ length: n }, (_, i) => ({
    permalink: `e${String(i).padStart(3, "0")}`,
    title: `Entry ${String(i)}`,
    type: "engram",
    status: "stable",
  }));
  return {
    domain: "station",
    folder: "",
    rows: [
      ...rows,
      {
        permalink: "MANIFEST",
        title: "Station",
        type: "manifest",
        status: null,
      },
    ],
    subfolders: [],
    total: n + 1,
    truncated: false,
  };
}

/**
 * A root deck cut by the tree: `n` rows as the tree gave them, the
 * MANIFEST among them (so `n - 1` engrams), of a level of `total`.
 */
function deckAt(n: number, total: number): DeckInput {
  const whole = deckOf(n - 1);
  return { ...whole, total, truncated: true };
}

/** The lines of the room's screen. */
function screenLines(room: RoomSpec): string[] {
  const screen = room.fixtures.find((f) => f.kind === "screen");
  return screen?.kind === "screen" ? screen.lines : [];
}

describe("the deck hub (M3 C8 to C12)", () => {
  it("gives every engram of the level exactly one door across the sections (Review Focus 2)", () => {
    // Mutation caught: a door lost at a section's cut, a door made twice, the MANIFEST kept on the root deck.
    const counts = [0, 1, 24, 25, 49];
    expect(counts.length).toBeGreaterThan(0);
    for (const n of counts) {
      const input = deckOf(n);
      const sections = Math.max(1, Math.ceil(n / 24));
      const seen: string[] = [];
      for (let s = 0; s < sections; s++)
        for (const f of generateDeck(input, s).fixtures)
          if (f.kind === "door" && f.address !== null)
            seen.push(f.address.permalink);
      expect(seen.sort()).toEqual(
        input.rows
          .filter((r) => !isManifestPermalink(r.permalink))
          .map((r) => r.permalink)
          .sort(),
      );
    }
  });

  it("lays the hub as a corridor with the lift at the entrance and the screen north", () => {
    // Mutation caught: doors not alternating, the lift off the entrance edge, first engrams far from the lift.
    const room = generateDeck(deckOf(24), 0);
    expect(room.hall.x1 - room.hall.x0).toBe(5);
    expect(room.depth).toBe(26);
    expect(room.fixtures.find((f) => f.kind === "lift")?.slot).toEqual({
      ...room.entrance,
      side: "s",
    });
    expect(room.fixtures.find((f) => f.kind === "screen")?.slot.side).toBe("n");
    const doors = room.fixtures.filter((f) => f.kind === "door");
    expect(doors.map((d) => d.slot.side).slice(0, 4)).toEqual([
      "w",
      "e",
      "w",
      "e",
    ]);
    expect(doors[0]?.slot.y).toBe(room.depth - 2);
    expect(
      doors.every(
        (d) => d.kind === "door" && d.style === "sliding" && d.relType === "",
      ),
    ).toBe(true);
    expect(room.space).toBe("deck");
  });

  it("says what a deck holds on its screen", () => {
    // Mutation caught: the section line on a one-section deck, the MANIFEST
    // counted, the cut count written against a constant 500.
    expect(screenLines(generateDeck(deckOf(10), 0))).toEqual([
      deckLabel("station", ""),
      "10 ENGRAMS",
    ]);
    expect(
      screenLines(generateDeck({ ...deckOf(0), subfolders: ["a"] }, 0)),
    ).toContain("NO ENGRAMS");
    expect(screenLines(generateDeck(deckAt(500, 812), 20)).at(-1)).toBe(
      "+312 MORE",
    );
    expect(screenLines(generateDeck(deckAt(480, 812), 19)).at(-1)).toBe(
      "+332 MORE",
    );
  });

  it("finds the section that holds the engram the player walks up from", () => {
    // Mutation caught: `from` ignored (the section asked for, or the first,
    // returned instead), the MANIFEST counted in the cut (every later engram
    // one place on), no clamp when `from` is not on the deck.
    const input = deckOf(49);
    expect(deckRoomSection(input, null, "e024")).toBe(1);
    expect(deckRoomSection(input, 2, "e023")).toBe(0);
    expect(deckRoomSection(input, 0, "e048")).toBe(2);
    expect(deckRoomSection(input, 9, "nowhere")).toBe(2);
    expect(deckRoomSection(input, null, null)).toBe(0);
  });

  it("builds the same deck twice and matches the golden byte for byte", () => {
    // Mutation caught: any change to the builder's output, key order included.
    expect(generateDeck(CANNED_DECK, 1)).toEqual(generateDeck(CANNED_DECK, 1));
    expect(JSON.stringify(generateDeck(CANNED_DECK, 1), null, 2) + "\n").toBe(
      deckGolden,
    );
  });
});
