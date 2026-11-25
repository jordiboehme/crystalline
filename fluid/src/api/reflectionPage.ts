/**
 * A Reflection page's life beside its text: its settings (freshness, the
 * page it follows, its status), its owner and retirement, the pacts it
 * follows, and its clock (when it was last checked and when it is due).
 *
 * The page view rides the page read (`page` on `GET .../reflections/{path}`,
 * absent for `PACT.md` and pact files); the clock is its own small read, so
 * a reconcile, which writes no file, refreshes the clock and not the text.
 * The clock answer carries the checksum of the page file as stored: a tab
 * that heard a plain change reads the text again only when that moved.
 *
 * Read field by field like the rest of the Reflection's readers: a field the
 * server leaves out reads as null, never as a guess.
 */

import { api, encodeSegment } from "./client";
import { asArray, asNumber, asObject, asString, asStrings } from "./json";
import { reflectionKey } from "./reflectionDomains";

/** Another page, by path and title; `exists` false when it is gone. */
export interface PageRef {
  path: string;
  title: string;
  exists: boolean;
}

export interface PageRetirement {
  /** When it was retired; null for a page retired only by its status word. */
  at: string | null;
  by: string | null;
  /** The page it was part of before, for a restore back under it. */
  wasOwnedBy: PageRef | null;
}

/** A person's request that the next reflect run checks the page again. */
export interface RefreshRequested {
  at: string;
  /** Who marked it, by login; null when the page record does not say. */
  by: string | null;
  byDisplay: string | null;
}

export interface PageFinding {
  page: string;
  key: string;
  value: string;
  reason: string;
}

export interface PageView {
  /** As written when it reads; null when absent, unreadable or owned. */
  freshness: string | null;
  follows: string | null;
  status: string | null;
  owner: PageRef | null;
  retired: PageRetirement | null;
  /** The not-fresh mark; null when none is set. */
  refreshRequested: RefreshRequested | null;
  /** The pages it follows, top down; empty means it follows PACT.md only. */
  chain: { path: string; title: string }[];
  /** The pact files it follows, in order. */
  pacts: string[];
  /** Its own pact file, `<page>.pact.md`. */
  pact: { path: string; exists: boolean };
  findings: PageFinding[];
}

export interface PageClock {
  path: string;
  freshness: string | null;
  lastReconciled: string | null;
  lastCheckedSecs: number | null;
  dueInSecs: number | null;
  stale: boolean;
  overdueSecs: number | null;
  missedSecs: number | null;
  loopSlowerThanFreshness: boolean;
  waiting: boolean;
  partOf: { path: string; title: string } | null;
  clockOf: string | null;
  refreshRequested: RefreshRequested | null;
}

export interface ClockAnswer {
  clock: PageClock | null;
  /** The page file's checksum as stored; null for a missing page. */
  checksum: string | null;
}

function readRef(value: unknown): PageRef | null {
  const r = asObject(value);
  const path = asString(r?.path);
  if (path === null) {
    return null;
  }
  return {
    path,
    title: asString(r?.title) ?? path,
    exists: r?.exists === true,
  };
}

function readRefresh(value: unknown): RefreshRequested | null {
  const r = asObject(value);
  const at = asString(r?.at);
  if (at === null) {
    return null;
  }
  return { at, by: asString(r?.by), byDisplay: asString(r?.by_display) };
}

function readTitled(value: unknown): { path: string; title: string } | null {
  const r = asObject(value);
  const path = asString(r?.path);
  return path === null ? null : { path, title: asString(r?.title) ?? path };
}

/** The `page` member of a page read; null when there is none. */
export function readPageView(value: unknown): PageView | null {
  const r = asObject(value);
  if (r === null) {
    return null;
  }
  const retired = asObject(r.retired);
  const pact = asObject(r.pact);
  return {
    freshness: asString(r.freshness),
    follows: asString(r.follows),
    status: asString(r.status),
    owner: readRef(r.owner),
    retired:
      retired === null
        ? null
        : {
            at: asString(retired.at),
            by: asString(retired.by),
            wasOwnedBy: readRef(retired.was_owned_by),
          },
    refreshRequested: readRefresh(r.refresh_requested),
    chain: asArray(r.chain).flatMap((c) => {
      const page = readTitled(c);
      return page === null ? [] : [page];
    }),
    pacts: asStrings(r.pacts),
    pact: {
      path: asString(pact?.path) ?? "",
      exists: pact?.exists === true,
    },
    findings: asArray(r.findings).flatMap((f) => {
      const o = asObject(f);
      const key = asString(o?.key);
      return key === null
        ? []
        : [
            {
              page: asString(o?.page) ?? "",
              key,
              value: asString(o?.value) ?? "",
              reason: asString(o?.reason) ?? "",
            },
          ];
    }),
  };
}

