/**
 * A folder becomes a deck (M3 C8 to C12): the hub corridor with a door to
 * each engram directly in the folder, the deck's lift at the entrance and
 * the deck's screen at the far end.
 *
 * The input is one level of the domain's tree as the tree answers it
 * (`DeckInput`): the rows directly in the folder, its subfolders' names,
 * the level's total and whether the server cut it. The builder:
 *
 * 1. drops the MANIFEST's row from the root deck (the MANIFEST is the
 *    bridge, never a door on a deck), then sorts the rest by permalink and
 *    cuts them into sections of `SECTION_SIZE` (`sectionsOf`); the section
 *    built is `clampSection(section, sections.length)`;
 * 2. lays the hub by hand: a hall 5 cells wide and
 *    `clamp(2 * ceil(n / 2) + 2, 6, 26)` cells deep for the `n` engrams of
 *    the section, the whole grid, no bays and no corridor, ceiling 4.0 m,
 *    archetype `engineering`, condition `clean`, `space: "deck"`, the
 *    entrance the south wall's centre cell (2, depth - 1);
 * 3. puts the fixtures in this order: the lift on the entrance edge
 *    (`deckStops`, the section built marked `here`), one sliding door per
 *    engram of the section in sorted order, alternating west (even index)
 *    and east (odd index) at cell row `depth - 2 - 2 * floor(i / 2)`, so the
 *    first engrams are nearest the lift, and the deck's screen on the north
 *    wall's centre edge;
 * 4. lights the grid as a generated room of salience 5 (`lightsFor`) and
 *    hands the base to `furnish` with no neighbours, which stands the
 *    heroes, dresses the hall and lays the curios, the finish and the
 *    decals exactly as it does for an engram's room.
 *
 * Every door is a `sliding` door with `relType` `""`, labelled with the
 * engram's title: the tree carries no salience, so every door is the same
 * style. The deck's seed is `seedFor(GAME_VERSION, "deck", domain, folder,
 * section)` and a door's `seedFor(deckSeed, "door", permalink)`, so the
 * same content gives the same deck byte for byte, and the order the tree
 * answered in never moves a door. The room's `permalink` is the folder's
 * slug and a `/` (`""` for the root), so the decals' deck stencils read the
 * deck's own number (`deckNumber`, as `folderDeck` numbers it); nothing
 * reads it as an address.
 *
 * The generator side: this module imports `generate.ts`, the folder and
 * lift helpers and the shared site types, never the session's modules.
 */

import { seedFor } from "../core/seed";
import { GAME_VERSION } from "../version";
import {
  clampSection,
  folderSlug,
  isManifestPermalink,
  sectionLabels,
  sectionOfPermalink,
  sectionsOf,
} from "./folders";
import { furnish, lightsFor } from "./generate";
import { LIFT_WORDS, deckLabel, deckStops, moreLine } from "./lifts";
import { NO_NEAR, type RoomBase } from "./sites";
import type { Fixture, Rect, RoomSpec, WallSlot } from "./types";

/**
 * One engram directly in a folder, as the tree lists it: its permalink,
 * title, type and status. The tree carries no salience and no path.
 */
export interface DeckRow {
  permalink: string;
  title: string;
  type: string | null;
  status: string | null;
}

/**
 * One level of a domain's tree, the input a deck is built from (M3 C8,
 * C11, C12). `folder` is the raw tree path, `""` for the root. `rows` are
 * the engrams directly in it as the tree gave them (on the root, the
 * MANIFEST's row among them; `generateDeck` drops it itself),
 * `subfolders` the names of the folders directly below it, `total` how
 * many engrams the level holds and `truncated` whether the server cut
 * `rows` short of `total`.
 */
export interface DeckInput {
  domain: string;
  folder: string;
  rows: readonly DeckRow[];
  subfolders: readonly string[];
  total: number;
  truncated: boolean;
}

/** A deck hub's width in cells (M3 C9): a 10 m corridor. */
const DECK_WIDTH = 5;

/** The shallowest and deepest a deck hub is, in cells (M3 C9). */
const DECK_MIN_DEPTH = 6;
const DECK_MAX_DEPTH = 26;

/** A deck hub's ceiling in metres (M3 C9). */
const DECK_CEILING = 4.0;

/** The salience a deck is lit as (M3 C9): the middle of the scale. */
const DECK_SALIENCE = 5;

/**
 * A deck's seed (M3 C10): the generator's version, the domain, the raw
 * folder and the section index, so each section of a folder is a room of
 * its own and the same section of the same folder is the same room.
 */
export function deckSeed(
  domain: string,
  folder: string,
  section: number,
): number {
  return seedFor(GAME_VERSION, "deck", domain, folder, section);
}

/**
 * The engrams a deck shows, in sections (M3 C8): the rows less the root
 * MANIFEST's, sorted by permalink and cut every `SECTION_SIZE`.
 */
