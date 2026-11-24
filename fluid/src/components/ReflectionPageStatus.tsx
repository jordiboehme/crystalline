/**
 * The lines under a Reflection page's title about how it keeps itself
 * current (design 2.1 to 2.3):
 *
 * - "Follows: PACT.md > Architecture overview": the pacts the page follows,
 *   top down, each step a link;
 * - "Part of: Architecture overview" for a page another page owns;
 * - the freshness line from the page clock (`reflectionClock.ts`), in the
 *   warning colour while the page is overdue, with the hint when the loop
 *   runs less often than the page's freshness, and a note when a reflect
 *   run skipped the page;
 * - for a curator, why an owned page is not checked on a schedule.
 *
 * Not part of the page's text: nothing to select or comment on.
 */

import type { ReactElement } from "react";
import { useEffect, useState } from "react";
import { Link } from "react-router";

import type { PageClock, PageView } from "../api/reflectionPage";
import { PACT_PATH } from "../api/reflectionDomains";
import { reflectionPageRoute } from "../paths";
import {
  freshnessLine,
  noFreshnessNote,
  skippedNote,
} from "../reflectionClock";
import { CHROME_LINK, FOCUS_RING } from "./primitives";

const STEP = `rounded underline-offset-[3px] ${CHROME_LINK} ${FOCUS_RING}`;

export function ReflectionPageStatus({
  domain,
  page,
  clock,
  readAt,
  curator,
}: {
  domain: string;
  page: PageView;
  /** The page clock; null while it loads and for a page that has none. */
  clock: PageClock | null;
  /**
   * When the clock was read (ms since the epoch, the query's
   * `dataUpdatedAt`); 0 before it was.
   */
  readAt: number;
  curator: boolean;
}): ReactElement {
  // The clock's seconds are relative to its read: a minute tick here ages
  // them, asks nothing of the server and redraws only these lines.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => {
      setNow(Date.now());
    }, 60_000);
    return () => {
      clearInterval(id);
    };
  }, []);
  const elapsed =
    readAt === 0 ? 0 : Math.max(0, Math.floor((now - readAt) / 1000));
  const line = page.retired === null ? freshnessLine(clock, elapsed) : null;
  const skipped = page.retired === null ? skippedNote(clock) : null;
  const unscheduled =
    curator && page.retired === null ? noFreshnessNote(clock) : null;
  return (
    <div
      data-anchor-skip=""
      data-testid="page-status"
      className="flex flex-col gap-0.5 text-sm text-ink-3 select-none print:hidden"
    >
      <p>
        Follows:{" "}
        <Link to={reflectionPageRoute(domain, PACT_PATH)} className={STEP}>
          PACT.md
        </Link>
        {page.chain.map((step) => (
          <span key={step.path}>
            {" > "}
            <Link to={reflectionPageRoute(domain, step.path)} className={STEP}>
              {step.title}
            </Link>
          </span>
        ))}
      </p>
      {page.owner !== null && page.retired === null && (
        <p>
          Part of:{" "}
          {page.owner.exists ? (
            <Link
              to={reflectionPageRoute(domain, page.owner.path)}
              className={STEP}
            >
              {page.owner.title}
            </Link>
          ) : (
            page.owner.title
          )}
        </p>
      )}
      {line !== null && (
        <p
          className={
            line.kind === "overdue"
              ? "font-medium text-amber-800 dark:text-amber-300"
              : undefined
          }
        >
          {line.text}
        </p>
      )}
      {line?.kind === "overdue" && line.hint !== null && <p>{line.hint}</p>}
      {skipped !== null && <p>{skipped}</p>}
      {unscheduled !== null && <p>{unscheduled}</p>}
    </div>
  );
}
