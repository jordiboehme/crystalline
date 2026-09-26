/**
 * The level select's words and its pure parts, apart from the component
 * so the component file exports a component only.
 *
 * Every string the select shows is here, and every one is the station's
 * own (C20): the title is the word that opens it, `NO SUCH LEVEL` is the
 * spec's line, and the rest follow the HUD's voice. The list is sorted by
 * the lowercased name and then by code point (C12), filtered by a trimmed,
 * case-insensitive substring (C12), and shown as a window of `LEVEL_ROWS`
 * rows around the selection (C15), which moves one row at a time and stops
 * at both ends (C13).
 */

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

/** The names sorted by lowercased name, then by code point (C12). */
export function sortLevels(names: readonly string[]): string[] {
  const order = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);
  return [...names].sort(
    (a, b) => order(a.toLowerCase(), b.toLowerCase()) || order(a, b),
  );
}

/**
 * The names that hold the query, ignoring case and the query's outer
 * spaces, in the order given; all of them for an empty query (C12).
 */
export function filterLevels(
  sorted: readonly string[],
  query: string,
): string[] {
  const q = query.trim().toLowerCase();
  if (q === "") return [...sorted];
  return sorted.filter((name) => name.toLowerCase().includes(q));
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
