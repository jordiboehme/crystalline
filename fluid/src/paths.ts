/**
 * The addresses inside the app, built rather than typed out.
 *
 * A route pattern and the links that point at it are one fact, and this is
 * where that fact lives: change a pattern in `routes.tsx` and the builders
 * beside it move with it, instead of a template literal in some component
 * quietly pointing nowhere.
 */

import { encodePermalink, encodeSegment } from "./api/client";

/** The address of one domain. */
export function domainRoute(domain: string): string {
  return `/d/${encodeSegment(domain)}`;
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
