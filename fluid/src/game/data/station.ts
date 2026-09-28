/**
 * Loading every kind of station address from real data (M3 C22, C23, C29):
 * the airlock's domain directory, a bridge's tree root and its MANIFEST, a
 * deck's tree level, and an engram's own place, each turned into the
 * `StationRoomInput` `roomFor` (`world/station.ts`) builds a room from.
 *
 * `loadStation` is the one entry point: it reads `address.kind` and hands
 * off to the loader for that kind, which reads only the endpoints that kind
 * needs. Every one of them shares `loadPlace`'s cancellation contract
 * (`abortable`, `checkAborted`, `data/source.ts`): a signal fired at any
 * await boundary rejects the promise with a `DOMException` named
 * `AbortError`, never leaves it hanging.
 *
 * What fails decides what comes back, the same rule `loadPlace` follows:
 *
 * - the **airlock** reads the domain listing alone. A failure gives its
 *   `domains` null - "cannot tell", never `offline` (C29's directory reads
 *   `?DOMAIN LIST ERROR` on `null`, a station-wide fault would be the wrong
 *   read for one listing failing to load).
 * - a **bridge** reads its domain's tree root, then, once the tree has
 *   answered, the MANIFEST engram the tree names (C22): the root row whose
 *   permalink upper-cases to `MANIFEST`, else the literal permalink
 *   `manifest`. A 404, 403 or a dropped connection on the tree gives
 *   `missing`, `denied` or `offline` for the whole bridge; any other tree
 *   failure is not fatal - the panel degrades instead (`folders: null`,
 *   `rootDeck: false`, the `manifest` fallback permalink), the same shape
 *   C29's `?DECK LIST ERROR` note already reads, so the choice costs no
 *   new case in `world/bridge.ts`. The MANIFEST fetch is `loadPlace`
 *   itself (a MANIFEST is an ordinary engram, and the bridge is its
 *   generated room with fittings, C3): a 404 there is not `missing` but
 *   C23's stand-in place (the domain as title, type `manifest`, no
 *   content and no ways), a 403 is `denied`, and anything else `loadPlace`
 *   already distinguishes on its own. The domain listing is read back from
 *   `loadPlace`'s own cache afterward, never fetched a second time (see
 *   `loadBridge`'s own comment); a failed listing gives `engrams: null`,
 *   `private: false` and falls the screen's `display` name back to the
 *   domain's own key.
 * - a **deck** reads its tree level alone: a 404, 403 or dropped connection
 *   gives `missing`, `denied` or `offline`; anything else is a fault and is
 *   rethrown, as `loadPlace`'s own detail fetch does. The rows, the
 *   subfolders, the total and whether the level was cut pass through
 *   unchanged - `generateDeck` drops the root's MANIFEST row itself (M3
 *   A17), so this loader never has to.
 * - an **engram** is `loadPlace` itself, with one thing added: the deck it
 *   belongs to, `folderOfPath` of its detail's `path` (domain relative,
 *   `crates/rest/src/engrams.rs` confirms it: `"alpha.md"`,
 *   `"notes/beta.md"`), or `folderOfPermalink` of its own permalink when
 *   the path is null (a virtual domain's engram lives at no file). The
 *   detail is read back from the cache under `loadPlace`'s own key
 *   (`engramDetailKey`) rather than fetched again.
 */

import type { QueryClient } from "@tanstack/react-query";

import { ApiProblem } from "../../api/client";
import { treeQuery, type DomainTree } from "../../api/domain";
import {
  DOMAINS_QUERY_KEY,
  fetchDomains,
  type DomainListing,
} from "../../api/domains";
import { engramDetailKey, type EngramDetail } from "../../api/engram";
import {
  folderOfPath,
  folderOfPermalink,
  isManifestPermalink,
} from "../world/folders";
import type { AirlockInput } from "../world/airlock";
import type { BridgeInput } from "../world/bridge";
import type { DeckInput, DeckRow } from "../world/deck";
import type { StationRoomInput } from "../world/station";
import type { PlaceInput, StationAddress } from "../world/types";
import { abortable, checkAborted, GAME_STALE_MS, loadPlace } from "./source";

