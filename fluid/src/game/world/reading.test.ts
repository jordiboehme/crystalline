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
import { heroFootprint } from "./footprints";
import type { Player } from "./move";
import {
  computerFocus,
  computersOf,
  fixtureReading,
  roomReading,
  type ComputerPoint,
} from "./reading";
import type { Fixture, Hero, PlaceInput, RoomSpec } from "./types";

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
    // The second of two sections under one heading opens at occurrence 1.
    const twice: PlaceInput = {
      ...CANNED_WORKSHOP,
      content: "# Log\n\n## Shift\nFirst.\n## Shift\nSecond.",
    };
    const room = generateRoom(twice);
    const second = indexOf(
      room,
      (f) => f.kind === "terminal" && f.heading === "Shift" && f.section === 1,
    );
    expect(fixtureReading(room, second, twice)?.section).toEqual({
      heading: "Shift",
      occurrence: 1,
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
    // The placard is the room's, so it reads with no place as well, under
    // the room's own title.
    expect(fixtureReading(workshop, i, null)).toEqual({
      title: workshop.title,
      content: placard.lines.join("\n\n"),
      section: null,
    });
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
    expect(kinds.length).toBeGreaterThan(0);
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

/**
 * `base` with one of each computer kind (0.22 R14) set by hand, as the
 * dev demo rooms set their lists, and a thing of another kind beside each
 * list's computers.
 */
function computerRoom(base: RoomSpec): RoomSpec {
  const hero = (kind: Hero["kind"], x: number, y: number, turn: number) => ({
    kind,
    variant: 0,
    x,
    y,
    turn,
    seed: 1,
  });
  const curio = (kind: RoomSpec["curios"][number]["kind"], x: number) => ({
    kind,
    variant: 0,
    x,
    y: 3.5,
    h: 0.9,
    turn: 0,
    seed: 1,
  });
  return {
    ...base,
    decor: [
      { kind: "captain-chair", x: 2, y: 2, turn: 0, seed: 1 },
      { kind: "command-console", x: 3, y: 2, turn: 0, seed: 1 },
    ],
    props: [
      {
        kind: "crate",
        variant: 0,
        anchor: "floor",
        x: 1.5,
        y: 1.5,
        turn: 0,
        seed: 1,
      },
      {
        kind: "wall-monitor",
        variant: 0,
        anchor: "wall",
        x: 1.5,
        y: 0,
        turn: 2,
        seed: 1,
      },
      {
        kind: "designer-tower",
        variant: 0,
        anchor: "floor",
        x: 2.5,
        y: 1.5,
        turn: 1,
        seed: 1,
      },
      {
        kind: "locker-bank",
        variant: 0,
        anchor: "wall",
        x: 2.5,
        y: 0,
        turn: 2,
        seed: 1,
      },
      {
        kind: "gravity-console",
        variant: 0,
        anchor: "floor",
        x: 3.5,
        y: 1.5,
        turn: 3,
        seed: 1,
      },
    ],
    heroes: [
      hero("arcade-cabinet", 0, 1.5, 1),
      hero("core-wall", 0, 3, 1),
      hero("photo-console", 3.5, 0, 2),
      hero("laser-desk", 4, 4, 1),
    ],
    curios: [
      curio("tape-drive", 1.5),
      curio("beige-laptop", 2.5),
      curio("breadbin-computer", 3.5),
      curio("slim-computer", 4.5),
      curio("pocket-console", 5.5),
    ],
  };
}

/** A player at `(x, z)` metres looking along `yaw`. */
function playerAt(x: number, z: number, yaw: number): Player {
  return { x, z, vx: 0, vz: 0, yaw, pitch: 0, bob: 0 };
}

/** A use point at `(x, z)` whose front faces south (+z). */
function pointAt(x: number, z: number, wall: boolean): ComputerPoint {
  return {
    list: "curios",
    index: 0,
    point: { x, z, inward: [0, 1], along: [1, 0] },
    wall,
  };
}

describe("computersOf (0.22 R14, R15)", () => {
  const room = computerRoom(workshop);

  it("lists exactly the computer kinds, in list order", () => {
    // Mutation caught: a kind left out of its set (one point fewer), or a
    // neighbour let in (the arcade cabinet, the captain's chair, the crate,
    // the locker bank, the tape drive or the pocket console).
    const points = computersOf(room, CANNED_WORKSHOP);
    expect(points.map((p) => [p.list, p.index, p.wall])).toEqual([
      ["decor", 1, false],
      ["props", 1, true],
      ["props", 2, false],
      ["props", 4, false],
      ["heroes", 1, true],
      ["heroes", 2, true],
      ["heroes", 3, false],
      ["curios", 1, false],
      ["curios", 2, false],
      ["curios", 3, false],
    ]);
  });

  it("puts a wall prop on its wall point and a free thing on its centre", () => {
    // Mutation caught: cell units taken for metres (no `CELL`), or the
    // thing's turn not read for its front.
    const points = computersOf(room, CANNED_WORKSHOP);
    const monitor = points.find((p) => p.list === "props" && p.index === 1);
    expect(monitor?.point.x).toBe(3);
    expect(monitor?.point.z).toBe(0);
    expect(monitor?.point.inward).toEqual([0, 1]);
    const laptop = points.find((p) => p.list === "curios" && p.index === 1);
    expect(laptop?.point.x).toBe(5);
    expect(laptop?.point.z).toBe(7);
    expect(laptop?.point.inward).toEqual([0, -1]);
  });

  it("lists nothing where the computers would read nothing", () => {
    // Mutation caught: the points listed whatever `roomReading` says, so
    // the airlock's or a placeless room's laptop offers a reader that
    // opens nothing.
    const airlock = computerRoom(
      airlockRoom({ domains: CANNED_DOMAINS, here: null }),
    );
    expect(airlock.curios.length).toBeGreaterThan(0);
    expect(computersOf(airlock, CANNED_WORKSHOP)).toEqual([]);
    expect(computersOf(room, null)).toEqual([]);
  });

  it("lists a deck's computers with no place, since a deck reads its listing", () => {
    // Mutation caught: a deck's computers dropped for want of a place.
    expect(computersOf(computerRoom(deck), null)).toHaveLength(10);
  });

  it("puts a hero's point half its footprint's depth in front of its centre", () => {
    // Mutation caught: the police box's construction copied as it is (half
    // the depth out from the anchor), which puts a backed hero's point
    // inside its own body, half way between its wall and its face.
    const points = computersOf(room, CANNED_WORKSHOP);
    const at = (index: number) =>
      points.find((p) => p.list === "heroes" && p.index === index)?.point;
    // The photo console stands backed on a north wall, 0.9 m deep, its
    // front south: its face is 0.9 m out from the wall at z 0.
    const backed = room.heroes[2];
    if (backed === undefined) throw new Error("no photo console");
    const box = heroFootprint(backed);
    expect(at(2)?.x).toBeCloseTo((box.x0 + box.x1) / 2, 9);
    expect(at(2)?.z).toBeCloseTo(box.z1, 9);
    expect(at(2)?.z).toBeCloseTo(0.9, 9);
    // The laser desk stands free at (8, 8) facing east, 3 m deep: its point
    // is its footprint's centre, not its face.
    expect(at(3)?.x).toBeCloseTo(8, 9);
    expect(at(3)?.z).toBeCloseTo(8, 9);
    expect(at(3)?.inward).toEqual([1, 0]);
  });

  it("uses the free laser desk from behind and a wall computer not through its wall", () => {
    // Mutation caught: a free hero's point left on its front face (out of
    // reach from behind a 3 m deep desk), or a wall hero given a free
    // point (used through its wall).
    const points = computersOf(room, CANNED_WORKSHOP);
    const desk = points.filter((p) => p.list === "heroes" && p.index === 3);
    expect(desk).toHaveLength(1);
    expect(desk[0]?.wall).toBe(false);
    // The desk faces east at (8, 8); the player stands west of it, behind
    // it, looking east.
    const behind = playerAt(8 - 1.85, 8, -Math.PI / 2);
    expect(computerFocus(desk, behind, "LAB")?.point.index).toBe(3);
    // The photo console is backed on the north wall; a player on the far
    // side of that wall cannot use it.
    const backed = points.filter((p) => p.list === "heroes" && p.index === 2);
    expect(backed[0]?.wall).toBe(true);
    const outside = playerAt(backed[0]?.point.x ?? 0, -1, Math.PI);
    expect(computerFocus(backed, outside, "LAB")).toBeNull();
  });

  it("skips a hero whose variant it does not know", () => {
    // Mutation caught: heroPoint throwing on a variant with no footprint.
    const odd = {
      ...room,
      heroes: room.heroes.map((h) =>
        h.kind === "laser-desk" ? { ...h, variant: 99 } : h,
      ),
    };
    let points: ReturnType<typeof computersOf> = [];
    expect(() => {
      points = computersOf(odd, CANNED_WORKSHOP);
    }).not.toThrow();
    expect(points.some((p) => p.list === "heroes" && p.index === 3)).toBe(
      false,
    );
    expect(points.some((p) => p.list === "heroes" && p.index === 2)).toBe(true);
  });
});

describe("computerFocus (0.22 R16)", () => {
  it("takes the nearest within reach and facing", () => {
    // Mutation caught: the first point taken instead of the nearest, or
    // the reach or the facing check dropped.
    const near = pointAt(0, -1, false);
    const far = { ...pointAt(0, -1.5, false), index: 1 };
    const player = playerAt(0, 0, 0);
    expect(computerFocus([far, near], player, "LAB")).toEqual({
      point: near,
      prompt: "SPACE READ LAB",
    });
    expect(computerFocus([pointAt(0, -2.5, false)], player, "LAB")).toBeNull();
    // 1 m away, but 60 degrees off the view.
    const off = pointAt(Math.sin(Math.PI / 3), -Math.cos(Math.PI / 3), false);
    expect(computerFocus([off], player, "LAB")).toBeNull();
  });

  it("refuses a wall point from behind its wall and takes a free one", () => {
    // Mutation caught: no depth check on a wall point (used through the
    // wall), or the depth check made on a free point too (a laptop never
    // read from behind its lid).
    // The points face south; the player stands north of them, facing south.
    const behind = playerAt(0, -1, Math.PI);
    expect(computerFocus([pointAt(0, 0, true)], behind, "LAB")).toBeNull();
    expect(computerFocus([pointAt(0, 0, false)], behind, "LAB")).not.toBeNull();
    // In front of its wall, the wall point is taken.
    const front = playerAt(0, 1, 0);
    expect(computerFocus([pointAt(0, 0, true)], front, "LAB")).not.toBeNull();
  });
});
