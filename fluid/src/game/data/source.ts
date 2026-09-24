/**
 * Where a place's payloads come from: Fluid's own query cache.
 *
 * The game fetches nothing through a channel of its own. The detail and the
 * neighbourhood graph are read with `client.fetchQuery` under the very keys the
 * reading screen uses (`engramDetailKey`, `graphKey`), so a room entered from
 * the page it mirrors costs no second request while the cached answer is fresh
 * (`GAME_STALE_MS`), and the page and the room cannot disagree about which version of the
 * engram they are holding. The domain listing is the sidebar's, under
 * `DOMAINS_QUERY_KEY`. Only the inbound page has a key of the game's own,
 * because it asks for a page size (`HATCH_CAP`) no screen asks for.
 *
 * What fails decides what comes back. The detail is the room: a 404, a 403 or
 * a dropped connection there is an answer the session shows as a place of its
 * own (`missing`, `denied`, `offline`), and anything else is a fault and is
 * rethrown. The graph and the inbound page are furniture: when either fails
 * the room is still built, with every resolved reference sealed or with no
 * hatches. A failed domain listing degrades too, to "cannot tell" rather
 * than to an empty list: an empty list would claim that no prefix names a
 * domain, while an unknown listing lets the resolver behave as the reading
 * screen does before its own listing lands.
 *
 * Door style needs each relation target's salience, which the graph does not
 * carry. So once the room's own payloads are in, every located relation target
 * is fetched as a detail too, in parallel and under the same detail key, each
 * one raced against `TARGET_TIMEOUT_MS`. A target that fails or is slow gets a
 * null salience and therefore a sliding door. Which targets are located is not
 * decided here: `placeFromDetail` is asked once with no saliences to say where
 * each relation lands, and once more with them, so the rule for what is a link
 * lives in one place only.
 */

import type { QueryClient } from "@tanstack/react-query";

import { ApiProblem } from "../../api/client";
import { DOMAINS_QUERY_KEY, fetchDomains } from "../../api/domains";
import {
  engramDetailKey,
  fetchEngramDetail,
  type EngramDetail,
} from "../../api/engram";
import { fetchGraph, graphKey, NEIGHBORHOOD_DEPTH } from "../../api/graph";
import { fetchInbound } from "../../api/inbound";
import { placeKeyOf } from "../paths";
import { HATCH_CAP, type PlaceAddress, type PlaceInput } from "../world/types";
import { placeFromDetail } from "./place";

/**
 * What loading one place came to.
 *
 * `place` carries the generator's input. The other three are answers the
 * server gave about the engram itself - it does not exist, this account may
 * not read it, or the server could not be reached - and each is a room the
 * session draws in its own right rather than an error. A fault that is none of
 * these rejects the promise instead, and so does a cancelled load (see
 * `loadPlace`).
 */
export type LoadedPlace =
  | { kind: "place"; place: PlaceInput }
  | { kind: "missing" }
  | { kind: "denied" }
  | { kind: "offline" };

/**
 * How long one relation target's detail may take before its door is built
 * without it.
 *
 * Three seconds: long enough for a cold server to answer a handful of reads,
 * short enough that one stuck request does not hold a whole room back. A
 * target that misses it gets a sliding door, and may get a different door on
 * the next visit once the request has landed in the cache.
 */
export const TARGET_TIMEOUT_MS = 3000;

/**
 * How long a payload the game fetched counts as fresh, in milliseconds.
 *
 * Every `fetchQuery` here passes it. At the default of zero a cached entry is
 * refetched on every read, so the prefetch fired when the player walks up to
 * a door would buy nothing once it had landed: stepping through would ask the
 * server again. Thirty seconds covers the walk from a door to the next room
 * and back. A room is built once per entry and live changes are milestone 4,
 * so a room that is a little behind the server is expected until then.
 */
export const GAME_STALE_MS = 30_000;

/** The cache key of the inbound page the game asks for. */
function inboundKey(domain: string, permalink: string): readonly unknown[] {
  return ["game", "inbound", domain, permalink];
}

/**
 * Read the engram's detail the way the reading screen does, so both share the
 * one cached entry.
 */
function detailQuery(domain: string, permalink: string) {
  return {
    queryKey: engramDetailKey(domain, permalink),
    queryFn: () => fetchEngramDetail(domain, permalink),
    staleTime: GAME_STALE_MS,
  };
}

/** Read the engram's neighbourhood the way the reading screen does. */
function graphQuery(domain: string, permalink: string) {
  return {
    queryKey: graphKey(domain, permalink, NEIGHBORHOOD_DEPTH),
    queryFn: () => fetchGraph(domain, permalink),
    staleTime: GAME_STALE_MS,
  };
}

/**
 * The error a cancelled load rejects with: a `DOMException` named
 * `AbortError`, the same shape `fetch` uses, whatever reason the caller gave
 * the signal. The session tells a room it abandoned from a room that failed
 * by that name alone.
 */
function abortError(): DOMException {
  return new DOMException("The place load was aborted.", "AbortError");
}

/** Reject with an `AbortError` when the signal has fired. */
function checkAborted(signal: AbortSignal | undefined): void {
  if (signal?.aborted === true) {
    throw abortError();
  }
}

