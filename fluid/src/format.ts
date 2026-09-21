/**
 * Showing a value the API wrote, in the words a reader reads.
 *
 * Dates arrive as the engine wrote them: `recorded_at` is a plain day and
 * `last_sync` is an RFC 3339 instant. Both are shown as the day they name,
 * cut out of the string rather than parsed and reformatted, because a
 * `Date` in a browser west of UTC turns `2026-08-04` into the third of August
 * and there is nothing in a knowledge base worth that. A value that is not a
 * date at all is shown as written.
 */

/** The leading `YYYY-MM-DD` of a value, or the value itself when it has none. */
export function formatDay(value: string): string {
  return /^\d{4}-\d{2}-\d{2}/.test(value) ? value.slice(0, 10) : value;
}

/** An RFC 3339 instant: a day, a `T`, and a time. */
const INSTANT_SHAPE = /^\d{4}-\d{2}-\d{2}T/;

/**
 * A stored instant, in the local date and time a reader's own clock shows:
 * `2026-08-10 08:00`.
 *
 * {@link formatDay} exists because most dates in a knowledge base are days
 * somebody wrote down, where parsing and reformatting would shift the date
 * itself across the browser's own midnight. A `last_checked` timestamp is the
 * opposite kind of value: it names a precise moment, in whatever zone the
 * check happened to run, and a reader wants that moment translated into
 * theirs - the whole point of showing it at all. So this one DOES parse: the
 * fields come off a `Date` built from the string, read with `getFullYear`,
 * `getHours` and so on rather than `toISOString`, which would hand back UTC
 * and reintroduce the same shift `formatDay` was written to avoid. A value
 * that is not a parseable instant - a plain day, or anything else - has no
 * time-of-day to translate, so it is shown the way `formatDay` shows it
 * instead.
 *
 * `now` is accepted only so a caller that also calls {@link relativeTime} can
 * pass the same tick to both without a branch; the date and time here never
 * move with it.
 */
export function formatInstant(value: string, _now: Date = new Date()): string {
  if (!INSTANT_SHAPE.test(value)) {
    return formatDay(value);
  }
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return formatDay(value);
  }
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${localDay(parsed)} ${pad(parsed.getHours())}:${pad(parsed.getMinutes())}`;
}

/**
 * How long ago (or from now) a stored instant was, in the words a reader
 * reads: "just now", "13 minutes ago", "2 hours ago", "3 days ago".
 *
 * `Intl.RelativeTimeFormat` supplies the wording; what this picks is the
 * unit, off the largest one that still fits the gap - the same reason a
 * calendar app says "2 hours ago" rather than "120 minutes ago". Under a
 * minute is answered as "just now" rather than `Intl`'s own "now" or "3
 * seconds ago", since nothing this app checks on a per-second cadence is
 * worth a reader trusting to the second. Returns null when `value` does not
 * parse, which a caller reads as "say nothing" rather than "say now".
 */
export function relativeTime(value: string, now: Date): string | null {
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return null;
  }
  const diffSeconds = (parsed.getTime() - now.getTime()) / 1000;
  const absSeconds = Math.abs(diffSeconds);
  if (absSeconds < 60) {
    return "just now";
  }
  const unit: Intl.RelativeTimeFormatUnit =
    absSeconds < 3600 ? "minute" : absSeconds < 86400 ? "hour" : "day";
  const secondsInUnit = unit === "minute" ? 60 : unit === "hour" ? 3600 : 86400;
  const rtf = new Intl.RelativeTimeFormat("en", { numeric: "auto" });
  return rtf.format(Math.trunc(diffSeconds / secondsInUnit), unit);
}

/**
 * An OKF actor, in the words a reader reads.
 *
 * The conventions are written for sorting rather than for reading: a person is
 * `human:name`, an automated job is `process:name` and an agent is
 * `name/version`. Each is turned around so the name comes first and the kind
 * follows it. An actor in none of those conventions is somebody else's
 * convention rather than a malformed one, so it is shown exactly as written.
 */
export function formatActor(by: string): string {
  const lower = by.toLowerCase();
  if (lower.startsWith("human:")) {
    return `${by.slice(6)} (human)`;
  }
  if (lower.startsWith("process:")) {
    return `${by.slice(8)} (process)`;
  }
  const slash = by.indexOf("/");
  if (slash > 0) {
    return `${by.slice(0, slash)} (agent, ${by.slice(slash + 1)})`;
  }
  return by;
}

/** The units a stored file is measured in, which is how its ceiling is stated. */
const SIZE_UNITS = ["B", "KiB", "MiB"] as const;

/**
 * A byte count in the words a reader reads: `812 B`, `45 KiB`, `1.2 MiB`.
 *
 * Binary units rather than decimal ones, because the ceiling an author is told
 * about is 10 MiB and a size shown in MB beside it would be a second unit for
 * the same quantity. A round-numbered fraction keeps one decimal only while it
 * says something: `1.2 MiB` is worth the digit, `45.0 KiB` is not, so anything
 * from ten up is stated whole.
 *
 * The unit is chosen against the ROUNDED figure rather than the exact one,
 * which is the whole difference between `1.0 MiB` and `1024 KiB`: one byte
 * under a megabyte rounds up to the next unit's own boundary, and a size that
 * reads as a full 1024 of anything is a size stated in the wrong unit.
 */
export function formatBytes(bytes: number): string {
  const render = (value: number, unit: number) =>
    unit === 0 || value >= 10 ? Math.round(value).toString() : value.toFixed(1);
  let value = Math.max(bytes, 0);
  let unit = 0;
  while (unit < SIZE_UNITS.length - 1 && Number(render(value, unit)) >= 1024) {
    value /= 1024;
    unit += 1;
  }
  return `${render(value, unit)} ${SIZE_UNITS[unit] ?? "B"}`;
}

/** `n thing` or `n things`, for the counts that appear all over the screens. */
export function plural(count: number, one: string, many: string): string {
  return `${String(count)} ${count === 1 ? one : many}`;
}

/**
 * Whether a url is an address a screen will hand a reader as a link.
 *
 * The url is the forge's own word, and on an enterprise install the forge is a
 * machine somebody else administers. Nothing about a review needs a scheme
 * other than http or https, so anything else - `javascript:` first among them
 * - is drawn as text instead of as a link that runs on press. Defence in
 * depth rather than a known hole: the engine builds these from the API's own
 * fields, and this is the line that holds if one day it does not.
 *
 * Shared rather than duplicated: the proposals card and the share dialog both
 * decide whether a proposal's own url is worth linking, off the same report.
 */
export function isWebAddress(url: string): boolean {
  return url.startsWith("https://") || url.startsWith("http://");
}

/**
 * Today where this browser is, as `YYYY-MM-DD`.
 *
 * The local day rather than the UTC one, for the reason {@link formatDay}
 * exists: the dates in a knowledge base are days somebody wrote down, and a
 * reader west of UTC comparing them against `toISOString()` would be told a day
 * ahead of the one they are living in.
 */
export function localDay(now: Date = new Date()): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${String(now.getFullYear())}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

/**
 * Whether `day` is on or before `today`, for two `YYYY-MM-DD` strings.
 *
 * Compared as text, which is what the ISO ordering is for, and never parsed
 * into a `Date`. A value that is not a plain day is not a date this app can
 * reason about, so it answers false rather than guessing.
 */
export function hasArrived(day: string, today: string): boolean {
  return /^\d{4}-\d{2}-\d{2}$/.test(day) && day <= today;
}