function deckSections(input: DeckInput): DeckRow[][] {
  const root = input.folder === "";
  return sectionsOf(
    input.rows.filter((r) => !(root && isManifestPermalink(r.permalink))),
  );
}

/**
 * The deck's screen (M3 C8, C11, C12): the deck's label, then
 * `SECTION <label> (<i> OF <k>)` when the deck has more than one section,
 * then `<count> ENGRAMS` (the level's engrams less the root MANIFEST) or
 * `NO ENGRAMS`, then `+<n> MORE` for the rows the server cut.
 */
function screenLines(
  input: DeckInput,
  labels: readonly string[],
  index: number,
): string[] {
  const lines = [deckLabel(input.domain, input.folder)];
  if (labels.length > 1)
    lines.push(
      `${LIFT_WORDS.section} ${labels[index] ?? ""} (${String(index + 1)} ${LIFT_WORDS.of} ${String(labels.length)})`,
    );
  const manifest =
    input.folder === "" &&
    input.rows.some((r) => isManifestPermalink(r.permalink));
  const count = Math.max(0, input.total - (manifest ? 1 : 0));
  lines.push(
    count === 0
      ? LIFT_WORDS.noEngrams
      : `${String(count)} ${LIFT_WORDS.engrams}`,
  );
  if (input.truncated) lines.push(moreLine(input.total - input.rows.length));
  return lines;
}

/**
 * The deck of one section of a folder (M3 C8 to C12): see the module doc
 * for the rules. `section` is clamped to the deck's sections, so a section
 * past the last builds the last and a deck with no engrams builds its one
 * empty section: a hub with no doors, a screen reading `NO ENGRAMS` and
 * its lift.
 */
export function generateDeck(input: DeckInput, section: number): RoomSpec {
  const sections = deckSections(input);
  const index = clampSection(section, sections.length);
  const rows = sections[index] ?? [];
  const labels = sectionLabels(sections);
  const seed = deckSeed(input.domain, input.folder, index);
  const depth = Math.min(
    DECK_MAX_DEPTH,
    Math.max(DECK_MIN_DEPTH, 2 * Math.ceil(rows.length / 2) + 2),
  );
  const grid = Array.from({ length: depth }, () => ".".repeat(DECK_WIDTH));
  const hall: Rect = { x0: 0, y0: 0, x1: DECK_WIDTH, y1: depth };
  const centre = Math.floor(DECK_WIDTH / 2);
  const entrance = { x: centre, y: depth - 1 };
  const slug = folderSlug(input.folder);

  const fixtures: Fixture[] = [
    {
      kind: "lift",
      slot: { ...entrance, side: "s" },
      stops: deckStops(
        input.domain,
        input.folder,
        labels,
        index,
        input.subfolders,
      ),
      note: null,
      seed: seedFor(seed, "lift"),
    },
  ];
  rows.forEach((row, i) => {
    const slot: WallSlot =
      i % 2 === 0
        ? { x: 0, y: depth - 2 - 2 * Math.floor(i / 2), side: "w" }
        : {
            x: DECK_WIDTH - 1,
            y: depth - 2 - 2 * Math.floor(i / 2),
            side: "e",
          };
    fixtures.push({
      kind: "door",
      slot,
      style: "sliding",
      relType: "",
      label: row.title,
      address: { domain: input.domain, permalink: row.permalink },
      sealedLabel: null,
      seed: seedFor(seed, "door", row.permalink),
    });
  });
  fixtures.push({
    kind: "screen",
    slot: { x: centre, y: 0, side: "n" },
    lines: screenLines(input, labels, index),
    keys: [],
    seed: seedFor(seed, "screen"),
  });

  const base: RoomBase = {
    version: GAME_VERSION,
    seed,
    domain: input.domain,
    permalink: slug === "" ? "" : `${slug}/`,
    space: "deck",
    title: deckLabel(input.domain, input.folder),
    archetype: "engineering",
    condition: "clean",
    width: DECK_WIDTH,
    depth,
    grid,
    hall,
    bays: [],
    corridor: null,
    entrance,
    ceiling: DECK_CEILING,
    spawn: { ...entrance, yaw: 0 },
    fixtures,
    decor: [],
    scaffold: [],
    heroes: [],
    lights: lightsFor(
      seed,
      grid,
      hall,
      DECK_WIDTH,
      depth,
      DECK_SALIENCE,
      "clean",
    ),
    dropped: 0,
    inboundMore: 0,
  };
  return furnish(base, NO_NEAR);
}

/**
 * The section a deck is built at (M3 C1, C28): the section holding the
 * engram `from` (the one the player walks up from), else
 * `clampSection(section, sections.length)`, so an unresolved `null` reads
 * as the first section and a section past the last as the last.
 */
export function deckRoomSection(
  input: DeckInput,
  section: number | null,
  from: string | null,
): number {
  const sections = deckSections(input);
  if (from !== null) {
    const holding = sectionOfPermalink(sections, from);
    if (holding >= 0) return holding;
  }
  return clampSection(section, sections.length);
}
