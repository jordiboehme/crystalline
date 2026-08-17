/**
 * What points at this engram, counted by relation.
 *
 * One chip per relation type, each carrying its count and opening onto a
 * searchable, paged list of the engrams that point here that way. The panel
 * itself never holds the references: an engram a thousand engrams point at is
 * two or three chips on the page, and the rows arrive only for the relation
 * somebody clicked.
 *
 * That is why it no longer reads the neighborhood graph. The graph is capped at
 * a hundred and fifty nodes server-side, so a panel drawn from it silently
 * stopped at the cap on exactly the engrams where knowing who points here
 * matters most, and the detail payload's own inbound block is a sample capped
 * at five. The counts here are of the whole index, under a query that never
 * loads it.
 *
 * The first paint costs no request at all when nothing points here: the detail
 * payload already carries the exact whole-index `inboundCount`, so a zero is
 * answered from what the page has already read and the summary is only asked
 * for when there is something to summarize.
 *
 * An engram nothing points at says so. "Nothing links here yet" is a fact about
 * the knowledge base and an invitation to connect it; an empty box would read
 * as a panel that failed to load.
 */

import { useQuery } from "@tanstack/react-query";
import { Suspense, lazy, useCallback } from "react";

import { problemDetail } from "../api/client";
import type { InboundRefPage } from "../api/inbound";
import {
  INBOUND_PAGE_SIZE,
  fetchInbound,
  fetchInboundSummary,
  inboundSummaryKey,
} from "../api/inbound";
import { RETIRED_CLASS, isRetired } from "../lifecycle";
import { engramRoute } from "../paths";
import type { RefPageResult } from "./RefPopover";

/**
 * The chip and its floating surface, on a chunk of their own.
 *
 * Split off because the popover is the app's first use of a floating panel
 * that is not a menu, and its positioning engine is about eight and a half
 * kilobytes the initial download would otherwise carry on every screen,
 * including the ones with no references on them at all. Nothing is lost by
 * waiting: the chips cannot be drawn before the summary request answers, and
 * the chunk is fetched in parallel with it.
 */
const RefPopover = lazy(async () => ({
  default: (await import("./RefPopover")).RefPopover,
}));

export interface BacklinksPanelProps {
  /** The engram being read. */
  domain: string;
  permalink: string;
  /**
   * How many references the detail payload counted, across the whole index.
   * Zero is answered without a request.
   */
  inboundCount: number;
}

/** One page of inbound references, as the popover primitive takes them. */
function asPage(page: InboundRefPage): RefPageResult {
  return {
    total: page.total,
    rows: page.hits.map((hit, index) => ({
      // Unique across the pages a popover accumulates, and stable within one:
      // the same engram may point here twice with the same relation, so the
      // address alone is not a key.
      key: `${String(page.page)}:${String(index)}:${hit.domain}/${hit.permalink}`,
      title: hit.title,
      href: engramRoute(hit.domain, hit.permalink),
      detail: `${hit.domain} / ${hit.path || hit.permalink}`,
      className: isRetired(hit.status) ? RETIRED_CLASS : undefined,
    })),
    hasMore: page.page * page.limit < page.total,
  };
}

export function BacklinksPanel({
  domain,
  permalink,
  inboundCount,
}: BacklinksPanelProps) {
  const summary = useQuery({
    queryKey: inboundSummaryKey(domain, permalink),
    queryFn: () => fetchInboundSummary(domain, permalink),
    // Nothing points here, and the detail payload already said so exactly.
    enabled: inboundCount > 0,
  });

  const pageFor = useCallback(
    (rel: string) => (page: number, q: string) =>
      fetchInbound(domain, permalink, {
        rel,
        q,
        page,
        limit: INBOUND_PAGE_SIZE,
      }).then(asPage),
    [domain, permalink],
  );

  const types = summary.data?.types ?? [];
  return (
    <section
      aria-label="Backlinks"
      className="rounded border border-slate-200 px-4 py-3 dark:border-slate-800"
    >
      <h2 className="mb-2 text-sm font-semibold">Backlinks</h2>
      {inboundCount > 0 && summary.isPending ? (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          Looking for what points here
        </p>
      ) : summary.error ? (
        <p
          role="alert"
          className="rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {/*
            What the reader lost, then the server's own words for why: the
            detail is the only part of this that says anything specific, and
            every other error surface in this app prints it verbatim.
          */}
          What points here is unknown: {problemDetail(summary.error)}
        </p>
      ) : types.length === 0 ? (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          Nothing links here yet.
        </p>
      ) : (
        <ul className="flex flex-wrap gap-2">
          {types.map((entry) => (
            <li key={entry.rel}>
              {/*
                No fallback: the chunk lands in the same breath as the summary
                that named the chips, and a placeholder chip that then swapped
                for a real one would be a flicker rather than an answer.
              */}
              <Suspense fallback={null}>
                <RefPopover
                  label={entry.rel}
                  count={entry.count}
                  fetchPage={pageFor(entry.rel)}
                />
              </Suspense>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
