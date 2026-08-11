/**
 * The addresses inside the app, built rather than typed out.
 *
 * A route pattern and the links that point at it are one fact, and this is
 * where that fact lives: change a pattern in `routes.tsx` and the builders
 * beside it move with it, instead of a template literal in some component
 * quietly pointing nowhere.
 */

import { encodePermalink, encodeSegment } from "./api/client";
import { crystallineAddress } from "./api/engram";
import { NEIGHBORHOOD_DEPTH } from "./api/graph";

/** The address of one domain. */
export function domainRoute(domain: string): string {
  return `/d/${encodeSegment(domain)}`;
}

/**
 * One folder of a domain, browsed on the domain's own screen.
 *
 * A query parameter rather than a segment of the path, because the browse view
 * is one of the states that screen holds in its URL beside the frontmatter
 * filters, and the root folder is the domain itself rather than a folder named
 * nothing.
 */
export function folderRoute(domain: string, folder: string): string {
  return folder === ""
    ? domainRoute(domain)
    : `${domainRoute(domain)}?path=${encodeURIComponent(folder)}`;
}

/**
 * The address of one engram.
 *
 * Built from the client's own encoders rather than from its `engramPath`,
 * which is the API path (`/domains/{d}/engrams/{permalink}`) and would be the
 * wrong URL to put in a link. What matters is shared: a permalink is itself a
 * path, so its segments are encoded one by one and its slashes stay slashes,
 * which is exactly what the `/d/:domain/e/*` pattern matches. Encoding the
 * permalink whole would turn `notes/deep/gamma` into one escaped segment and
 * the link would miss.
 */
export function engramRoute(domain: string, permalink: string): string {
  return `${domainRoute(domain)}/e/${encodePermalink(permalink)}`;
}

/**
 * The editor over one engram. Not under `/e/`: that pattern ends in a splat,
 * and a splat swallows everything after it, so `edit` gets its own segment
 * ahead of the permalink - the same shape the API gave its action routes.
 */
export function editRoute(domain: string, permalink: string): string {
  return `${domainRoute(domain)}/edit/${encodePermalink(permalink)}`;
}

/**
 * The MANIFEST page of one domain. Its own segment rather than a permalink
 * under `/e/`: a MANIFEST is not an engram and carries no permalink of its
 * own to encode.
 */
export function manifestRoute(domain: string): string {
  return `${domainRoute(domain)}/manifest`;
}

/** The editor over one domain's MANIFEST. */
export function manifestEditRoute(domain: string): string {
  return `${manifestRoute(domain)}/edit`;
}

/**
 * The neighborhood of one engram, full screen.
 *
 * The anchor is the engram's own `crystalline://` address rather than the two
 * halves of it, because that is what `GET /graph` takes and what somebody can
 * paste in from anywhere else. The depth rides along only when it is not the
 * default: a URL says what was chosen, and one hop is what a neighborhood is
 * when nobody chose.
 */
export function graphRoute(
  domain: string,
  permalink: string,
  depth: number = NEIGHBORHOOD_DEPTH,
): string {
  const anchor = encodeURIComponent(crystallineAddress(domain, permalink));
  return depth === NEIGHBORHOOD_DEPTH
    ? `/graph?anchor=${anchor}`
    : `/graph?anchor=${anchor}&depth=${String(depth)}`;
}

/**
 * The account administration screen.
 *
 * A constant rather than a literal at each call site for the reason every
 * builder here exists: the pattern in `routes.tsx` and the links pointing at
 * it are one fact, kept in one place.
 */
export function usersRoute(): string {
  return "/users";
}

/** Where the topbar's search box sends a query. */
export function searchRoute(query: string): string {
  return `/search?q=${encodeURIComponent(query)}`;
}

/**
 * Everything carrying one tag, across every domain.
 *
 * Search rather than a domain listing on purpose: a tag is a thread through the
 * whole knowledge base, and following it out of the domain it was noticed in is
 * the point of clicking one.
 */
export function tagRoute(tag: string): string {
  return `/search?tags=${encodeURIComponent(tag)}`;
}
