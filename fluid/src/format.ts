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

/** `n thing` or `n things`, for the counts that appear all over the screens. */
export function plural(count: number, one: string, many: string): string {
  return `${String(count)} ${count === 1 ? one : many}`;
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
