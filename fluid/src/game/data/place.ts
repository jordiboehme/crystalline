/**
 * From what the reading screen fetches to what the station is built from.
 *
 * `placeFromDetail` takes the engram detail, its neighbourhood graph, the
 * domain list and a page of inbound references, and narrows them into the
 * generator's `PlaceInput`. It fetches nothing itself: the source that calls it
 * has already gathered every payload, so the mapping is a pure function and
 * the same payloads always build the same room.
 *
 * Targets resolve through the reading screen's own resolver
 * (`buildWikilinkResolver` and `referenceState` in `wikilinks.ts`) rather than
 * through a rule of the game's own. What counts as a link is one fact: a door
 * that opens where the page shows a broken reference, or a sealed door where
 * the page shows a link, would be the game contradicting the app it hides in.
 *
 * A target the index resolved but the graph did not locate (a truncated
 * graph, say) keeps `resolved: true` and gets no address. The generator seals
 * it as `NO ROUTE`. It is never looked up by its bracket text instead: the
 * text is a title as often as a permalink, and a door that guessed its way to
 * a permalink could lead somewhere the reference never meant.
 *
 * Hatches come from the inbound page, not from the detail's five-entry sample,
 * and are capped at `HATCH_CAP`. An engram hundreds of others point at is a
 * hub, and a hub is still one room: past the cap the placard names the rest
 * as a count, and the true total rides along for it as `inboundTotal`.
 */

import type { EngramDetail, EngramReference } from "../../api/engram";
import type { GraphNeighborhood } from "../../api/graph";
import type { InboundRefPage } from "../../api/inbound";
import {
  buildWikilinkResolver,
  innerOf,
  referenceState,
  type WikilinkResolver,
} from "../../wikilinks";
import { placeKeyOf } from "../paths";
import {
  HATCH_CAP,
  type PlaceAddress,
  type PlaceInbound,
  type PlaceInput,
  type PlaceReference,
} from "../world/types";

/**
 * Everything one place is built from.
 *
 * `graph` and `inbound` are null when their request failed: the room is still
 * built, with every resolved reference sealed and no hatches. `targetSalience`
 * holds each located target's salience keyed by `placeKeyOf`, null (or no
 * entry) where it could not be fetched; it picks the door style.
 *
 * `domains` is undefined when the domain listing could not be read. That is
 * not the same as an empty list: an empty list tells the resolver that no
 * prefix names a domain, while undefined tells it this caller cannot tell, and
 * it falls back to the behaviour the reading screen has before its listing
 * lands. A listing outage therefore degrades the room instead of failing it.
 */
export interface PlaceSources {
  detail: EngramDetail;
  graph: GraphNeighborhood | null;
  domains: readonly string[] | undefined;
  inbound: InboundRefPage | null;
  targetSalience: ReadonlyMap<string, number | null>;
}

/**
 * Read an engram route (`/d/<domain>/e/<permalink>`) back into its address,
 * or null when it is not one.
 *
 * Split into segments and read by position rather than searched for `/e/`, so
 * a domain that happens to be called `e` still reads right. Every segment is
 * decoded on its own, the inverse of how `engramRoute` encoded it.
 */
function addressOfRoute(href: string): PlaceAddress | null {
  const [lead, d, domain, e, ...rest] = href.split("/");
  if (
    lead !== "" ||
    d !== "d" ||
    e !== "e" ||
    domain === undefined ||
    domain === "" ||
    rest.length === 0
  ) {
    return null;
  }
  try {
    return {
      domain: decodeURIComponent(domain),
      permalink: rest.map((segment) => decodeURIComponent(segment)).join("/"),
    };
  } catch {
    return null;
  }
}

/** Compare two strings by code unit, the same on every machine and locale. */
function compare(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Map one parsed reference through the resolver and the graph. */
function placeReference(
  reference: EngramReference,
  resolve: WikilinkResolver,
  targetSalience: ReadonlyMap<string, number | null>,
): PlaceReference {
  const resolution = resolve(innerOf(reference.target));
  const state = referenceState(resolution, reference.resolved);
  const address =
    resolution?.kind === "resolved" ? addressOfRoute(resolution.href) : null;
  const base = {
    relType: reference.relType,
    target: {
      domain: reference.target.domain,
      target: reference.target.target,
    },
  };
  if (state === "unresolved") {
    return {
      ...base,
      resolved: false,
      address: null,
      targetTitle: null,
      targetSalience: null,
    };
  }
  if (address === null || resolution?.kind !== "resolved") {
    // Resolved by the index, not placed by the graph: sealed as NO ROUTE.
    return {
      ...base,
      resolved: true,
      address: null,
      targetTitle: null,
      targetSalience: null,
    };
  }
  const key = placeKeyOf(address.domain, address.permalink);
  return {
    ...base,
    resolved: true,
    address,
    // The resolver's label is the reading screen's label: the node's title,
    // or the bracket text for a node that carries none.
    targetTitle: resolution.label,
    targetSalience: targetSalience.get(key) ?? null,
  };
}

/**
 * The hatches: the inbound page deduplicated by address, sorted by domain and
 * then permalink, and capped at `HATCH_CAP`.
 *
 * A source that points here with more than one relation keeps the
 * alphabetically smallest of them, not the one the page happened to list
 * first, so a page that arrives in another order builds the same hatch. Sorted
 * before the cap, so which hatches make it does not depend on that order
 * either.
 */
function hatchesOf(page: InboundRefPage): PlaceInbound[] {
  const seen = new Map<string, PlaceInbound>();
  for (const hit of page.hits) {
    const key = placeKeyOf(hit.domain, hit.permalink);
    const known = seen.get(key);
    if (known === undefined || compare(hit.rel, known.relType) < 0) {
      seen.set(key, {
        address: { domain: hit.domain, permalink: hit.permalink },
        title: hit.title,
        relType: hit.rel,
      });
    }
  }
  return [...seen.entries()]
    .sort(([a], [b]) => compare(a, b))
    .slice(0, HATCH_CAP)
    .map(([, hatch]) => hatch);
}

/**
 * Build the generator's input for one engram from the payloads the reading
 * screen already fetches. Pure and deterministic: the order of the inbound
 * hits never changes the result, and the order of the graph's nodes does not
 * either as long as titles are unique within a domain. Two nodes sharing a
 * title are told apart by the shared resolver, which keeps the first node it
 * finds under that title, so for them the node order decides where a link
 * written by title lands.
 */
export function placeFromDetail(input: PlaceSources): PlaceInput {
  const { detail, graph, domains, inbound, targetSalience } = input;
  const resolve = buildWikilinkResolver(detail, graph ?? undefined, domains);

  const map = (reference: EngramReference) =>
    placeReference(reference, resolve, targetSalience);

  const { frontmatter } = detail;
  return {
    domain: detail.domain,
    permalink: detail.permalink,
    title: detail.title,
    type: frontmatter.type,
    status: frontmatter.status,
    salience: frontmatter.salience,
    validFrom: frontmatter.validFrom,
    validTo: frontmatter.validTo,
    tags: [...frontmatter.tags],
    content: detail.content,
    relations: detail.relations.map(map),
    links: detail.links.map(map),
    inbound: inbound === null ? [] : hatchesOf(inbound),
    inboundTotal: inbound === null ? detail.inboundCount : inbound.total,
    observations: detail.observations.map((observation) => ({
      category: observation.category,
      content: observation.content,
    })),
  };
}
