/**
 * Searching across the domains.
 *
 * `GET /search` answers the same page envelope a domain listing does, so a
 * search pages through {@link EngramList} like any other list. What it adds is
 * `mode`: the mode that actually ran, which is not always the one asked for -
 * hybrid and semantic need embeddings and a query to embed, and fall back to
 * text without them. Nothing here hides that; the screen shows it.
 *
 * A domain named here is a filter rather than a resource, so a name nobody
 * registered narrows the answer to nothing and comes back as an empty 200. An
 * empty result under a filter is a normal answer, not a failure.
 */

import { api } from "./client";
import type { EngramPage } from "./engrams";
import { ENGRAM_PAGE_SIZE, readEngramPage } from "./engrams";

/**
 * The modes this app offers.
 *
 * The engine knows one more, `permalink`, which looks an address up rather than
 * searching for text. It is left off deliberately: a reader who has an address
 * follows it, and a mode that answers nothing for every ordinary query would be
 * a trap in a list of four that all do search.
 */
export const SEARCH_MODES = ["hybrid", "text", "semantic", "title"] as const;

/** One of the modes this app offers. */
export type SearchMode = (typeof SEARCH_MODES)[number];

/** What a search runs as when nobody chose: lexical plus semantic ranking. */
export const DEFAULT_SEARCH_MODE: SearchMode = "hybrid";

/** Read a mode name, falling back to the default for anything unknown. */
export function readSearchMode(value: string | null): SearchMode {
  return (SEARCH_MODES as readonly string[]).includes(value ?? "")
    ? (value as SearchMode)
    : DEFAULT_SEARCH_MODE;
}

/** Everything one search asks for. */
export interface SearchRequest {
  /** The free text. Empty for a filter-only search, which the API allows. */
  q: string;
  /** The domains to search, or empty for every registered domain. */
  domains: string[];
  /** One `type` value, free form. */
  type: string | null;
  /** One `status` value, free form. */
  status: string | null;
  /** Tags that must all be present. */
  tags: string[];
  /** Only engrams recorded on or after this `YYYY-MM-DD` day. */
  after: string | null;
  /** The mode asked for, which the engine may answer in a lower one. */
  mode: SearchMode;
}

/** A search with nothing asked of it. */
export const NO_SEARCH: SearchRequest = {
  q: "",
  domains: [],
  type: null,
  status: null,
  tags: [],
  after: null,
  mode: DEFAULT_SEARCH_MODE,
};

/** Whether any filter narrows this search, the mode aside. */
export function hasSearchFilters(request: SearchRequest): boolean {
  return (
    request.domains.length > 0 ||
    request.type !== null ||
    request.status !== null ||
    request.tags.length > 0 ||
    request.after !== null
  );
}

/**
 * Whether there is anything to search for.
 *
 * A filter with no query text counts: the API takes a filter-only search and
 * answers it, and a screen that refused to send one would be hiding a way of
 * asking that works.
 */
export function isSearchable(request: SearchRequest): boolean {
  return request.q.trim() !== "" || hasSearchFilters(request);
}

/** The cache key of one search, which is every parameter it carries. */
export function searchKey(request: SearchRequest): readonly unknown[] {
  return [
    "search",
    request.q,
    request.domains,
    request.type,
    request.status,
    request.tags,
    request.after,
    request.mode,
  ];
}

/** Fetch one page of results. */
export async function fetchSearch(
  request: SearchRequest,
  page: number,
): Promise<EngramPage> {
  const query = new URLSearchParams();
  const text = request.q.trim();
  if (text !== "") {
    query.set("q", text);
  }
  if (request.domains.length > 0) {
    query.set("domains", request.domains.join(","));
  }
  if (request.type !== null) {
    query.set("type", request.type);
  }
  if (request.status !== null) {
    query.set("status", request.status);
  }
  if (request.tags.length > 0) {
    query.set("tags", request.tags.join(","));
  }
  if (request.after !== null) {
    query.set("after", request.after);
  }
  query.set("search_type", request.mode);
  query.set("page", String(page));
  query.set("limit", String(ENGRAM_PAGE_SIZE));
  const payload = await api<unknown>(`/search?${query.toString()}`);
  // A search answers rows from anywhere and names the domain on each, so there
  // is no domain of the request to fall back to: a row that names none carries
  // no address and is dropped.
  return readEngramPage(payload, "", page);
}
