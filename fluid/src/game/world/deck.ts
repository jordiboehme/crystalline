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
 *    hands the base to `furnish` with no neighbours and the hall's centre
 *    column reserved (`centreLane`), which stands the heroes, dresses the
 *    hall and lays the curios, the finish and the decals as it does for an
 *    engram's room, keeping every hero and floor prop out of that column,
 *    so the corridor stays open from the lift to the screen.
 *
 * A hangar folder (`isHangar`, M3 C13) takes the hangar's plan instead of
 * step 2's and 3's slots: `hangarLayout` (`world/hangar.ts`) gives the 20
 * by 16 cell hall, the entrance (10, 15), `space: "hangar"` with the
 * `hangar` structure right after it, a 9 m ceiling, the door slots on the
 * west, east, south and north walls clear of the bay door and the gantry
 * legs (the doors take them in sorted order) and the screen's edge (4, 0,
 * n) beside the bay door. Its zones are lit at `HANGAR_LIGHT`, steady, and
 * it reserves no centre column: a hangar is a hall, not a corridor. The
 * lift, the screen's lines, the doors' style and seeds and `furnish` are
 * the hub's.
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
 * The generator side: this module imports `generate.ts`, `hangar.ts`, the
 * folder and lift helpers and the shared site types, never the session's
 * modules.
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
import {
  HANGAR_CEILING,
  HANGAR_DEPTH,
  HANGAR_LIGHT,
  HANGAR_WIDTH,
  hangarLayout,
  isHangar,
} from "./hangar";
import {
  LIFT_WORDS,
  deckLabel,
  deckStops,
  engramCount,
  moreLine,
} from "./lifts";
import { NO_NEAR, NO_RESERVE, type Reserved, type RoomBase } from "./sites";
import type { Fixture, HangarSpec, Rect, RoomSpec, WallSlot } from "./types";
import { CELL } from "./units";

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
 * then how many engrams the level holds less the root MANIFEST
 * (`engramCount`: `NO ENGRAMS`, `1 ENGRAM` or `<n> ENGRAMS`), then
 * `+<n> MORE` for the rows the server cut.
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
  lines.push(engramCount(count));
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
  const slug = folderSlug(input.folder);
  const plan = isHangar(input.domain, input.folder)
    ? hangarPlan(rows.length, seed)
    : hubPlan(rows.length);
  const { grid, width, depth, entrance } = plan;
  const hall: Rect = { x0: 0, y0: 0, x1: width, y1: depth };

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
    const slot = plan.doorSlots[i];
    if (slot === undefined) return;
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
    slot: plan.screen,
    lines: screenLines(input, labels, index),
    keys: [],
    seed: seedFor(seed, "screen"),
  });

  const lights = lightsFor(
    seed,
    grid,
    hall,
    width,
    depth,
    DECK_SALIENCE,
    "clean",
  );
  const base: RoomBase = {
    version: GAME_VERSION,
    seed,
    domain: input.domain,
    permalink: slug === "" ? "" : `${slug}/`,
    space: plan.hangar === null ? "deck" : "hangar",
    ...(plan.hangar === null ? {} : { hangar: plan.hangar }),
    title: deckLabel(input.domain, input.folder),
    archetype: "engineering",
    condition: "clean",
    width,
    depth,
    grid,
    hall,
    bays: [],
    corridor: null,
    entrance,
    ceiling: plan.ceiling,
    spawn: { ...entrance, yaw: 0 },
    fixtures,
    decor: [],
    scaffold: [],
    heroes: [],
    lights:
      plan.hangar === null
        ? lights
        : lights.map((z) => ({ ...z, level: HANGAR_LIGHT })),
    dropped: 0,
    inboundMore: 0,
  };
  return furnish(
    base,
    NO_NEAR,
    plan.hangar === null ? centreLane(entrance.x, depth) : NO_RESERVE,
  );
}

/**
 * A deck's floor plan: its grid and size, its entrance, the wall slots its
 * doors take in sorted order, its screen's edge, its ceiling and, for a
 * hangar, its structure (null for a hub).
 */
interface DeckPlan {
  grid: string[];
  width: number;
  depth: number;
  entrance: { x: number; y: number };
  doorSlots: WallSlot[];
  screen: WallSlot;
  ceiling: number;
  hangar: HangarSpec | null;
}

/**
 * The hub's floor plan (M3 C9) for `n` engrams: 5 cells wide,
 * `clamp(2 * ceil(n / 2) + 2, 6, 26)` deep, the entrance the south wall's
 * centre cell, the doors alternating west (even index) and east (odd) at
 * row `depth - 2 - 2 * floor(i / 2)`, the screen on the north wall's
 * centre edge.
 */
function hubPlan(n: number): DeckPlan {
  const depth = Math.min(
    DECK_MAX_DEPTH,
    Math.max(DECK_MIN_DEPTH, 2 * Math.ceil(n / 2) + 2),
  );
  const centre = Math.floor(DECK_WIDTH / 2);
  const doorSlots = Array.from({ length: n }, (_, i): WallSlot =>
    i % 2 === 0
      ? { x: 0, y: depth - 2 - 2 * Math.floor(i / 2), side: "w" }
      : {
          x: DECK_WIDTH - 1,
          y: depth - 2 - 2 * Math.floor(i / 2),
          side: "e",
        },
  );
  return {
    grid: Array.from({ length: depth }, () => ".".repeat(DECK_WIDTH)),
    width: DECK_WIDTH,
    depth,
    entrance: { x: centre, y: depth - 1 },
    doorSlots,
    screen: { x: centre, y: 0, side: "n" },
    ceiling: DECK_CEILING,
    hangar: null,
  };
}

/** A hangar's floor plan (M3 C14, C15), from `hangarLayout`. */
function hangarPlan(n: number, seed: number): DeckPlan {
  const layout = hangarLayout({ rows: n, seed });
  return {
    grid: layout.grid,
    width: HANGAR_WIDTH,
    depth: HANGAR_DEPTH,
    entrance: layout.entrance,
    doorSlots: layout.doorSlots,
    screen: layout.screen,
    ceiling: HANGAR_CEILING,
    hangar: layout.hangar,
  };
}

/**
 * What a deck hub keeps clear (M3 C9): its whole centre column, north wall
 * to south wall, as a reserved box that no hero and no floor prop enters,
 * so the corridor stays open from the lift to the screen. The sight line
 * from the spawn to the screen's glass lies inside it. No edge is
 * reserved, so wall props and runs are placed as in any room. A hangar is
 * a hall, not a corridor, and takes no such box.
 */
function centreLane(centre: number, depth: number): Reserved {
  return {
    boxes: [
      { x0: centre * CELL, x1: (centre + 1) * CELL, z0: 0, z1: depth * CELL },
    ],
    edges: new Set<string>(),
  };
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
