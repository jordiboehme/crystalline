/**
 * Showing a date the API wrote.
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

/** `n thing` or `n things`, for the counts that appear all over the screens. */
export function plural(count: number, one: string, many: string): string {
  return `${String(count)} ${count === 1 ? one : many}`;
}
