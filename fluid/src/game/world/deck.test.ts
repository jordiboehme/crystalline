import { describe, expect, it } from "vitest";

import { CANNED_DECK } from "./canned";
import { deckRoomSection, generateDeck, type DeckInput } from "./deck";
import { folderDeck, isManifestPermalink } from "./folders";
import { heroFootprint, propFootprint } from "./footprints";
import deckGolden from "./golden/deck.json?raw";
import { isHangar } from "./hangar";
import { deckLabel } from "./lifts";
import { overlaps } from "./sites";
import type { Box, RoomSpec } from "./types";
import { CELL } from "./units";

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

/**
 * A deck of folder `folder` in domain `station`: `n` engrams directly in
 * it, permalinks under the folder's own name, no subfolders, the whole
 * level.
 */
function folderDeckOf(folder: string, n: number): DeckInput {
  return {
    domain: "station",
    folder,
    rows: Array.from({ length: n }, (_, i) => ({
      permalink: `${folder}/e${String(i).padStart(3, "0")}`,
      title: `Entry ${String(i)}`,
      type: "engram",
      status: "stable",
    })),
    subfolders: [],
    total: n,
    truncated: false,
  };
}

/** A deck's centre column, north wall to south wall, in metres. */
function centreColumn(room: RoomSpec): Box {
  const x = Math.floor(room.width / 2);
  return { x0: x * CELL, x1: (x + 1) * CELL, z0: 0, z1: room.depth * CELL };
}

/** The lines of the room's screen. */
function screenLines(room: RoomSpec): string[] {
  const screen = room.fixtures.find((f) => f.kind === "screen");
  return screen?.kind === "screen" ? screen.lines : [];
}

describe("the deck hub (M3 C8 to C12)", () => {
  it("gives every engram of the level exactly one door across the sections (Review Focus 2)", () => {
    // Mutation caught: a door lost at a section's cut, a door made twice, the MANIFEST kept on the root deck.
    const inputs = [0, 1, 24, 25, 48, 49].map(deckOf);
    inputs.push(deckAt(500, 812));
    expect(inputs.length).toBeGreaterThan(0);
    for (const input of inputs) {
      const n = input.rows.filter(
        (r) => !isManifestPermalink(r.permalink),
      ).length;
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
    expect(doors.map((d) => d.slot.y).slice(0, 6)).toEqual([
      24, 24, 22, 22, 20, 20,
    ]);
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
    expect(screenLines(generateDeck(deckOf(1), 0))).toEqual([
      deckLabel("station", ""),
      "1 ENGRAM",
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

  it("numbers a deck of a folder that is not its own slug like its label (M3 C6, C10)", () => {
    // Mutation caught: the room's permalink made of the raw folder instead
    // of its slug, so the stencils read another deck than the title.
    const folders = ["Old Logs", "My Notes", "a/b c/d"];
    expect(folders.length).toBeGreaterThan(0);
    for (const folder of folders) {
      const room = generateDeck(folderDeckOf(folder, 3), 0);
      const deck = folderDeck("station", folder);
      expect(room.title).toBe(deckLabel("station", folder));
      expect(room.title.split(" ")[1]).toBe(String(deck));
      const stencils = room.decals.filter((d) => d.stencil !== undefined);
      expect(stencils.length).toBeGreaterThan(0);
      for (const d of stencils) expect(d.stencil?.deck, folder).toBe(deck);
    }
  });

  it("builds a section past either end as the nearest section, seed included", () => {
    // Mutation caught: the seed taken from the section asked for instead of
    // the clamped one, so one place becomes two rooms.
    expect(generateDeck(CANNED_DECK, 9)).toEqual(generateDeck(CANNED_DECK, 1));
    expect(generateDeck(CANNED_DECK, -3)).toEqual(generateDeck(CANNED_DECK, 0));
  });

  it("keeps the deck's centre column clear of heroes and floor props (M3 C9)", () => {
    // Mutation caught: the centre box not handed to `furnish`, or the hero
    // pass not keeping off `reserved`. The count of decks that still stand a
    // hero catches a fix that keeps heroes out of decks altogether. The
    // folders are the first sixty that are no hangar (M3 C13): a hangar is
    // a hall, not a corridor, and keeps no centre column.
    const folders: string[] = [];
    for (let f = 0; folders.length < 60; f++)
      if (!isHangar("station", `f${String(f)}`)) folders.push(`f${String(f)}`);
    const rooms: RoomSpec[] = [];
    for (const folder of folders)
      for (const n of [1, 3, 6, 10, 17, 24])
        rooms.push(generateDeck(folderDeckOf(folder, n), 0));
    expect(rooms.length).toBe(360);
    expect(rooms.every((r) => r.space === "deck")).toBe(true);
    let withHero = 0;
    for (const room of rooms) {
      const lane = centreColumn(room);
      if (room.heroes.length > 0) withHero++;
      for (const h of room.heroes)
        expect(
          overlaps(heroFootprint(h), lane),
          `${room.title} ${h.kind}`,
        ).toBe(false);
      for (const p of room.props) {
        const box = propFootprint(p);
        if (box !== null)
          expect(overlaps(box, lane), `${room.title} ${p.kind}`).toBe(false);
      }
    }
    expect(withHero).toBeGreaterThanOrEqual(100);
  }, 30_000);

  it("builds the same deck twice and matches the golden byte for byte", () => {
    // Mutation caught: any change to the builder's output, key order included.
    expect(generateDeck(CANNED_DECK, 1)).toEqual(generateDeck(CANNED_DECK, 1));
    expect(JSON.stringify(generateDeck(CANNED_DECK, 1), null, 2) + "\n").toBe(
      deckGolden,
    );
  });
});
