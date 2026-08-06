/**
 * The rows every list in this app is made of, and the page envelope they
 * arrive in.
 *
 * One reader, three callers: the domain listing (`GET /domains/{d}/engrams`),
 * search, and the folder tree, whose entries carry less (a browse payload knows
 * a permalink and a title, not a status) and are read into the same row so one
 * list component can draw either. What a source does not say stays null rather
 * than being filled in with a plausible default: a row that claims a status
 * nobody wrote would be a lie a reader cannot see through.
 */

import { api, encodeSegment } from "./client";
import { asArray, asNumber, asObject, asString, asStrings } from "./json";

/** One engram as a list draws it. */
export interface EngramRow {
  /** The domain it lives in, which its link needs. */
  domain: string;
  /** Its permalink, which is a path of its own. */
  permalink: string;
  /** Its title, falling back to the permalink when it has none. */
  title: string;
  /** Its `type` frontmatter, free form, or null when the source did not say. */
  type: string | null;
  /** Its `status` frontmatter, free form, or null when the source did not say. */
  status: string | null;
  /** Its tags, in the order the source gave them. */
  tags: string[];
  /** `engram` or `observation`: what the hit itself is, on a search payload. */
  kind: string | null;
  /** The line an observation hit sits on, when it is one. */
  line: number | null;
  /** The matched text a search payload carries, when it carries one. */
  snippet: string | null;
}

/** The engine's page envelope, which every list here pages the same way. */
export interface EngramPage {
  /** The search mode that actually ran, on a payload that ran one. */
  mode: string | null;
  /** How many rows match in total, across every page. */
  total: number;
  /** Which page this is, one based. */
  page: number;
  /** How many rows a page holds. */
  limit: number;
  /** How many rows this page holds. */
  count: number;
  /** The rows themselves. */
  hits: EngramRow[];
}

/** The frontmatter filters `GET /domains/{d}/engrams` and `/search` share. */
export interface EngramFilters {
  /** One `type` value, free form. */
  type: string | null;
  /** One `status` value, free form. */
  status: string | null;
  /** Tags that must all be present. */
  tags: string[];
}

/** No filter at all: the whole domain. */
export const NO_FILTERS: EngramFilters = { type: null, status: null, tags: [] };

/** Whether any filter is set, which is what decides which view a screen shows. */
export function hasFilters(filters: EngramFilters): boolean {
  return (
    filters.type !== null || filters.status !== null || filters.tags.length > 0
  );
}

/**
 * How many rows a page asks for.
 *
 * Larger than the API's default of ten because these lists are virtualized and
 * a reader scrolls through a screenful in one flick; small enough that the
 * first page is still one quick request.
 */
export const ENGRAM_PAGE_SIZE = 50;

/**
 * Read one row, or null when it carries no address.
 *
 * `domain` is the domain of the request, used when the payload does not name
 * one of its own: a domain listing answers rows about the domain in the path,
 * while search answers rows from anywhere and names the domain on each.
 */
export function readEngramRow(
  value: unknown,
  domain: string,
): EngramRow | null {
  const record = asObject(value);
  if (!record) {
    return null;
  }
  const permalink = asString(record.permalink);
  const where = asString(record.domain) ?? asString(domain);
  if (permalink === null || where === null) {
    return null;
  }
  return {
    domain: where,
    permalink,
    title: asString(record.title) ?? permalink,
    // A listing calls it `engram_type` and a browse payload calls it `type`;
    // both mean the engram's own `type` frontmatter.
    type: asString(record.engram_type) ?? asString(record.type),
    status: asString(record.status),
    tags: asStrings(record.tags),
    kind: asString(record.kind),
    line: asNumber(record.line),
    snippet: asString(record.snippet),
  };
}

/**
 * Read a page envelope.
 *
 * `page` is what was asked for, used when the payload does not say: paging
 * depends on knowing which page came back, and guessing page one for a later
 * page would make the list ask for page two forever.
 */
export function readEngramPage(
  payload: unknown,
  domain: string,
  page: number,
): EngramPage {
  const record = asObject(payload);
  const hits = asArray(record?.hits)
    .map((hit) => readEngramRow(hit, domain))
    .filter((row): row is EngramRow => row !== null);
  return {
    mode: asString(record?.mode),
    total: asNumber(record?.total) ?? hits.length,
    page: asNumber(record?.page) ?? page,
    limit: asNumber(record?.limit) ?? ENGRAM_PAGE_SIZE,
    count: asNumber(record?.count) ?? hits.length,
    hits,
  };
}

/** A page of rows made from a source that has no paging of its own. */
export function singlePage(hits: EngramRow[]): EngramPage {
  return {
    mode: null,
    total: hits.length,
    page: 1,
    limit: hits.length,
    count: hits.length,
    hits,
  };
}

/** Whether a page envelope has another page behind it. */
export function hasNextPage(page: EngramPage): boolean {
  return page.page * page.limit < page.total;
}

/** The cache key of one domain's filtered listing. */
export function domainEngramsKey(
  domain: string,
  filters: EngramFilters,
): readonly unknown[] {
  return ["domain-engrams", domain, filters.type, filters.status, filters.tags];
}

/** Fetch one page of a domain's engrams, filtered by frontmatter. */
export async function fetchDomainEngrams(
  domain: string,
  filters: EngramFilters,
  page: number,
): Promise<EngramPage> {
  const query = new URLSearchParams();
  if (filters.type !== null) {
    query.set("type", filters.type);
  }
  if (filters.status !== null) {
    query.set("status", filters.status);
  }
  if (filters.tags.length > 0) {
    query.set("tags", filters.tags.join(","));
  }
  query.set("page", String(page));
  query.set("limit", String(ENGRAM_PAGE_SIZE));
  const payload = await api<unknown>(
    `/domains/${encodeSegment(domain)}/engrams?${query.toString()}`,
  );
  return readEngramPage(payload, domain, page);
}
