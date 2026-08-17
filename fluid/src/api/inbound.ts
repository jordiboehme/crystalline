/**
 * What points at one engram, as `GET /domains/{domain}/inbound/{permalink}`
 * answers it: a page of referencing engrams, an exact total under the active
 * filters, and the per-relation summary of all of them.
 *
 * The detail payload's own inbound block stays what it is - a count and a
 * sample capped at five, cheap enough to ride every read. This is the endpoint
 * for the other case: an engram hundreds or thousands of engrams point at,
 * where the answer to "who points here" is a map to browse rather than a list
 * to print. The summary is what the panel draws its chips from, the page is
 * what one chip's popover fills with, and neither ever loads the whole set.
 */

import { api, encodePermalink, encodeSegment } from "./client";
import { asArray, asNumber, asObject, asString } from "./json";

/** One engram that points at the one being read. */
export interface InboundRefHit {
  /** The domain it lives in, which may not be the target's. */
  domain: string;
  permalink: string;
  /** Its title, falling back to the permalink when it carries none. */
  title: string;
  /** Its domain-relative file path, or the empty string for a virtual domain. */
  path: string;
  /**
   * Its `status` frontmatter, free form, or null when it carries none. Here so
   * a retired engram still reads as retired in a list of what points at
   * something, the way it does everywhere else in this app.
   */
  status: string | null;
  /** The relation it points with; `links_to` for a prose wikilink. */
  rel: string;
}

/** One relation type pointing at this engram, with how many do. */
export interface InboundRefType {
  rel: string;
  count: number;
}

/** One page of inbound references. */
export interface InboundRefPage {
  /** How many references match the active filters, exactly. */
  total: number;
  page: number;
  limit: number;
  /** How many hits this page carries. */
  count: number;
  /**
   * Every relation type pointing here with its count, most-used first.
   *
   * Deliberately not narrowed by the filters and present on every page, so a
   * client may read it from whichever response it happens to hold: it is the
   * map the reader filters *with*, and a map that redrew itself as it was used
   * would be no map at all.
   */
  types: InboundRefType[];
  hits: InboundRefHit[];
}

/** How many references one popover page carries. */
export const INBOUND_PAGE_SIZE = 20;

/** What one request narrows and pages by. */
export interface InboundRefQuery {
  /** One relation type, or undefined for every one. */
  rel?: string;
  /** A substring of the referencing engram's title or path. */
  q?: string;
  /** One-based page. Defaults to 1. */
  page?: number;
  /** Page size. Defaults to {@link INBOUND_PAGE_SIZE}. */
  limit?: number;
}

/** Read one hit, or null when it carries no address. */
function readHit(value: unknown): InboundRefHit | null {
  const record = asObject(value);
  const domain = asString(record?.domain);
  const permalink = asString(record?.permalink);
  if (domain === null || permalink === null) {
    return null;
  }
  return {
    domain,
    permalink,
    title: asString(record?.title) || permalink,
    path: asString(record?.path) ?? "",
    status: asString(record?.status),
    rel: asString(record?.rel) ?? "",
  };
}

/** Read one summary entry, or null when it names no relation. */
function readType(value: unknown): InboundRefType | null {
  const record = asObject(value);
  const rel = asString(record?.rel);
  if (rel === null) {
    return null;
  }
  return { rel, count: asNumber(record?.count) ?? 0 };
}

/** Read an inbound page payload. */
export function readInboundPage(payload: unknown): InboundRefPage {
  const record = asObject(payload);
  const hits = asArray(record?.hits)
    .map(readHit)
    .filter((hit): hit is InboundRefHit => hit !== null);
  return {
    total: asNumber(record?.total) ?? hits.length,
    page: asNumber(record?.page) ?? 1,
    limit: asNumber(record?.limit) ?? INBOUND_PAGE_SIZE,
    count: asNumber(record?.count) ?? hits.length,
    types: asArray(record?.types)
      .map(readType)
      .filter((entry): entry is InboundRefType => entry !== null),
    hits,
  };
}

/** The path of one engram's inbound references, filters and all. */
export function inboundPath(
  domain: string,
  permalink: string,
  query: InboundRefQuery = {},
): string {
  const params = new URLSearchParams({
    page: String(query.page ?? 1),
    limit: String(query.limit ?? INBOUND_PAGE_SIZE),
  });
  if (query.rel) {
    params.set("rel", query.rel);
  }
  // An empty filter is no filter: an unfiltered request and one whose box was
  // typed into and then cleared must be the same request, so they land on the
  // same cache key and the same server-side plan.
  if (query.q && query.q.trim() !== "") {
    params.set("q", query.q.trim());
  }
  const path = `/domains/${encodeSegment(domain)}/inbound/${encodePermalink(permalink)}`;
  return `${path}?${params.toString()}`;
}

/** The cache key of one engram's relation summary. */
export function inboundSummaryKey(
  domain: string,
  permalink: string,
): readonly unknown[] {
  return ["inbound", domain, permalink];
}

/** Fetch one page of what points at an engram. */
export async function fetchInbound(
  domain: string,
  permalink: string,
  query: InboundRefQuery = {},
): Promise<InboundRefPage> {
  return readInboundPage(
    await api<unknown>(inboundPath(domain, permalink, query)),
  );
}

/**
 * The summary alone, for the panel's first paint.
 *
 * Asks for the smallest page the envelope has rather than for no page at all:
 * `limit` is a paging knob, and a zero that meant "summary only" would be a
 * second meaning for it on the wire. The one hit that comes back is unused, and
 * the panel is drawn entirely from `types`.
 */
export async function fetchInboundSummary(
  domain: string,
  permalink: string,
): Promise<InboundRefPage> {
  return fetchInbound(domain, permalink, { page: 1, limit: 1 });
}
