/**
 * The list of engrams every screen that shows more than one of them is built
 * from: a domain's folder, a domain's filtered listing, a page of search
 * results.
 *
 * It owns two things its callers should not have to repeat. Paging, because
 * every one of those sources answers with the same envelope and a reader
 * reaching the bottom means the same thing on all of them; and virtualization,
 * because nothing in this app is allowed to scroll an unvirtualized list. What
 * it does not own is where the rows come from: a caller hands it a loader for
 * one page and the cache key that loader belongs to.
 *
 * Rows are a fixed height on purpose. A measured row would let a long title
 * push the list around while a reader scrolls it, and the row is built to
 * truncate instead: everything a row says is one line of it.
 */

import { useInfiniteQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useMemo, useRef } from "react";
import type { ReactNode } from "react";
import { Link } from "react-router";

import { problemDetail } from "../api/client";
import type { EngramPage, EngramRow } from "../api/engrams";
import { hasNextPage as envelopeHasNext } from "../api/engrams";
import { RETIRED_CLASS, isRetired } from "../lifecycle";
import { engramRoute } from "../paths";
import { ENGRAM_PREFETCH } from "../prefetch";
import { snippetParts, stripSnippetMarkup } from "../snippet";
import { Chip, statusVariant } from "./primitives";

/** How tall one row is, in pixels. The tests scroll by it, so it is exported. */
export const ENGRAM_ROW_HEIGHT = 76;

/**
 * How many of a row's tags are drawn before the rest become a count.
 *
 * The row has one line for everything under the title, and the snippet is the
 * reason the row is there at all: on a well-tagged engram an uncapped tag list
 * ate that line and left the reader a row that never said what matched. Two
 * tags are a hint at what this is; the rest are a hover away.
 */
const MAX_ROW_TAGS = 2;

/** How many rows beyond the viewport are drawn, so a flick does not flash. */
const OVERSCAN = 4;

export interface EngramListProps {
  /** The cache key these pages live under. Changing it starts a new list. */
  queryKey: readonly unknown[];
  /** Fetch one page, one based. */
  loadPage: (page: number) => Promise<EngramPage>;
  /** What this list is, for the reader and for the accessibility tree. */
  label: string;
  /** What to say when the source answers with nothing. */
  emptyMessage: string;
  /**
   * Words to mark inside a row's snippet: the terms of the query that produced
   * these rows. Empty for a list nobody searched for.
   */
  highlight?: string[];
  /**
   * What the envelope itself says, drawn above the rows, given the envelope and
   * how many rows are in hand across the pages loaded so far. Search uses it for
   * the mode that actually ran, which is a fact about the page rather than about
   * any row, and which matters just as much when the page is empty.
   *
   * A caller that gives one owns the whole status line: the count this list
   * would otherwise draw is its to say, because two lines stacked above the
   * rows saying near enough the same thing is what that replaced.
   */
  summary?: (page: EngramPage, shown: number) => ReactNode;
  /**
   * The way out of an empty answer, drawn under the empty message. Only a
   * caller knows whether there is one: an empty search under a filter can be
   * widened, and an empty folder cannot.
   */
  emptyActions?: ReactNode;
  /**
   * Whether a row says which domain it lives in, on the front of its
   * permalink. Off by default, because a list of one domain's engrams would
   * repeat that domain on every row; a search that swept several has to say it.
   */
  showDomain?: boolean;
}