/**
 * Race one round of requests against the signal, so a cancelled load rejects
 * at once instead of waiting for a request that may hang.
 *
 * The listener is removed when the round settles either way, so a long-lived
 * signal does not collect one listener per round.
 *
 * This race is also what covers cancellation. When the session aborts the
 * signal and then cancels the queries it started, the abort listener rejects
 * first, so a TanStack `CancelledError` never reaches the caller and needs no
 * mapping of its own.
 */
async function abortable<T>(
  round: Promise<T>,
  signal: AbortSignal | undefined,
): Promise<T> {
  if (signal === undefined) {
    return round;
  }
  checkAborted(signal);
  let onAbort: (() => void) | undefined;
  const aborted = new Promise<never>((_, reject) => {
    onAbort = () => reject(abortError());
    signal.addEventListener("abort", onAbort, { once: true });
  });
  try {
    return await Promise.race([round, aborted]);
  } finally {
    if (onAbort !== undefined) {
      signal.removeEventListener("abort", onAbort);
    }
  }
}

/** The settled value, or null when the request failed. */
function valueOr<T>(result: PromiseSettledResult<T>): T | null {
  return result.status === "fulfilled" ? result.value : null;
}

/**
 * One target's salience, or null when its detail failed or took longer than
 * `TARGET_TIMEOUT_MS`. A timed-out request is left running rather than
 * cancelled: it still lands in the cache, where the next visit finds it.
 */
async function targetSalience(
  client: QueryClient,
  address: PlaceAddress,
): Promise<number | null> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<null>((resolve) => {
    timer = setTimeout(() => resolve(null), TARGET_TIMEOUT_MS);
  });
  const fetched = client
    .fetchQuery(detailQuery(address.domain, address.permalink))
    .then((detail: EngramDetail) => detail.frontmatter.salience)
    .catch(() => null);
  try {
    return await Promise.race([fetched, timeout]);
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Load everything one place is built from and build its `PlaceInput`.
 *
 * Called once per room entry. The detail, the graph, the domain listing and
 * the inbound page (`page: 1, limit: HATCH_CAP`) are asked for in parallel,
 * then every located relation target's detail, then the place is mapped by
 * `placeFromDetail`. See the module comment for what each failure turns into.
 *
 * `signal` cancels the load: when it has fired at any await boundary the
 * promise rejects with a `DOMException` named `AbortError`, so the session can
 * tell a room it walked away from apart from one that failed. Requests already
 * started are not cancelled; they finish into the shared cache.
 */
export async function loadPlace(
  client: QueryClient,
  domain: string,
  permalink: string,
  signal?: AbortSignal,
): Promise<LoadedPlace> {
  checkAborted(signal);
  const [detailResult, graphResult, domainsResult, inboundResult] =
    await abortable(
      Promise.allSettled([
        client.fetchQuery(detailQuery(domain, permalink)),
        client.fetchQuery(graphQuery(domain, permalink)),
        client.fetchQuery({
          queryKey: DOMAINS_QUERY_KEY,
          queryFn: fetchDomains,
          staleTime: GAME_STALE_MS,
        }),
        client.fetchQuery({
          queryKey: inboundKey(domain, permalink),
          queryFn: () =>
            fetchInbound(domain, permalink, { page: 1, limit: HATCH_CAP }),
          staleTime: GAME_STALE_MS,
        }),
      ]),
      signal,
    );
  checkAborted(signal);

  if (detailResult.status === "rejected") {
    const error: unknown = detailResult.reason;
    if (error instanceof ApiProblem) {
      if (error.status === 404) {
        return { kind: "missing" };
      }
      if (error.status === 403) {
        return { kind: "denied" };
      }
      if (error.status === 0) {
        return { kind: "offline" };
      }
    }
    throw error;
  }
  const sources = {
    detail: detailResult.value,
    graph: valueOr(graphResult),
    domains:
      domainsResult.status === "fulfilled"
        ? domainsResult.value.domains.map((entry) => entry.name)
        : undefined,
    inbound: valueOr(inboundResult),
  };

  // First pass with no saliences, only to learn where each relation lands.
  const located = new Map<string, PlaceAddress>();
  const unstyled = placeFromDetail({ ...sources, targetSalience: new Map() });
  for (const relation of unstyled.relations) {
    if (relation.address !== null) {
      const { domain: d, permalink: p } = relation.address;
      located.set(placeKeyOf(d, p), relation.address);
    }
  }

  const saliences = await abortable(
    Promise.all(
      [...located].map(
        async ([key, address]) =>
          [key, await targetSalience(client, address)] as const,
      ),
    ),
    signal,
  );
  checkAborted(signal);

  return {
    kind: "place",
    place: placeFromDetail({ ...sources, targetSalience: new Map(saliences) }),
  };
}

/**
 * Warm the cache for a place the player is likely to enter next: its detail
 * and its graph, under the reading screen's keys.
 *
 * Fire and forget. `prefetchQuery` never rejects, so a failed warm-up costs
 * nothing, and `loadPlace` on the same place joins a request still in flight,
 * or reads the answer it left while that is fresh, instead of starting a
 * second one.
 */
export function prefetchPlace(
  client: QueryClient,
  domain: string,
  permalink: string,
): void {
  void client.prefetchQuery(detailQuery(domain, permalink));
  void client.prefetchQuery(graphQuery(domain, permalink));
}
