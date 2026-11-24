/**
 * The words under a Reflection page's title about its freshness clock:
 * "Last checked 2 hours ago, due again in 1 hour", or "Overdue since 3
 * hours, last checked 5 hours ago" in the warning colour, or "Waiting: open
 * in the editor" for a due page a co-editing room holds.
 *
 * The clock's times are seconds relative to when it was read, so every
 * sentence here takes the seconds since then (`elapsed`) and ages them: an
 * open tab does not stay at "2 hours ago".
 *
 * An owned page has no clock of its own: it says its owner's last check and
 * no due time. An owner with no freshness keeps none, and a curator reads
 * why (see {@link noFreshnessNote}).
 *
 * Overdue is the server's rule (one whole period missed), never worked out
 * here: a 1h page under a daily loop is not overdue all day.
 */

import type { PageClock } from "./api/reflectionPage";
import { ageText } from "./reflectionText";

export type FreshnessLine =
  | { kind: "plain"; text: string }
  | { kind: "overdue"; text: string; hint: string | null }
  | { kind: "waiting"; text: string };

export const WAITING_TEXT = "Waiting: open in the editor";
export const LOOP_SLOWER_HINT =
  "The reflect loop runs less often than this page's freshness.";

/** A span of time: "3 hours", or "less than a minute". */
export function span(seconds: number): string {
  return seconds < 60 ? "less than a minute" : ageText(seconds);
}

/** "checked 2 hours ago", or "checked just now". */
export function checked(seconds: number): string {
  return seconds < 60 ? "checked just now" : `checked ${ageText(seconds)} ago`;
}

export function freshnessLine(
  clock: PageClock | null,
  elapsed: number,
): FreshnessLine | null {
  if (clock === null || clock.lastCheckedSecs === null) {
    return null;
  }
  const last = clock.lastCheckedSecs + elapsed;
  // An owned page: its owner's last check, never a due time.
  if (clock.partOf !== null) {
    return clock.clockOf === null
      ? null
      : { kind: "plain", text: `Last ${checked(last)}` };
  }
  if (clock.waiting) {
    return { kind: "waiting", text: WAITING_TEXT };
  }
  if (clock.overdueSecs !== null) {
    return {
      kind: "overdue",
      text: `Overdue since ${span(clock.overdueSecs + elapsed)}, last ${checked(last)}`,
      hint: clock.loopSlowerThanFreshness ? LOOP_SLOWER_HINT : null,
    };
  }
  const due = clock.dueInSecs === null ? 0 : clock.dueInSecs - elapsed;
  return {
    kind: "plain",
    text: `Last ${checked(last)}, ${due <= 0 ? "due now" : `due again in ${span(due)}`}`,
  };
}

/**
 * For a curator: why an owned page is not checked on a schedule. Null when
 * a page up its owners keeps a clock, and for a page on its own.
 */
export function noFreshnessNote(clock: PageClock | null): string | null {
  if (clock?.partOf == null || clock.clockOf !== null) {
    return null;
  }
  return `${clock.partOf.title} has no freshness, so this page is not checked on a schedule`;
}

/** A reflect run started after the page fell due and left it. */
export function skippedNote(clock: PageClock | null): string | null {
  return clock !== null && clock.missedSecs !== null && !clock.waiting
    ? "A reflect run skipped this page."
    : null;
}
