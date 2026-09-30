/**
 * The stops a lift lists and the words the lift, the screen and the exit
 * show (M3 C7, C8, C12, C24): the generator side of the station's lifts,
 * pure lists built from names, never from the order an API answered in.
 *
 * Three lifts, one per kind of place:
 *
 * - the airlock's (`airlockStops`): every domain's bridge, labelled with
 *   the domain's local name, the private ones marked with the key;
 * - the bridge's (`bridgeStops`): `AIRLOCK`, `DECK 1` (the root deck, only
 *   when the root holds engrams besides the MANIFEST) and one deck per
 *   top-level folder;
 * - a deck's (`deckStops`): `BRIDGE`, `UP` (a nested folder's parent deck),
 *   its own sections when there are more than one (the current one marked
 *   `here`), and one deck per subfolder.
 *
 * Fixed stops come first in that order, then sections in order, then
 * folders and domains by `byLabel` over their names: the comparator the
 * level select sorts its rows with (`sortLevels` in `ui/levels.ts` imports
 * it from here), so the two lists of domains never drift apart. A folder is
 * sorted by its name, never by its `DECK <n> <NAME>` label, whose number is
 * a hash.
 *
 * Every word the lift, the screen and the exit draw is a constant in
 * `LIFT_WORDS`, never spelled inline elsewhere. This module is on the
 * generator side: it never imports `ui/`, `render/` or the session's
 * modules.
 */

import { childFolder, folderDeck, folderName, parentFolder } from "./folders";
import type { LiftStop } from "./types";

/**
 * The order of two labels (M3 C7): lowercased first, then code unit order,
 * so `A` and `a` sit together and the upper-case one comes first. The
 * comparator the level select's rows are sorted with too.
 */
export function byLabel(a: string, b: string): number {
  const order = (x: string, y: string) => (x < y ? -1 : x > y ? 1 : 0);
  return order(a.toLowerCase(), b.toLowerCase()) || order(a, b);
}

/**
 * Every word the lift panel, the lift overlay, the station screens and
 * the exit draw: the fixed stops, the deck and section prefixes, the
 * screens' count and status lines, and the overflow line's word.
 */
export const LIFT_WORDS = {
  airlock: "AIRLOCK",
  bridge: "BRIDGE",
  up: "UP",
  deck: "DECK",
  section: "SECTION",
  engram: "ENGRAM",
  engrams: "ENGRAMS",
  noDecks: "NO DECKS",
  noEngrams: "NO ENGRAMS",
  deckError: "?DECK LIST ERROR",
  domainError: "?DOMAIN LIST ERROR",
  more: "MORE",
  of: "OF",
} as const;

/**
 * How many stops a lift's call panel lists: at most `LIFT_LINES` stop
 * labels, then `moreLine` for the stops left out (only when some are),
 * then the lift's `note` when it has one. So a panel of 14 stops and a
 * note reads 10 labels, `+4 MORE` and the note, 12 lines. The overlay
 * lists every stop.
 */
export const LIFT_LINES = 10;

/**
 * How many lines a station screen shows, its heading included (M3 C24):
 * the airlock's directory ends in `moreLine` when domains are left out.
 */
export const SCREEN_LINES = 12;

/**
 * A deck's label (M3 C7): `DECK 1` for a domain's root, else
 * `DECK <n> <NAME>` with `n` the folder's deck number (`folderDeck`) and
 * `NAME` its last segment, upper-cased.
 */
export function deckLabel(domain: string, folder: string): string {
  const deck = `${LIFT_WORDS.deck} ${String(folderDeck(domain, folder))}`;
  return folder === "" ? deck : `${deck} ${folderName(folder).toUpperCase()}`;
}

/**
 * How many engrams a screen names (M3 C8, C12): `NO ENGRAMS` for none,
 * `1 ENGRAM` for one, `<n> ENGRAMS` for more. The deck's screen and the
 * bridge's count line both read it, so the two never spell a count apart.
 */
export function engramCount(n: number): string {
  if (n === 0) return LIFT_WORDS.noEngrams;
  return `${String(n)} ${n === 1 ? LIFT_WORDS.engram : LIFT_WORDS.engrams}`;
}

/** The overflow line of a panel or screen: `+<n> MORE`. */
export function moreLine(n: number): string {
  return `+${String(n)} ${LIFT_WORDS.more}`;
}

/** Folder names in `byLabel` order; the input is never mutated. */
function sortedNames(names: readonly string[]): string[] {
  return [...names].sort(byLabel);
}

/** A stop that rides to a folder's deck, its first section. */
function deckStop(domain: string, folder: string): LiftStop {
  return {
    label: deckLabel(domain, folder),
    to: { kind: "deck", domain, folder, section: 0 },
    key: false,
    here: false,
  };
}

/**
 * The airlock lift's stops (M3 C24): every domain's bridge, labelled with
 * its local name, sorted by `byLabel` over the name, the key on every
 * private one and `here` on the domain named `here` (the one the player
 * came from), if any.
 */
export function airlockStops(
  domains: readonly { name: string; private: boolean }[],
  here: string | null,
): LiftStop[] {
  return [...domains]
    .sort((a, b) => byLabel(a.name, b.name))
    .map((d) => ({
      label: d.name,
      to: { kind: "bridge", domain: d.name },
      key: d.private,
      here: d.name === here,
    }));
}

/**
 * The bridge lift's stops (M3 C7, C12): `AIRLOCK`, then `DECK 1` when
 * `rootDeck` (the root holds engrams besides the MANIFEST), then one deck
 * per top-level folder in `folders`, sorted by `byLabel` over the folder's
 * name. An empty domain's bridge lists `AIRLOCK` alone.
 */
export function bridgeStops(
  domain: string,
  folders: readonly string[],
  rootDeck: boolean,
): LiftStop[] {
  const stops: LiftStop[] = [
    {
      label: LIFT_WORDS.airlock,
      to: { kind: "airlock" },
      key: false,
      here: false,
    },
  ];
  if (rootDeck) stops.push(deckStop(domain, ""));
  for (const folder of sortedNames(folders))
    stops.push(deckStop(domain, folder));
  return stops;
}

/**
 * A deck lift's stops (M3 C7): `BRIDGE`; `UP` to the parent folder's deck
 * when `folder` is nested (a top-level folder's parent is the root, whose
 * deck the bridge lists); `SECTION <label>` for each of `sectionLabels`
 * when there are more than one, `here` on the one at index `section`; then
 * one deck per name in `subfolders` (the folder's direct children, by name,
 * joined onto `folder`), sorted by `byLabel` over the name.
 */
export function deckStops(
  domain: string,
  folder: string,
  sectionLabels: readonly string[],
  section: number,
  subfolders: readonly string[],
): LiftStop[] {
  const stops: LiftStop[] = [
    {
      label: LIFT_WORDS.bridge,
      to: { kind: "bridge", domain },
      key: false,
      here: false,
    },
  ];
  const parent = parentFolder(folder);
  if (parent !== null && parent !== "")
    stops.push({ ...deckStop(domain, parent), label: LIFT_WORDS.up });
  if (sectionLabels.length > 1)
    sectionLabels.forEach((label, i) => {
      stops.push({
        label: `${LIFT_WORDS.section} ${label}`,
        to: { kind: "deck", domain, folder, section: i },
        key: false,
        here: i === section,
      });
    });
  for (const name of sortedNames(subfolders))
    stops.push(deckStop(domain, childFolder(folder, name)));
  return stops;
}
