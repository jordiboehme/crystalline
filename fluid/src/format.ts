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
