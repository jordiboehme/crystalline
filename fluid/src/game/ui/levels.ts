/**
 * The level select's words and its pure parts, apart from the component
 * so the component file exports a component only.
 *
 * Every string the select shows is here, and every one is the station's
 * own (C20): the title is the word that opens it, `NO SUCH LEVEL` is the
 * spec's line, and the rest follow the HUD's voice. Each domain is one row
 * (`levelsOf`), keyed and jumped to by its local name and found by every
 * name it answers to. The list is sorted by the lowercased label and then
 * by UTF-16 code unit (C12), filtered by a trimmed, case-insensitive
 * substring of any of those names (C12), and shown as a window of
 * `LEVEL_ROWS` rows around the selection (C15), which moves one row at a
 * time and stops at both ends (C13).
 */

import type { DomainSummary } from "../../api/domains";
import { domainSpellings } from "../../domainNames";
import { byLabel } from "../world/lifts";

/** The title line: the word that opens the select. */
export const LEVELS_TITLE = "IDCLEV";
/** The dialog's accessible name. */
export const LEVELS_LABEL = "Jump to a domain";
/** The filter field's accessible name. */
export const LEVELS_FIELD = "Domain name";
/** The list's accessible name. */
export const LEVELS_LIST = "Domains";
/** The one line shown when no domain matches (the spec's). */
export const NO_SUCH_LEVEL = "NO SUCH LEVEL";
/** The one line shown while the listing has not arrived (C16). */
export const LEVELS_LOADING = "LOADING DOMAINS";
/** The one line shown when the listing could not be read (C16). */
export const LEVELS_FAILED = "?DOMAIN LIST ERROR";
/** The mark after the current domain's name (C14). */
export const LEVELS_HERE = "HERE";
/** The key legend under the list. */
export const LEVELS_FOOTER =
  "TYPE TO FILTER  UP/DOWN SELECT  ENTER JUMP  ESC CLOSE";

/** How many rows the list shows at most (C15). */
export const LEVEL_ROWS = 10;

/** One row of the list: one domain of the listing. */
export interface Level {
  /** The local registered name: the row's key and where a jump goes. */
  key: string;
  /** What the row shows: the local name, or the canonical one beside it. */
  label: string;
  /** The name the domain's content declares, or null when not said. */
  canonical: string | null;
  /** The former names the domain still answers to. */
  aliases: readonly string[];
  /** Whether another domain's local name holds this one's canonical name. */
  shadowed: boolean;
  /** Every name the filter finds the row by: local, canonical, aliases. */
  terms: readonly string[];
}

/**
 * One row per domain, in the listing's order. The label is the local name,
 * or `canonical (local)` when the canonical name differs from it.
 */
export function levelsOf(
  domains: readonly Pick<
    DomainSummary,
    "name" | "canonicalName" | "aliases" | "shadowed"
  >[],
): Level[] {
  return domains.map((d) => ({
    key: d.name,
    label:
      d.canonicalName !== null && d.canonicalName !== d.name
        ? `${d.canonicalName} (${d.name})`
        : d.name,
    canonical: d.canonicalName,
    aliases: d.aliases,
    shadowed: d.shadowed,
    terms: [
      d.name,
      ...(d.canonicalName !== null ? [d.canonicalName] : []),
      ...d.aliases,
    ],
  }));
}

/**
 * The key of the row `current` names, so only one row is ever marked, by
 * the same spelling table links resolve through (`domainSpellings`): a
 * local name first, then a canonical name one domain alone claims, then an
 * alias one domain alone lists and no other name takes; null when `current`
 * resolves nowhere, as a contested canonical name does (C14). A canonical
 * name the listing marks shadowed is left out: a local name holds it.
 */
export function hereKey(
  levels: readonly Level[],
  current: string,
): string | null {
  const spellings = domainSpellings(
    levels.map((l) => ({
      name: l.key,
      canonicalName: l.shadowed ? null : l.canonical,
      aliases: [...l.aliases],
    })),
  );
  return spellings.get(current) ?? null;
}

/**
 * The items sorted by lowercased label, then by UTF-16 code unit (C12):
 * `byLabel`, the comparator the airlock's lift sorts its domains with too
 * (M3 C24).
 */
export function sortLevels<T>(
  items: readonly T[],
  label: (item: T) => string = String,
): T[] {
  return [...items].sort((x, y) => byLabel(label(x), label(y)));
}

/**
 * The items with a term that holds the query, ignoring case and the query's
 * outer spaces, in the order given; all of them for an empty query (C12).
 */
export function filterLevels<T>(
  sorted: readonly T[],
  query: string,
  terms: (item: T) => readonly string[] = (item) => [String(item)],
): T[] {
  const q = query.trim().toLowerCase();
  if (q === "") return [...sorted];
  return sorted.filter((item) =>
    terms(item).some((term) => term.toLowerCase().includes(q)),
  );
}

/**
 * The rows `[start, end)` of a list of `count` shown around `selected`:
 * everything when it fits, else `rows` rows with the selection in the
 * middle, pushed back inside the list at both ends (C15).
 */
export function levelWindow(
  count: number,
  selected: number,
  rows: number = LEVEL_ROWS,
): { start: number; end: number } {
  if (count <= rows) return { start: 0, end: count };
  const start = Math.min(
    Math.max(0, selected - Math.floor(rows / 2)),
    count - rows,
  );
  return { start, end: start + rows };
}

/**
 * The selection moved by `by` rows in a list of `count`, clamped to it and
 * never wrapping; 0 for an empty list (C13).
 */
export function stepSelection(
  count: number,
  selected: number,
  by: number,
): number {
  if (count === 0) return 0;
  return Math.min(Math.max(0, selected + by), count - 1);
}
