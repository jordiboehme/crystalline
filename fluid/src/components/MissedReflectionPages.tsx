/**
 * The Reflection pages a reflect run missed (design 2.2): a run started after
 * the page fell due and ended without reconciling it. A page only overdue
 * because the loop runs less often than its freshness is not here: the loop
 * did not fail it, and the page itself says so.
 *
 * Two places say it: a banner an admin sees on every page (the first few,
 * with the rest one link away), and a section of the maintenance report
 * (every page). Each names the domain, the page and since when. Nothing is
 * drawn while no page was missed or when the read fails: a banner about a
 * problem must not become one.
 */

import { useQuery } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useId } from "react";
import { Link } from "react-router";

import type { MissedPage } from "../api/reflectionMissed";
import { MISSED_PAGES_KEY, fetchMissedPages } from "../api/reflectionMissed";
import { plural } from "../format";
import { maintenanceRoute, reflectionPageRoute } from "../paths";
import { WAITING_TEXT, checked, span } from "../reflectionClock";
import { FOCUS_RING } from "./primitives";

/** How many pages the banner names before it points to Maintenance. */
const BANNER_SHOWN = 5;

const LINK = `rounded underline underline-offset-[3px] hover:no-underline ${FOCUS_RING}`;

// eslint-disable-next-line react-refresh/only-export-components
export function missedLine(page: MissedPage): string {
  const since = `missed since ${span(page.missedSecs)}`;
  return page.waiting
    ? `${since}. ${WAITING_TEXT}`
    : `${since}, last ${checked(page.lastCheckedSecs)}`;
}

function useMissedPages() {
  return useQuery({
    queryKey: MISSED_PAGES_KEY,
    queryFn: fetchMissedPages,
    refetchInterval: 60_000,
    retry: false,
  });
}

function PageLink({ page }: { page: MissedPage }): ReactElement {
  return (
    <Link to={reflectionPageRoute(page.domain, page.path)} className={LINK}>
      {`${page.domain}: ${page.path}`}
    </Link>
  );
}

export function MissedPagesBanner(): ReactElement | null {
  const headingId = useId();
  const missed = useMissedPages();
  const pages = missed.data ?? [];
  if (pages.length === 0) {
    return null;
  }
  const rest = pages.length - BANNER_SHOWN;
  return (
    <div
      role="status"
      aria-labelledby={headingId}
      className="mb-4 flex flex-col gap-1 rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-200"
    >
      <p id={headingId} className="sr-only">
        Reflection pages
      </p>
      <p>
        {`A reflect run missed ${plural(pages.length, "Reflection page", "Reflection pages")}.`}
      </p>
      <ul className="flex flex-col gap-0.5">
        {pages.slice(0, BANNER_SHOWN).map((page) => (
          <li key={`${page.domain}\u0000${page.path}`}>
            <PageLink page={page} />
            {`, ${missedLine(page)}`}
          </li>
        ))}
      </ul>
      {rest > 0 && (
        <Link to={maintenanceRoute()} className={`self-start ${LINK}`}>
          {`${String(rest)} more in Maintenance`}
        </Link>
      )}
    </div>
  );
}

export function MissedPagesList(): ReactElement | null {
  const headingId = useId();
  const missed = useMissedPages();
  const pages = missed.data ?? [];
  if (pages.length === 0) {
    return null;
  }
  return (
    <section aria-labelledby={headingId} className="flex flex-col gap-2">
      <h2 id={headingId} className="text-section">
        Reflection pages a reflect run missed
      </h2>
      <p className="text-caption text-slate-500 dark:text-slate-400">
        A reflect run started after these pages fell due and ended without
        checking them. Run the reflect loop again, or look at what stops it.
      </p>
      <ul className="flex flex-col gap-1 text-sm">
        {pages.map((page) => (
          <li key={`${page.domain}\u0000${page.path}`}>
            <PageLink page={page} />
            <span className="text-slate-600 dark:text-slate-300">
              {`, ${missedLine(page)}. Freshness ${page.freshness}.`}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}
