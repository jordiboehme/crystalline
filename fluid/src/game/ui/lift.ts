/**
 * The lift overlay's words and its pure parts, apart from the component so
 * the component file exports a component only.
 *
 * Every string the overlay shows is here: the title is the word a lift's
 * panel answers to, `NO SUCH STOP` is the line for a filter that matches
 * nothing, `NO STOPS` the line for a lift that lists none at all (an
 * account that can read no domain, at the airlock), so an empty listing
 * never reads as a filter miss, and the rest follow the HUD's own voice.
 * The stops keep the
 * lift's own order (never sorted: the fixture already ordered them, M3
 * C24), filtered by a trimmed, case-insensitive substring of the stop's
 * label (`filterLevels`, `./levels`), and shown as a window of `LEVEL_ROWS`
 * rows around the selection (`levelWindow`), which moves one row at a time
 * and stops at both ends (`stepSelection`).
 *
 * A row keeps the stop's index in the lift's own `stops` array (`liftRows`)
 * so a filtered, windowed list still rides to the right stop: `Session.ride`
 * takes that index, never a position in whatever the filter currently
 * shows.
 */

import type { LiftStop } from "../world/types";

/** The title line: the word a lift's panel opens with. */
export const LIFT_TITLE = "LIFT";
/** The dialog's accessible name. */
export const LIFT_LABEL = "Choose a stop";
/** The filter field's accessible name. */
export const LIFT_FIELD = "Stop name";
/** The list's accessible name. */
export const LIFT_LIST = "Stops";
/** The one line shown when no stop matches the filter. */
export const NO_SUCH_STOP = "NO SUCH STOP";
/** The one line shown when the lift lists no stops at all, filter or not. */
export const NO_STOPS = "NO STOPS";
/** The mark after the stop the lift already stands at. */
export const LIFT_HERE = "HERE";
/** The accessible text of the key drawn before a private stop's label. */
export const LIFT_PRIVATE = "private";
/** The key legend under the list. */
export const LIFT_FOOTER =
  "TYPE TO FILTER  UP/DOWN SELECT  ENTER RIDE  ESC CLOSE";

/** One row of the list: one stop of the lift, keeping its place in `stops`. */
export interface LiftRow {
  /** The stop's index in the lift's own stops array: what `onRide` takes. */
  index: number;
  /** What the row shows. */
  label: string;
  /** Whether the row is drawn with the key pictogram (a private domain). */
  key: boolean;
  /** Whether the row is the stop the lift already stands at. */
  here: boolean;
}

/**
 * One row per stop, in the lift's own order (never re-sorted): each row
 * carries the stop's index in `stops`, so it survives filtering intact.
 */
export function liftRows(stops: readonly LiftStop[]): LiftRow[] {
  return stops.map((stop, index) => ({
    index,
    label: stop.label,
    key: stop.key,
    here: stop.here,
  }));
}
