/**
 * What the CRT reader shows for a thing the player reads with Space.
 *
 * Every wall information element and every computer opens the one reader
 * with a `Reading`: a title, markdown and the `##` section to open at.
 * Everything here is pure: a room, a fixture index and the room's place go
 * in, a reading or null comes out. The content is the markdown as written;
 * the reader applies its own inline rules when it draws it.
 *
 * - A terminal reads the place's markdown at its own section.
 * - A machine reads what the room's computers read (`roomReading`): the
 *   place's engram from the top, or a deck's or hangar's listing built from
 *   its own doors (0.22 R15); nothing in the airlock or a room with no
 *   place.
 * - A poster reads every observation of its category, not only the ones on
 *   the wall (0.22 R17); an observation with no category is under `NOTES`,
 *   the poster's own rule.
 * - The placard reads its lines, one paragraph each, under the room's
 *   title.
 * - A screen reads its first line as the title and its other lines as
 *   paragraphs, a line in its `keys` reading `<name> (private)`. The
 *   airlock's directory (`large`) reads every stop of the room's lift
 *   instead, and the lift's note after them, since its `+N MORE` line cuts
 *   the list (0.22 R18).
 *
 * Only fixtures are read (0.22 R12): a prop is decoration, and a way's
 * label is already its whole text. With no place, a terminal and a poster
 * read nothing.
 */

import { NOTES } from "./generate";
import type { Fixture, PlaceInput, RoomSpec } from "./types";

/** What the reader shows: a title, markdown, and the `##` section to open at (null: the top). */
export interface Reading {
  title: string;
  content: string;
  section: { heading: string; occurrence: number } | null;
}

/** The suffix a private line reads with. */
const PRIVATE = "(private)";

/** `line` as read: with the private suffix when `key` says so. */
function marked(line: string, key: boolean): string {
  return key ? `${line} ${PRIVATE}` : line;
}

/** Lines as markdown paragraphs, one each. */
function paragraphs(lines: readonly string[]): string {
  return lines.join("\n\n");
}

/**
 * A deck's or hangar's listing: the deck's label as the title, one list
 * line per door in fixture order (the engram each leads to), then the
 * lift's note as its own paragraph when it has one.
 */
function deckListing(room: RoomSpec): Reading {
  const items: string[] = [];
  let note: string | null = null;
  for (const f of room.fixtures) {
    if (f.kind === "door") items.push(`- ${f.label}`);
    else if (f.kind === "lift" && f.note !== null) note = f.note;
  }
  const parts = [items.join("\n")];
  if (note !== null) parts.push(note);
  return {
    title: room.title,
    content: parts.filter((p) => p !== "").join("\n\n"),
    section: null,
  };
}

/** What the room's computers read (spec 3a): the place's engram from the top; a deck's or hangar's listing; null elsewhere. 0.22 R15. */
export function roomReading(
  room: RoomSpec,
  place: PlaceInput | null,
): Reading | null {
  if (room.space === "deck" || room.space === "hangar") {
    return deckListing(room);
  }
  if (room.space === "airlock" || place === null) return null;
  return { title: place.title, content: place.content, section: null };
}

/**
 * The category an observation's poster carries: its own, trimmed, or
 * `NOTES` when it has none (the generator's poster rule).
 */
function posterCategory(category: string | null): string {
  const trimmed = category?.trim() ?? "";
  return trimmed === "" ? NOTES : trimmed;
}

/** A screen's reading; see the module doc. */
function screenReading(
  room: RoomSpec,
  screen: Extract<Fixture, { kind: "screen" }>,
): Reading {
  const keys = new Set(screen.keys);
  const title = marked(screen.lines[0] ?? "", keys.has(0));
  if (screen.large === true) {
    const lift = room.fixtures.find((f) => f.kind === "lift");
    if (lift !== undefined) {
      const lines = lift.stops.map((s) => marked(s.label, s.key));
      if (lift.note !== null) lines.push(lift.note);
      return { title, content: paragraphs(lines), section: null };
    }
  }
  const lines = screen.lines.slice(1).map((l, i) => marked(l, keys.has(i + 1)));
  return { title, content: paragraphs(lines), section: null };
}

/** What the fixture at `index` reads (spec 3b table), or null when it reads nothing. 0.22 R12, R17, R18. */
export function fixtureReading(
  room: RoomSpec,
  index: number,
  place: PlaceInput | null,
): Reading | null {
  const fixture = room.fixtures[index];
  if (fixture === undefined) return null;
  switch (fixture.kind) {
    case "terminal":
      if (place === null) return null;
      return {
        title: place.title,
        content: place.content,
        section: { heading: fixture.heading, occurrence: fixture.section },
      };
    case "machine":
      return roomReading(room, place);
    case "poster": {
      if (place === null) return null;
      const items = place.observations
        .filter((o) => posterCategory(o.category) === fixture.category)
        .map((o) => `- ${o.content}`);
      return {
        title: place.title,
        content: [`## ${fixture.category}`, items.join("\n")]
          .filter((p) => p !== "")
          .join("\n\n"),
        section: null,
      };
    }
    case "placard":
      return {
        title: room.title,
        content: paragraphs(fixture.lines),
        section: null,
      };
    case "screen":
      return screenReading(room, fixture);
    case "door":
    case "portal":
    case "hatch":
    case "lift":
    case "exit":
      return null;
  }
}
