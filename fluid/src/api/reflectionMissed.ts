/**
 * The Reflection pages a reflect run missed, across every domain the caller
 * may read (`GET /reflection/missed`): a run started after the page fell due
 * and ended without reconciling it. Longest missed first, as the server
 * orders them. For the admin banner and the maintenance view.
 *
 * Its own small module, apart from the lazy Reflection readers: the banner
 * is drawn by the frame on every screen.
 */

import { api } from "./client";
import { asArray, asNumber, asObject, asString } from "./json";

export interface MissedPage {
  domain: string;
  path: string;
  freshness: string;
  lastCheckedSecs: number;
  missedSecs: number;
  waiting: boolean;
}

export const MISSED_PAGES_KEY = ["reflection-missed"] as const;

export function readMissedPages(payload: unknown): MissedPage[] {
  return asArray(asObject(payload)?.pages).flatMap((value) => {
    const r = asObject(value);
    const domain = asString(r?.domain);
    const path = asString(r?.path);
    const missed = asNumber(r?.missed_secs);
    if (domain === null || path === null || missed === null) {
      return [];
    }
    return [
      {
        domain,
        path,
        freshness: asString(r?.freshness) ?? "",
        lastCheckedSecs: asNumber(r?.last_checked_secs) ?? 0,
        missedSecs: missed,
        waiting: r?.waiting === true,
      },
    ];
  });
}

export async function fetchMissedPages(): Promise<MissedPage[]> {
  return readMissedPages(await api<unknown>("/reflection/missed"));
}
