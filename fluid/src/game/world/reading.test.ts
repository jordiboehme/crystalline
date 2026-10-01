/**
 * What every read fixture and the room's computers read (0.22 spec 3a and
 * 3b), on the canned rooms: the workshop's engram for a terminal, a poster
 * and the placard, the canned bridge for a machine, the canned deck and
 * hangar for a screen and the listing, and the airlock for its directory.
 */

import { describe, expect, it } from "vitest";

import { airlockRoom } from "./airlock";
import { withBridge } from "./bridge";
import {
  CANNED_BRIDGE,
  CANNED_BRIDGE_DATA,
  CANNED_DECK,
  CANNED_DOMAINS,
  CANNED_HANGAR,
  CANNED_WORKSHOP,
} from "./canned";
import { consoleRoom } from "./consoleRoom";
import { generateDeck } from "./deck";
import { NOTES, POSTER_LINES, generateRoom } from "./generate";
import { SCREEN_LINES } from "./lifts";
import { fixtureReading, roomReading } from "./reading";
import type { Fixture, PlaceInput, RoomSpec } from "./types";

/** The index of the first fixture that passes `test`, or a failed test. */
function indexOf(room: RoomSpec, test: (f: Fixture) => boolean): number {
  const index = room.fixtures.findIndex(test);
  expect(index).toBeGreaterThanOrEqual(0);
  return index;
}

/** The fixture at `index`, which must exist. */
function fixtureAt(room: RoomSpec, index: number): Fixture {
  const f = room.fixtures[index];
  if (f === undefined) throw new Error(`no fixture ${String(index)}`);
  return f;
}

const workshop = generateRoom(CANNED_WORKSHOP);
const bridge = generateRoom(CANNED_BRIDGE);
const deck = generateDeck(CANNED_DECK, 1);
const hangar = generateDeck(CANNED_HANGAR, 0);

describe("fixtureReading at a terminal", () => {
  it("reads the place at the terminal's section, as the reader always has", () => {
    // Mutation caught: the terminal opened at the top (`section: null`) or
    // at its heading with the wrong occurrence.
    const i = indexOf(workshop, (f) => f.kind === "terminal");
    const terminal = fixtureAt(workshop, i);
    if (terminal.kind !== "terminal") throw new Error("not a terminal");
    expect(fixtureReading(workshop, i, CANNED_WORKSHOP)).toEqual({
      title: CANNED_WORKSHOP.title,
      content: CANNED_WORKSHOP.content,
      section: { heading: terminal.heading, occurrence: terminal.section },
    });
  });

  it("reads nothing with no place", () => {
    // Mutation caught: a terminal in a hand-built room opening an empty
    // reader.
    const i = indexOf(workshop, (f) => f.kind === "terminal");
    expect(fixtureReading(workshop, i, null)).toBeNull();
  });
});

describe("fixtureReading at a poster (0.22 R17)", () => {
  /** The workshop's place with eight warnings and two uncategorised notes. */
  const crowded: PlaceInput = {
    ...CANNED_WORKSHOP,
    observations: [
      ...Array.from({ length: 8 }, (_, i) => ({
        category: "warning",
        content: `Warning ${String(i)}.`,
      })),
      { category: null, content: "A loose note." },
      { category: "  ", content: "A blank note." },
      { category: "decision", content: "One decision." },
    ],
  };
  const room = generateRoom(crowded);

  it("reads every observation of its category, more than the wall shows", () => {
    // Mutation caught: the poster's own six lines read instead of the
    // place's observations.
    const i = indexOf(
      room,
      (f) => f.kind === "poster" && f.category === "warning",
    );
    const poster = fixtureAt(room, i);
    if (poster.kind !== "poster") throw new Error("not a poster");
    expect(poster.lines).toHaveLength(POSTER_LINES);
    const reading = fixtureReading(room, i, crowded);
    expect(reading).toEqual({
      title: crowded.title,
      content: [
        "## warning",
        Array.from({ length: 8 }, (_, k) => `- Warning ${String(k)}.`).join(
          "\n",
        ),
      ].join("\n\n"),
      section: null,
    });
  });

  it("reads the observations with no category under NOTES", () => {
    // Mutation caught: a null or blank category left out of the NOTES
    // poster's reading.
    const i = indexOf(room, (f) => f.kind === "poster" && f.category === NOTES);
    expect(fixtureReading(room, i, crowded)?.content).toBe(
      `## ${NOTES}\n\n- A loose note.\n- A blank note.`,
    );
  });

  it("reads nothing with no place", () => {
    // Mutation caught: a poster reading its wall lines with no place.
    const i = indexOf(workshop, (f) => f.kind === "poster");
    expect(fixtureReading(workshop, i, null)).toBeNull();
  });
});