export function readClockAnswer(payload: unknown): ClockAnswer {
  const r = asObject(payload);
  const c = asObject(r?.clock);
  return {
    checksum: asString(r?.checksum),
    clock:
      c === null
        ? null
        : {
            path: asString(c.path) ?? "",
            freshness: asString(c.freshness),
            lastReconciled: asString(c.last_reconciled),
            lastCheckedSecs: asNumber(c.last_checked_secs),
            dueInSecs: asNumber(c.due_in_secs),
            stale: c.stale === true,
            overdueSecs: asNumber(c.overdue_secs),
            missedSecs: asNumber(c.missed_secs),
            loopSlowerThanFreshness: c.loop_slower_than_freshness === true,
            waiting: c.waiting === true,
            partOf: readTitled(c.part_of),
            clockOf: asString(c.clock_of),
            refreshRequested: readRefresh(r?.refresh_requested),
          },
  };
}

function base(domain: string): string {
  return `/domains/${encodeSegment(domain)}/reflection`;
}

/** Under the domain's Reflection key, so a change event refreshes it. */
export function reflectionClockKey(
  domain: string,
  path: string,
): readonly unknown[] {
  return [...reflectionKey(domain), "clock", path];
}

export async function fetchReflectionClock(
  domain: string,
  path: string,
): Promise<ClockAnswer> {
  const query = new URLSearchParams({ path });
  return readClockAnswer(
    await api<unknown>(`${base(domain)}/clock?${query.toString()}`),
  );
}

/** The settings a curator changes beside the text. */
export interface PageSettings {
  freshness?: string | null;
  follows?: string | null;
  status?: string | null;
}

/**
 * What a settings change sends: only the keys whose value moved, a cleared
 * one as `""` (the route keeps a key it is not sent, and clears one sent
 * empty). Asking to clear a key that is not set sends nothing for it.
 */
export function settingsChanges(
  current: PageSettings,
  wanted: PageSettings,
): PageSettings {
  const out: PageSettings = {};
  for (const key of ["freshness", "follows", "status"] as const) {
    if (!(key in wanted)) {
      continue;
    }
    const now = current[key] ?? "";
    const next = (wanted[key] ?? "").trim();
    if (next !== now) {
      out[key] = next;
    }
  }
  return out;
}

export interface SettingsReceipt {
  checksum: string;
  page: PageView | null;
}

export async function setReflectionSettings(
  domain: string,
  path: string,
  changes: PageSettings,
): Promise<SettingsReceipt> {
  const r = asObject(
    await api<unknown>(`${base(domain)}/settings`, {
      method: "PATCH",
      body: JSON.stringify({ path, ...changes }),
    }),
  );
  return {
    checksum: asString(r?.checksum) ?? "",
    page: readPageView(r?.page),
  };
}

export async function retireReflection(
  domain: string,
  path: string,
): Promise<void> {
  await api(`${base(domain)}/retire`, {
    method: "POST",
    body: JSON.stringify({ path }),
  });
}

/** Mark a page not fresh: the next reflect run checks it again. */
export async function markReflectionStale(
  domain: string,
  path: string,
): Promise<void> {
  await api(`${base(domain)}/mark-stale`, {
    method: "POST",
    body: JSON.stringify({ path }),
  });
}

/** Restore a retired page: on its own, or back under `owner`. */
export async function restoreReflection(
  domain: string,
  path: string,
  owner?: string,
): Promise<void> {
  await api(`${base(domain)}/restore`, {
    method: "POST",
    body: JSON.stringify(owner === undefined ? { path } : { path, owner }),
  });
}
