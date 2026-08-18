/**
 * Search across the domains.
 *
 * The URL is the state. Every filter and the query itself live in the search
 * params, under the API's own names (`q`, `domains`, `type`, `status`, `tags`,
 * `after`, `search_type`), so a result page is a link somebody can send and the
 * back button walks the searches rather than the keystrokes. Nothing on this
 * screen holds a filter that the address bar does not show.
 *
 * Typing is debounced, and the pause is what writes the URL and fires the
 * query: one request per pause rather than one per keystroke. That write
 * replaces the current history entry rather than pushing, so refining a query
 * does not bury the page the reader arrived from under a stack of prefixes -
 * except the first query on a bare `/search`, which pushes, so the back button
 * still leads out of the results. A facet is a deliberate move and pushes.
 *
 * The mode is what the engine actually ran, not what was asked for. Hybrid and
 * semantic need embeddings and a query to embed them against, and fall back to
 * text without them; this screen says so instead of labelling a text search
 * hybrid.
 */

import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router";

import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import type { EngramPage } from "../api/engrams";
import type { SearchMode, SearchRequest } from "../api/search";
import {
  DEFAULT_SEARCH_MODE,
  SEARCH_DEBOUNCE_MS,
  fetchSearch,
  hasSearchFilters,
  isSearchable,
  readSearchMode,
  searchKey,
} from "../api/search";
import { fetchTags, vocabularyKey } from "../api/vocabulary";
import { EngramList } from "../components/EngramList";
import { Facets } from "../components/Facets";
import type { FacetChange } from "../components/Facets";
import { BUTTON } from "../components/primitives";
import { plural } from "../format";
import { searchTerms } from "../snippet";

export default function Search() {
  const [params, setParams] = useSearchParams();

  const request = useMemo<SearchRequest>(
    () => ({
      q: params.get("q") ?? "",
      domains: csv(params.get("domains")),
      type: params.get("type"),
      status: params.get("status"),
      tags: csv(params.get("tags")),
      after: params.get("after"),
      mode: readSearchMode(params.get("search_type")),
    }),
    [params],
  );
  const terms = useMemo(() => searchTerms(request.q), [request.q]);

  const [text, setText] = useState(request.q);
  // What the URL last agreed with, which is how a change made here is told
  // apart from one the URL made on its own.
  const settled = useRef(request.q);

  useEffect(() => {
    const q = request.q;
    // The URL moved by itself: a link, the back button, a shared address. It
    // wins, and no pending write survives it.
    if (q !== settled.current) {
      settled.current = q;
      setText(q);
      return;
    }
    // Compared trimmed, because trimmed is what gets written: a reader who
    // typed a space between two words and paused there would otherwise watch
    // the URL's tidier copy come back and eat it.
    const pending = text.trim();
    if (pending === q) {
      return;
    }
    const timer = setTimeout(() => {
      settled.current = pending;
      setParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          write(next, "q", pending);
          return next;
        },
        // The first query on a bare /search is a step, so the reader can leave
        // the results the way they came. Every refinement after it replaces.
        { replace: q !== "" },
      );
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [request.q, text, setParams]);

  /** Apply a facet. The URL is the only state, so this is the whole of it. */
  function apply(next: FacetChange) {
    const updated = new URLSearchParams(params);
    if (next.domains !== undefined) {
      write(updated, "domains", next.domains.join(","));
    }
    if (next.type !== undefined) {
      write(updated, "type", next.type ?? "");
    }
    if (next.status !== undefined) {
      write(updated, "status", next.status ?? "");
    }
    if (next.tags !== undefined) {
      write(updated, "tags", next.tags.join(","));
    }
    if (next.after !== undefined) {
      write(updated, "after", next.after ?? "");
    }
    if (next.mode !== undefined) {
      // The default is not written: a URL says what was chosen, and hybrid is
      // what a search is when nobody chose.
      write(updated, "search_type", modeParam(next.mode));
    }
    setParams(updated);
  }

  const listing = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });
  // The vocabulary endpoint takes one domain or none, so the tags on offer are
  // that domain's when exactly one is filtered on, and the instance's
  // otherwise. Either way the chips are the real set for what is being
  // searched rather than a guess assembled from a page of results.
  const scope =
    request.domains.length === 1 ? (request.domains[0] ?? null) : null;
  const tags = useQuery({
    queryKey: vocabularyKey(scope),
    queryFn: () => fetchTags(scope),
  });

  const searchable = isSearchable(request);
  const filtering = hasSearchFilters(request);

  return (
    <div className="flex flex-col gap-6">
      <header>
        <h1 className="text-display">Search</h1>
      </header>

      <form
        role="search"
        onSubmit={(event) => {
          // Submitting is only ever "run it now": the pause already does this,
          // and a page reload would throw the results away.
          event.preventDefault();
        }}
      >
        <label
          htmlFor="search-query"
          className="mb-1 block text-xs text-slate-500 dark:text-slate-400"
        >
          Search query
        </label>
        <input
          id="search-query"
          type="search"
          value={text}
          placeholder="What do you want to remember?"
          onChange={(event) => {
            setText(event.target.value);
          }}
          className="w-full rounded border border-slate-300 bg-white px-3 py-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 dark:border-slate-700 dark:bg-slate-900"
        />
      </form>

      <Facets
        request={request}
        domains={listing.data?.domains.map((domain) => domain.name) ?? []}
        tags={tags.data ?? []}
        onChange={apply}
      />

      {searchable ? (
        <EngramList
          queryKey={searchKey(request)}
          loadPage={(page) => fetchSearch(request, page)}
          label="Search results"
          emptyMessage={
            filtering
              ? "No engram matches this search with these filters on. An empty answer under a filter is an answer: try clearing one."
              : "No engram matches this search."
          }
          highlight={terms}
          // A sweep of more than one domain has to say which one answered; a
          // search already scoped to one would repeat its name on every row.
          showDomain={scope === null}
          summary={(page, shown) => (
            <StatusLine page={page} shown={shown} asked={request.mode} />
          )}
          emptyActions={
            filtering ? (
              <>
                <button
                  type="button"
                  className={BUTTON.secondary}
                  onClick={() => {
                    // The query survives: widening a search is not ending it.
                    apply({
                      domains: [],
                      type: null,
                      status: null,
                      tags: [],
                      after: null,
                    });
                  }}
                >
                  Clear filters
                </button>
                {request.domains.length > 0 && (
                  <button
                    type="button"
                    className={BUTTON.secondary}
                    onClick={() => {
                      apply({ domains: [] });
                    }}
                  >
                    Search all domains
                  </button>
                )}
              </>
            ) : undefined
          }
        />
      ) : (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          Nothing has been searched for yet. Type a query, or turn on a filter
          to sweep every domain by frontmatter alone.
        </p>
      )}
    </div>
  );
}