/**
 * What loading one address came to: the room's own input, or one of the
 * three answers the server gave about the place itself - not registered,
 * not readable by this account, or unreachable - each a room the session
 * draws on its own rather than an error (M3 C29). A fault that is none of
 * these, or a cancelled load, rejects the promise instead (see the module
 * comment).
 */
export type LoadedStation =
  | StationRoomInput
  | { kind: "missing" }
  | { kind: "denied" }
  | { kind: "offline" };

/** The domain listing's query, shared by the airlock and a bridge. */
function domainsQuery() {
  return {
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
    staleTime: GAME_STALE_MS,
  };
}

/**
 * The station's reading of an `ApiProblem`: `missing` for a 404, `denied`
 * for a 403, `offline` for a dropped connection (status `0`), null for
 * anything else (including an error that is not an `ApiProblem` at all),
 * which the caller decides on its own - rethrown by a deck, degraded
 * instead by a bridge's tree (see the module comment).
 */
function faultOf(error: unknown): "missing" | "denied" | "offline" | null {
  if (error instanceof ApiProblem) {
    if (error.status === 404) return "missing";
    if (error.status === 403) return "denied";
    if (error.status === 0) return "offline";
  }
  return null;
}

/**
 * The airlock's input (M3 C24, C29): every domain's name and whether it is
 * private, or null when the listing failed. `here` is always null - only
 * `roomFor` knows what the player is arriving from, so only it can mark a
 * stop as the one the player came from.
 */
async function loadAirlock(
  client: QueryClient,
  signal: AbortSignal | undefined,
): Promise<LoadedStation> {
  checkAborted(signal);
  const domains = await abortable(
    client
      .fetchQuery(domainsQuery())
      .then((listing) =>
        listing.domains.map((d) => ({ name: d.name, private: d.private })),
      )
      .catch(() => null),
    signal,
  );
  checkAborted(signal);
  const input: AirlockInput = { domains, here: null };
  return { kind: "airlock", input };
}

/** C23's stand-in place for a domain whose MANIFEST does not exist. */
function standInPlace(domain: string, permalink: string): PlaceInput {
  return {
    domain,
    permalink,
    title: domain,
    type: "manifest",
    status: null,
    salience: null,
    validFrom: null,
    validTo: null,
    tags: [],
    content: "",
    relations: [],
    links: [],
    inbound: [],
    inboundTotal: 0,
    observations: [],
  };
}

/**
 * One domain's bridge (M3 C20, C22, C23, C29): the tree root, then the
 * MANIFEST by the permalink the tree names. See the module comment for
 * what each failure turns into.
 *
 * The listing is never fetched here directly: `loadPlace`'s own parallel
 * round (for the MANIFEST) already reads it, under the very key
 * `domainsQuery` names, to resolve a bracket link's domain prefix - so
 * this reads it back from that one attempt's cache (`client.getQueryData`,
 * fetching nothing) once `loadPlace` has settled, whether that attempt
 * succeeded or failed. `loadPlace` reads every one of its four payloads in
 * parallel regardless of any other one failing, so the listing has always
 * been attempted by the time it returns. Fetching it here too, ahead of
 * `loadPlace`, would cost a second request on a failed listing (TanStack
 * Query does not treat an error result as fresh, so a second `fetchQuery`
 * for the same key retries rather than reusing it).
 */
