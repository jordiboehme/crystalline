/**
 * What was recorded lately, across every domain.
 *
 * The window is the engine's to choose: it defaults there rather than here, so
 * the API and the MCP tool answer the same question when neither is told which
 * window to use, and the answer says which window it used. Whatever shows the
 * feed shows that word rather than a window it assumed.
 */

import { api } from "./client";
import type { EngramRow } from "./engrams";
import { readEngramRow } from "./engrams";
import { asArray, asObject, asString } from "./json";

/** One entry of the feed: a row, plus the day it was recorded. */
export interface ActivityItem extends EngramRow {
  /** The `recorded_at` date, as written, or null when there was none. */
  recordedAt: string | null;
}

/** The feed, and the window it covers. */
export interface Activity {
  /** The window the engine actually used, such as `7d`. */
  timeframe: string | null;
  /** The entries, newest first, as the engine ordered them. */
  items: ActivityItem[];
}

/** Read the activity payload. */
export function readActivity(payload: unknown): Activity {
  const record = asObject(payload);
  const items: ActivityItem[] = [];
  for (const entry of asArray(record?.engrams)) {
    // Every entry names its own domain; one that does not is unaddressable
    // and is dropped rather than linked into the wrong domain.
    const row = readEngramRow(entry, "");
    if (row === null) {
      continue;
    }
    items.push({ ...row, recordedAt: asString(asObject(entry)?.recorded_at) });
  }
  return { timeframe: asString(record?.timeframe), items };
}

/** The cache key of the activity feed. */
export const ACTIVITY_QUERY_KEY = ["activity"] as const;

/** Fetch the activity feed, on the engine's own default window. */
export async function fetchActivity(): Promise<Activity> {
  return readActivity(await api<unknown>("/activity"));
}