describe("fixtureReading at the placard", () => {
  it("reads its lines, one paragraph each, under the room's title", () => {
    // Mutation caught: the placard's lines joined into one paragraph, or
    // the place's title instead of the room's.
    const i = indexOf(workshop, (f) => f.kind === "placard");
    const placard = fixtureAt(workshop, i);
    if (placard.kind !== "placard") throw new Error("not a placard");
    expect(placard.lines.length).toBeGreaterThan(1);
    expect(fixtureReading(workshop, i, CANNED_WORKSHOP)).toEqual({
      title: workshop.title,
      content: placard.lines.join("\n\n"),
      section: null,
    });
    // The placard is the room's, so it reads with no place as well.
    expect(fixtureReading(workshop, i, null)?.content).toBe(
      placard.lines.join("\n\n"),
    );
  });
});

describe("fixtureReading at a machine", () => {
  it("reads the place's engram from the top: a bridge its MANIFEST", () => {
    // Mutation caught: a machine opened at a terminal's section, or reading
    // nothing.
    const i = indexOf(bridge, (f) => f.kind === "machine");
    expect(fixtureReading(bridge, i, CANNED_BRIDGE)).toEqual({
      title: CANNED_BRIDGE.title,
      content: CANNED_BRIDGE.content,
      section: null,
    });
  });

  it("reads nothing in a room with no place that is no deck", () => {
    // Mutation caught: a machine with no place reading the room's title.
    const i = indexOf(bridge, (f) => f.kind === "machine");
    expect(fixtureReading(bridge, i, null)).toBeNull();
  });
});

describe("fixtureReading at a screen", () => {
  it("reads a deck's screen: its first line the title, the others paragraphs", () => {
    // Mutation caught: the first line read twice, or the lines joined into
    // one paragraph.
    const i = indexOf(deck, (f) => f.kind === "screen");
    const screen = fixtureAt(deck, i);
    if (screen.kind !== "screen") throw new Error("not a screen");
    expect(screen.lines.length).toBeGreaterThan(1);
    expect(fixtureReading(deck, i, null)).toEqual({
      title: screen.lines[0],
      content: screen.lines.slice(1).join("\n\n"),
      section: null,
    });
  });

  it("reads a line in its keys as private", () => {
    // Mutation caught: the key lines read bare, or the mark put on the line
    // after the keyed one.
    const slot = { x: 1, y: 1, side: "n" as const };
    const room: RoomSpec = {
      ...deck,
      fixtures: [
        {
          kind: "screen",
          slot,
          lines: ["HEAD", "open", "shut", "last"],
          keys: [2],
          seed: 1,
        },
      ],
    };
    expect(fixtureReading(room, 0, null)?.content).toBe(
      "open\n\nshut (private)\n\nlast",
    );
    // A private bridge's screen keys its first line, the domain's name.
    const fitted = withBridge(CANNED_BRIDGE, bridge, {
      ...CANNED_BRIDGE_DATA,
      private: true,
    });
    const s = indexOf(fitted, (f) => f.kind === "screen");
    expect(fixtureReading(fitted, s, CANNED_BRIDGE)?.title).toBe(
      `${CANNED_BRIDGE_DATA.display} (private)`,
    );
  });

  it("reads the airlock's directory as every stop of its lift (0.22 R18)", () => {
    // Mutation caught: the directory's own lines read, `+N MORE` and all.
    const domains = Array.from({ length: SCREEN_LINES + 4 }, (_, k) => ({
      name: `dom-${String(k).padStart(2, "0")}`,
      private: k === 3,
    }));
    const airlock = airlockRoom({ domains, here: null });
    const i = indexOf(airlock, (f) => f.kind === "screen");
    const screen = fixtureAt(airlock, i);
    if (screen.kind !== "screen") throw new Error("not a screen");
    expect(screen.lines.at(-1)).toMatch(/^\+\d+ MORE$/);
    const reading = fixtureReading(airlock, i, null);
    expect(reading?.title).toBe(screen.lines[0]);
    const lines = reading?.content.split("\n\n") ?? [];
    expect(lines).toHaveLength(domains.length);
    expect(lines).toContain("dom-03 (private)");
    expect(lines).toContain(`dom-${String(SCREEN_LINES + 3)}`);
    expect(reading?.content).not.toMatch(/MORE/);
  });

  it("reads the airlock's failed listing as the lift's note", () => {
    // Mutation caught: the lift's note dropped, a failed listing read blank.
    const airlock = airlockRoom({ domains: null, here: null });
    const i = indexOf(airlock, (f) => f.kind === "screen");
    const lift = airlock.fixtures.find((f) => f.kind === "lift");
    expect(lift?.note).not.toBeNull();
    expect(fixtureReading(airlock, i, null)?.content).toBe(lift?.note);
  });
});