/**
 * How many results there are and what ranked them, on one line.
 *
 * One line rather than the two this used to stack: a tally and a mode note are
 * both facts about the page, and a reader scanning for "did this find
 * anything" should not have to read two sentences to learn it. The mode is
 * always on it rather than only on a fallback, because "this was ranked by
 * text" is what explains the order of the results, and a reader who only ever
 * sees the note when something went wrong learns to read it as an error.
 *
 * An empty page counts nothing - the empty message below it already says there
 * is nothing, and "0 results" ahead of that would be the same sentence twice -
 * but it still says what ranked the search, as a sentence of its own rather
 * than as the clause that follows a count. Which mode ran is exactly what
 * explains an empty answer to a reader whose words were right.
 */
function StatusLine({
  page,
  shown,
  asked,
}: {
  page: EngramPage;
  shown: number;
  asked: SearchMode;
}) {
  const ran = page.mode;
  // The total survives the merge: a page of fifty out of two hundred is a fact
  // about this search that nothing else on the screen says.
  const counted =
    shown < page.total
      ? `${String(shown)} of ${String(page.total)} results`
      : plural(page.total, "result", "results");
  const line = statusText(ran, asked, shown === 0 ? null : counted);
  if (line === null) {
    return null;
  }
  return (
    <p className="text-caption pb-2 text-slate-500 tabular-nums dark:text-slate-400">
      {line}
    </p>
  );
}

/**
 * The status line's own words, given what ran, what was asked for, and the
 * tally when there is one to give. Null when there is nothing to say at all.
 *
 * Split out from the component because every branch of it is a sentence
 * somebody reads: with a count the mode is the clause that finishes it, and
 * without one the mode has to stand up on its own.
 */
function statusText(
  ran: string | null,
  asked: SearchMode,
  counted: string | null,
): string | null {
  if (ran === null) {
    return counted === null ? null : `${counted}.`;
  }
  if (ran !== asked) {
    const explained = `Asked for ${asked}, ran as ${ran}: ${asked} needs embeddings and a query to embed them against, so the engine fell back.`;
    return counted === null ? explained : `${counted}. ${explained}`;
  }
  return counted === null
    ? `Ranked by ${ran}.`
    : `${counted}, ranked by ${ran}.`;
}

/** A comma list from the URL, without the empties. */
function csv(value: string | null): string[] {
  return (value ?? "").split(",").filter((entry) => entry !== "");
}

/** The param a mode is written as, which is nothing for the default. */
function modeParam(mode: SearchMode): string {
  return mode === DEFAULT_SEARCH_MODE ? "" : mode;
}

/** Set a param, or drop it when there is nothing to say. */
function write(params: URLSearchParams, key: string, value: string): void {
  if (value === "") {
    params.delete(key);
  } else {
    params.set(key, value);
  }
}