export function EngramList({
  queryKey,
  loadPage,
  label,
  emptyMessage,
  highlight = [],
  summary,
  emptyActions,
  showDomain = false,
}: EngramListProps) {
  const scroller = useRef<HTMLDivElement>(null);

  const query = useInfiniteQuery({
    queryKey,
    queryFn: ({ pageParam }) => loadPage(pageParam),
    initialPageParam: 1,
    getNextPageParam: (last: EngramPage) =>
      envelopeHasNext(last) ? last.page + 1 : undefined,
  });
  const { data, hasNextPage, isFetchingNextPage, fetchNextPage } = query;

  const rows = useMemo(
    () => data?.pages.flatMap((page) => page.hits) ?? [],
    [data],
  );
  const total = data?.pages[data.pages.length - 1]?.total ?? rows.length;

  // The virtualizer hands back functions rather than values, which the React
  // Compiler cannot memoize, so it would skip this component. That is the
  // right trade here and it costs nothing today: the compiler is not enabled
  // in this app's Vite config, and the list re-renders on scroll by design.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => ENGRAM_ROW_HEIGHT,
    overscan: OVERSCAN,
  });
  const drawn = virtualizer.getVirtualItems();

  // Reaching the last row is the request for the next page. Watching the drawn
  // window rather than a scroll handler is what makes that true whatever moved
  // it: a wheel, a keyboard, or a row that was removed under the reader.
  useEffect(() => {
    const last = drawn[drawn.length - 1];
    if (!last || rows.length === 0) {
      return;
    }
    if (last.index >= rows.length - 1 && hasNextPage && !isFetchingNextPage) {
      void fetchNextPage();
    }
  }, [drawn, rows.length, hasNextPage, isFetchingNextPage, fetchNextPage]);

  if (query.isPending) {
    return (
      <p className="py-6 text-sm text-slate-500 dark:text-slate-400">
        Loading engrams
      </p>
    );
  }

  if (query.error) {
    return <ListProblem error={query.error} />;
  }

  const envelope = data?.pages[0];

  if (rows.length === 0) {
    return (
      <div>
        {envelope && summary?.(envelope, 0)}
        <p className="py-6 text-sm text-slate-500 dark:text-slate-400">
          {emptyMessage}
        </p>
        {emptyActions && (
          <div className="flex flex-wrap gap-2">{emptyActions}</div>
        )}
      </div>
    );
  }

  return (
    <div>
      {envelope && summary?.(envelope, rows.length)}
      {summary === undefined && (
        <p className="text-caption pb-2 text-slate-500 tabular-nums dark:text-slate-400">
          {rows.length} of {total} shown
        </p>
      )}
      {/*
        The box hugs what is in it and only starts scrolling once there is
        more than a screenful: a fixed 60vh left three results sitting in a
        tall empty frame. The inner list carries its own measured height, so
        capping rather than fixing is all the virtualizer needs.
      */}
      <div
        ref={scroller}
        className="max-h-[60vh] overflow-y-auto rounded border border-slate-200 dark:border-slate-800"
      >
        <ul
          aria-label={label}
          className="relative w-full"
          style={{ height: `${String(virtualizer.getTotalSize())}px` }}
        >
          {drawn.map((item) => {
            const row = rows[item.index];
            // The virtualizer only ever hands back indices within `rows`;
            // this guards the type honestly rather than trusting that.
            if (!row) {
              return null;
            }
            return (
              <Row
                key={`${row.domain}/${row.permalink}`}
                row={row}
                offset={item.start}
                highlight={highlight}
                showDomain={showDomain}
              />
            );
          })}
        </ul>
      </div>
      {isFetchingNextPage && (
        <p className="pt-2 text-xs text-slate-500 dark:text-slate-400">
          Loading more
        </p>
      )}
    </div>
  );
}

/**
 * One row.
 *
 * Everything the row knows is on it: what it is called, where it lives, what
 * kind of thing it is, what state it is in and what it is tagged with. A
 * retired one is faded and still readable, which is the whole point of fading
 * rather than filtering.
 *
 * The second line is ordered by what earns the width. The snippet is the reason
 * a searched row is on screen, so it takes what is left of the line and
 * truncates last; the tags are a hint rather than the answer, so they are
 * capped and sit after it. Reading order is chips, line number, match, tags.
 */