async function loadBridge(
  client: QueryClient,
  domain: string,
  signal: AbortSignal | undefined,
): Promise<LoadedStation> {
  checkAborted(signal);
  let folders: readonly string[] | null;
  let rootDeck: boolean;
  let manifestPermalink: string;
  try {
    const tree = await abortable(
      client.fetchQuery(treeQuery(domain, "")),
      signal,
    );
    const manifestRow = tree.engrams.find((r) =>
      isManifestPermalink(r.permalink),
    );
    manifestPermalink = manifestRow?.permalink ?? "manifest";
    folders = tree.folders;
    rootDeck = tree.total - (manifestRow === undefined ? 0 : 1) > 0;
  } catch (error) {
    const fault = faultOf(error);
    if (fault !== null) return { kind: fault };
    folders = null;
    rootDeck = false;
    manifestPermalink = "manifest";
  }
  checkAborted(signal);

  const loadedPlace = await loadPlace(
    client,
    domain,
    manifestPermalink,
    signal,
  );
  checkAborted(signal);
  let place: PlaceInput;
  if (loadedPlace.kind === "place") {
    place = loadedPlace.place;
  } else if (loadedPlace.kind === "missing") {
    place = standInPlace(domain, manifestPermalink);
  } else {
    return loadedPlace;
  }

  let engrams: number | null = null;
  let priv = false;
  let display = domain;
  const listing = client.getQueryData<DomainListing>(DOMAINS_QUERY_KEY);
  if (listing !== undefined) {
    const row = listing.domains.find((d) => d.name === domain);
    if (row !== undefined) {
      engrams = row.engrams;
      priv = row.private;
      display = row.canonicalName ?? domain;
    }
  }

  const bridge: BridgeInput = {
    domain,
    display,
    engrams,
    private: priv,
    folders,
    rootDeck,
  };
  return { kind: "bridge", place, bridge };
}

/**
 * One folder's deck (M3 C8, C11, C12, C29): the tree level, passed through
 * unchanged (`generateDeck` drops the root's MANIFEST row itself, M3 A17).
 */
async function loadDeck(
  client: QueryClient,
  address: { domain: string; folder: string; section: number | null },
  signal: AbortSignal | undefined,
): Promise<LoadedStation> {
  checkAborted(signal);
  let tree: DomainTree;
  try {
    tree = await abortable(
      client.fetchQuery(treeQuery(address.domain, address.folder)),
      signal,
    );
  } catch (error) {
    const fault = faultOf(error);
    if (fault !== null) return { kind: fault };
    throw error;
  }
  checkAborted(signal);
  const rows: DeckRow[] = tree.engrams.map((r) => ({
    permalink: r.permalink,
    title: r.title,
    type: r.type,
    status: r.status,
  }));
  const input: DeckInput = {
    domain: address.domain,
    folder: address.folder,
    rows,
    subfolders: tree.folders,
    total: tree.total,
    truncated: tree.truncated,
  };
  return { kind: "deck", input, section: address.section };
}

/**
 * One engram's place, with the deck it belongs to (M3 C28, C29): `loadPlace`
 * itself, plus `folder`, read from the detail `loadPlace` already cached
 * under `engramDetailKey` - never fetched a second time.
 */
async function loadEngram(
  client: QueryClient,
  domain: string,
  permalink: string,
  signal: AbortSignal | undefined,
): Promise<LoadedStation> {
  const loaded = await loadPlace(client, domain, permalink, signal);
  if (loaded.kind !== "place") return loaded;
  const detail = client.getQueryData<EngramDetail>(
    engramDetailKey(domain, permalink),
  );
  const folder =
    detail?.path !== null && detail?.path !== undefined
      ? folderOfPath(detail.path)
      : folderOfPermalink(permalink);
  return { kind: "engram", place: loaded.place, folder };
}

/**
 * Load everything one `StationAddress` is built from (M3 C22, C23, C29):
 * see the module comment for what each address kind reads and what each
 * failure turns into. `signal` cancels the load exactly as `loadPlace`'s
 * does.
 */
export async function loadStation(
  client: QueryClient,
  address: StationAddress,
  signal?: AbortSignal,
): Promise<LoadedStation> {
  checkAborted(signal);
  switch (address.kind) {
    case "airlock":
      return loadAirlock(client, signal);
    case "bridge":
      return loadBridge(client, address.domain, signal);
    case "deck":
      return loadDeck(client, address, signal);
    case "engram":
      return loadEngram(client, address.domain, address.permalink, signal);
  }
}
