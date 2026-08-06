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

import { ApiProblem } from "../api/client";
import type { EngramPage, EngramRow } from "../api/engrams";
import { hasNextPage as envelopeHasNext } from "../api/engrams";
import { RETIRED_CLASS, isRetired } from "../lifecycle";
import { engramRoute } from "../paths";
import { snippetParts } from "../snippet";

/** How tall one row is, in pixels. The tests scroll by it, so it is exported. */
export const ENGRAM_ROW_HEIGHT = 76;

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
   * What the envelope itself says, drawn above the rows. Search uses it for the
   * mode that actually ran, which is a fact about the page rather than about
   * any row, and which matters just as much when the page is empty.
   */
  summary?: (page: EngramPage) => ReactNode;
}

export function EngramList({
  queryKey,
  loadPage,
  label,
  emptyMessage,
  highlight = [],
  summary,
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
  const total = data?.pages[data.pages.length - 1].total ?? rows.length;

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
        {envelope && summary?.(envelope)}
        <p className="py-6 text-sm text-slate-500 dark:text-slate-400">
          {emptyMessage}
        </p>
      </div>
    );
  }

  return (
    <div>
      {envelope && summary?.(envelope)}
      <p className="pb-2 text-xs text-slate-500 tabular-nums dark:text-slate-400">
        {rows.length} of {total} shown
      </p>
      <div
        ref={scroller}
        className="h-[60vh] min-h-64 overflow-y-auto rounded border border-slate-200 dark:border-slate-800"
      >
        <ul
          aria-label={label}
          className="relative w-full"
          style={{ height: `${String(virtualizer.getTotalSize())}px` }}
        >
          {drawn.map((item) => (
            <Row
              key={`${rows[item.index].domain}/${rows[item.index].permalink}`}
              row={rows[item.index]}
              offset={item.start}
              highlight={highlight}
            />
          ))}
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
 */
function Row({
  row,
  offset,
  highlight,
}: {
  row: EngramRow;
  offset: number;
  highlight: string[];
}) {
  const retired = isRetired(row.status);
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
        // Named by what it points at. Without this the name is every badge on
        // the row run together, which is what a screen reader would read out
        // for each of a hundred rows.
        aria-label={`${row.title}, ${row.permalink}`}
        className="flex h-full flex-col justify-center gap-1 rounded px-3 py-2 hover:bg-slate-50 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:hover:bg-slate-900"
      >
        <span className="flex items-baseline gap-2">
          <span className="truncate font-medium">{row.title}</span>
          <span className="truncate font-mono text-xs text-slate-500 dark:text-slate-400">
            {row.permalink}
          </span>
        </span>
        <span className="flex items-center gap-1.5 overflow-hidden text-xs whitespace-nowrap">
          {row.type !== null && <Badge>{row.type}</Badge>}
          {row.status !== null && (
            <Badge title={retired ? "A retired status" : undefined}>
              {row.status}
            </Badge>
          )}
          {row.line !== null && (
            <span className="text-slate-500 tabular-nums dark:text-slate-400">
              line {row.line}
            </span>
          )}
          {row.tags.map((tag) => (
            <span key={tag} className="text-slate-500 dark:text-slate-400">
              #{tag}
            </span>
          ))}
          {row.snippet !== null && (
            <span className="truncate text-slate-500 dark:text-slate-400">
              <Snippet text={row.snippet} highlight={highlight} />
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
 */
function Snippet({ text, highlight }: { text: string; highlight: string[] }) {
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

function Badge({ children, title }: { children: ReactNode; title?: string }) {
  return (
    <span
      title={title}
      className="rounded bg-slate-100 px-1.5 py-0.5 text-slate-600 dark:bg-slate-800 dark:text-slate-300"
    >
      {children}
    </span>
  );
}

/**
 * A failed page, said out loud where the rows would have been. Inline rather
 * than a redirect, for the reason the sidebar's is: a refusal mid-session is an
 * answer about this list, not a reason to bounce a reader to a login form.
 */
function ListProblem({ error }: { error: Error }) {
  const detail = error instanceof ApiProblem ? error.detail : error.message;
  return (
    <p
      role="alert"
      className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
    >
      {detail}
    </p>
  );
}