function Row({
  row,
  offset,
  highlight,
  showDomain,
}: {
  row: EngramRow;
  offset: number;
  highlight: string[];
  showDomain: boolean;
}) {
  const retired = isRetired(row.status);
  const path = showDomain ? `${row.domain}/${row.permalink}` : row.permalink;
  const tags = row.tags.slice(0, MAX_ROW_TAGS);
  const overflow = row.tags.length - tags.length;
  return (
    <li
      className={`absolute top-0 left-0 w-full px-1 ${retired ? RETIRED_CLASS : ""}`}
      style={{
        height: `${String(ENGRAM_ROW_HEIGHT)}px`,
        transform: `translateY(${String(offset)}px)`,
      }}
    >
      <Link
        to={engramRoute(row.domain, row.permalink)}
        {...ENGRAM_PREFETCH}
        // Named by what it points at. Without this the name is every badge on
        // the row run together, which is what a screen reader would read out
        // for each of a hundred rows.
        aria-label={`${row.title}, ${path}`}
        className="flex h-full flex-col justify-center gap-1 rounded px-3 py-2 hover:bg-slate-50 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:hover:bg-slate-900"
      >
        <span className="flex items-baseline gap-2">
          <span className="truncate font-medium">
            <Marked text={row.title} highlight={highlight} />
          </span>
          <span className="truncate font-mono text-xs text-slate-500 dark:text-slate-400">
            {path}
          </span>
        </span>
        <span className="flex items-center gap-1.5 overflow-hidden text-xs whitespace-nowrap">
          {row.type !== null && <Chip>{row.type}</Chip>}
          {row.status !== null && (
            <span {...(retired ? { title: "A retired status" } : {})}>
              <Chip variant={statusVariant(row.status)}>{row.status}</Chip>
            </span>
          )}
          {row.line !== null && (
            <span className="text-slate-500 tabular-nums dark:text-slate-400">
              line {row.line}
            </span>
          )}
          {row.snippet !== null && (
            // The one thing on the line that is allowed to take what is left
            // of it: everything beside it is sized by its own content.
            <span className="min-w-0 flex-1 truncate text-slate-500 dark:text-slate-400">
              <Snippet text={row.snippet} highlight={highlight} />
            </span>
          )}
          {tags.map((tag) => (
            <span
              key={tag}
              className="shrink-0 text-slate-500 dark:text-slate-400"
            >
              #{tag}
            </span>
          ))}
          {overflow > 0 && (
            // The whole list is in the tooltip: a native title is the entire
            // overflow affordance, and a row does not open a popover.
            <span
              title={row.tags.join(" ")}
              className="shrink-0 text-slate-500 dark:text-slate-400"
            >
              +{overflow}
            </span>
          )}
        </span>
      </Link>
    </li>
  );
}

/**
 * The matched text of a row, with the searched-for words marked.
 *
 * The snippet is text the engine cut out of an engram and it is rendered as
 * text: the pieces become elements here, and nothing turns a server string into
 * markup. An engram that talks about `<script>` reads the way it was written.
 *
 * The window was cut out of the file, though, so it arrives wearing whatever
 * markdown syntax it crossed. That comes back out first, and the terms are
 * matched against what is left, so the mark lands on the word the reader sees.
 */
function Snippet({ text, highlight }: { text: string; highlight: string[] }) {
  return <Marked text={stripSnippetMarkup(text)} highlight={highlight} />;
}

/**
 * Plain text with the searched-for words marked.
 *
 * The title is marked the same way the snippet is, and by the same code: a
 * reader scanning a page of results is looking for their own words, and a
 * result whose title is the match had been the one row that never said so.
 * Titles arrive as text rather than as a cut of a file, so nothing is stripped
 * out of them first.
 */
function Marked({ text, highlight }: { text: string; highlight: string[] }) {
  if (highlight.length === 0) {
    return text;
  }
  return snippetParts(text, highlight).map((part, index) =>
    part.match ? (
      // Keyed by position: the pieces are a cut of one string, so a piece is
      // only ever itself and the list never reorders.
      <mark
        key={index}
        className="bg-amber-100 text-inherit dark:bg-amber-900/60"
      >
        {part.text}
      </mark>
    ) : (
      <span key={index}>{part.text}</span>
    ),
  );
}

/**
 * A failed page, said out loud where the rows would have been. Inline rather
 * than a redirect, for the reason the sidebar's is: a refusal mid-session is an
 * answer about this list, not a reason to bounce a reader to a login form.
 */
function ListProblem({ error }: { error: Error }) {
  const detail = problemDetail(error);
  return (
    <p
      role="alert"
      className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
    >
      {detail}
    </p>
  );
}
