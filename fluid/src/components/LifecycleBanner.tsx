/**
 * What an engram's lifecycle says about how to read it.
 *
 * Two facts earn a line here and nothing else does.
 *
 * A retired status (`deprecated`, `superseded`, `archived`, `legacy`) means the
 * engram is kept for the record rather than as current knowledge. It is never
 * hidden - hiding it would misrepresent what the domain holds - so it is
 * announced instead, together with the supersedes chain, which is the way out
 * of it: `superseded_by` points at what replaced this, `supersedes` at what
 * this replaced.
 *
 * A `stale_after` that has passed means the knowledge is due for a check. That
 * is a different claim from being retired and gets its own line, because an
 * engram can be either, both, or neither.
 *
 * Everything else is silence. Absent `valid_from` means the knowledge has
 * always been valid and absent `valid_to` means it is valid forever, so an
 * engram carrying no dates renders nothing at all rather than a placeholder
 * date somebody, or some agent, would carry away as a fact.
 */

import { Link } from "react-router";

import { hasArrived, localDay } from "../format";
import { isRetired } from "../lifecycle";

/** One end of the supersedes chain, as far as the index could follow it. */
export interface LifecycleLink {
  /** What to call it: the target's title, or the target as it was written. */
  label: string;
  /** Where it lives, or null when nothing on this instance resolved it. */
  href: string | null;
}

export interface LifecycleBannerProps {
  /** The engram's `status` frontmatter, free form. */
  status: string | null;
  /** Its `stale_after` date, or null when it carries none. */
  staleAfter: string | null;
  /** What replaced it, from its `superseded_by` relations. */
  supersededBy: LifecycleLink[];
  /** What it replaced, from its `supersedes` relations. */
  supersedes: LifecycleLink[];
  /** Today, as `YYYY-MM-DD`. Defaults to the day this browser is having. */
  today?: string;
}

export function LifecycleBanner({
  status,
  staleAfter,
  supersededBy,
  supersedes,
  today = localDay(),
}: LifecycleBannerProps) {
  const retired = isRetired(status);
  const stale = staleAfter !== null && hasArrived(staleAfter, today);
  if (!retired && !stale) {
    return null;
  }

  return (
    <div className="flex flex-col gap-2">
      {retired && (
        <aside
          // A statement about what is on screen rather than an alert about
          // something that just happened: nothing here interrupts a reader.
          role="note"
          className="rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100"
        >
          <p>
            This engram is <strong>{status}</strong>, so it is kept for the
            record rather than as current knowledge.
          </p>
          <Chain label="Superseded by" links={supersededBy} />
          <Chain label="Supersedes" links={supersedes} />
        </aside>
      )}
      {stale && (
        <aside
          role="status"
          className="rounded border border-slate-300 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-200"
        >
          Due for a check: it was marked stale after {staleAfter}.
        </aside>
      )}
    </div>
  );
}

/**
 * One direction of the chain, drawn only when it leads somewhere.
 *
 * A target the index did not resolve is named and not linked. The engram says
 * it was superseded by something, which is worth showing; nothing on this
 * instance answers to that name, and a link that goes nowhere would say
 * otherwise.
 */
function Chain({ label, links }: { label: string; links: LifecycleLink[] }) {
  if (links.length === 0) {
    return null;
  }
  return (
    <p className="mt-1 flex flex-wrap items-baseline gap-x-2">
      <span>{label}</span>
      {links.map((link) =>
        link.href === null ? (
          <span
            key={link.label}
            title="Nothing on this instance resolves this target"
            className="underline decoration-dotted underline-offset-2"
          >
            {link.label}
          </span>
        ) : (
          <Link
            key={link.label}
            to={link.href}
            className="font-medium underline underline-offset-2 hover:no-underline"
          >
            {link.label}
          </Link>
        ),
      )}
    </p>
  );
}
