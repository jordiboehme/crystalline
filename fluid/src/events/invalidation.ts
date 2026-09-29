/**
 * From one change event to the query keys it makes stale. Pure, and the
 * whole policy: the provider dedupes and fires, and nothing else in the app
 * decides what a change touches.
 *
 * Invalidation only, never `setQueryData`: a key nobody is looking at is
 * marked stale and refetched when next mounted, so a burst on a domain
 * nobody has open costs nothing.
 *
 * `syncStatusKey` is `["domains", domain, "sync"]`, inside the `["domains"]`
 * family, so the `DOMAINS_QUERY_KEY` row of an add, a delete, a MANIFEST
 * edit or a `domain` event also reaches every active `["domains", ...]`
 * query by prefix (the sync card, the local-changes list, an open conflict
 * dialog). All of those are reads; `sharePlanKey` sits outside that family
 * on purpose, so the prefix never pulls an origin.
 */

import { SYNC_SUMMARY_KEY, sharePlanKey, syncStatusKey } from "../api/admin";
import { domainTreeKey } from "../api/domain";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { engramDetailKey } from "../api/engram";
import { domainEngramsRoot } from "../api/engrams";
import type { ChangeEvent, DomainChange, EngramChange } from "../api/events";

export type QueryKey = readonly unknown[];

/** The graph prefix over every depth, as `graphKey` spells its first three parts. */
function graphPrefix(domain: string, permalink: string): QueryKey {
  return ["graph", domain, permalink];
}

/** What every engram event of a domain touches beyond the engram itself. */
function domainRows(domain: string): QueryKey[] {
  return [
    domainTreeKey(domain),
    domainEngramsRoot(domain),
    ["search"],
    ["title-matches"],
    syncStatusKey(domain),
    // Only fires while the share dialog holds the plan; a plan pulls the
    // origin, and `refetchType: "active"` is what keeps this row cheap.
    sharePlanKey(domain),
  ];
}

export function keysForEngram(change: EngramChange): QueryKey[] {
  const keys: QueryKey[] = [
    engramDetailKey(change.domain, change.permalink),
    graphPrefix(change.domain, change.permalink),
    ...domainRows(change.domain),
  ];
  if (change.kind === "moved" && change.from) {
    keys.push(engramDetailKey(change.domain, change.from.permalink));
    keys.push(graphPrefix(change.domain, change.from.permalink));
  }
  if (change.kind === "added" || change.kind === "deleted") {
    keys.push(DOMAINS_QUERY_KEY);
  }
  // Jordi, 2026-09-27: a domain's MANIFEST carries its canonical name, and
  // domainSpellings, cross-domain link resolution and the station game's
  // IDCLEV level list are all built from this key.
  if (change.kind === "modified" && change.path === "MANIFEST.md") {
    keys.push(DOMAINS_QUERY_KEY);
  }
  return keys;
}

export function keysForDomain(change: DomainChange): QueryKey[] {
  return [
    ["engram", change.domain],
    ["graph", change.domain],
    ...domainRows(change.domain),
    DOMAINS_QUERY_KEY,
    SYNC_SUMMARY_KEY,
  ];
}

export function keysFor(event: ChangeEvent): QueryKey[] | "everything" {
  switch (event.event) {
    case "engram":
      return keysForEngram(event.change);
    case "domain":
      return keysForDomain(event.change);
    case "reset":
      return "everything";
  }
}