describe("fixtureReading at a way", () => {
  it("reads nothing at a door, a portal, a hatch, a lift or an exit", () => {
    // Mutation caught: a way's label read as a reading (0.22 R12).
    const kinds = ["door", "portal", "hatch"] as const;
    for (const kind of kinds) {
      const i = indexOf(bridge, (f) => f.kind === kind);
      expect(fixtureReading(bridge, i, CANNED_BRIDGE)).toBeNull();
    }
    const lift = indexOf(deck, (f) => f.kind === "lift");
    expect(fixtureReading(deck, lift, null)).toBeNull();
  });
});

describe("roomReading (0.22 R15)", () => {
  it("is the place's engram from the top in a room with a place", () => {
    // Mutation caught: the engram room's computers reading nothing.
    expect(roomReading(workshop, CANNED_WORKSHOP)).toEqual({
      title: CANNED_WORKSHOP.title,
      content: CANNED_WORKSHOP.content,
      section: null,
    });
  });

  it("is a deck's listing: its doors in order, then the lift's note", () => {
    // Mutation caught: the doors listed out of fixture order, or the note
    // left out.
    const labels = deck.fixtures.flatMap((f) =>
      f.kind === "door" ? [f.label] : [],
    );
    expect(labels.length).toBeGreaterThan(1);
    expect(roomReading(deck, null)).toEqual({
      title: deck.title,
      content: labels.map((l) => `- ${l}`).join("\n"),
      section: null,
    });
    const noted: RoomSpec = {
      ...deck,
      fixtures: deck.fixtures.map((f) =>
        f.kind === "lift" ? { ...f, note: "?DECK LIST ERROR" } : f,
      ),
    };
    expect(roomReading(noted, null)?.content).toBe(
      `${labels.map((l) => `- ${l}`).join("\n")}\n\n?DECK LIST ERROR`,
    );
  });

  it("is a hangar's listing as well", () => {
    // Mutation caught: only `deck` taken for a listing, a hangar reading
    // nothing.
    expect(hangar.space).toBe("hangar");
    const reading = roomReading(hangar, null);
    expect(reading?.title).toBe(hangar.title);
    expect(reading?.content.split("\n")).toHaveLength(
      hangar.fixtures.filter((f) => f.kind === "door").length,
    );
  });

  it("is null in the airlock and the console room", () => {
    // Mutation caught: the airlock's computers reading the place the
    // player came from.
    const airlock = airlockRoom({ domains: CANNED_DOMAINS, here: null });
    expect(roomReading(airlock, null)).toBeNull();
    expect(roomReading(airlock, CANNED_BRIDGE)).toBeNull();
    expect(roomReading(consoleRoom(), null)).toBeNull();
  });
});
