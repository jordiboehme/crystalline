/**
 * What points at this engram.
 *
 * The source is the neighborhood graph rather than the detail payload's own
 * inbound block, which carries a count and a sample capped at five: a panel
 * built from the sample would quietly stop at five on exactly the engrams where
 * knowing who points here matters most. The graph at one hop is the whole set
 * within that hop, and it carries an address for each end, which the sample
 * does not.
 *
 * An engram nothing points at says so. "Nothing links here yet" is a fact about
 * the knowledge base and an invitation to connect it; an empty box would read
 * as a panel that failed to load.
 */

import { Link } from "react-router";

import { problemDetail } from "../api/client";
import type { Backlink } from "../api/graph";
import { RETIRED_CLASS, isRetired } from "../lifecycle";
import { engramRoute } from "../paths";

export interface BacklinksPanelProps {
  /** The engrams pointing here, already derived from the graph. */
  backlinks: Backlink[];
  /** Whether the graph request is still in flight. */
  pending: boolean;
  /** Why it failed, when it did. */
  error: Error | null;
  /** Whether the node cap cut the neighborhood short. */
  truncated: boolean;
}

export function BacklinksPanel({
  backlinks,
  pending,
  error,
  truncated,
}: BacklinksPanelProps) {
  return (
    <section
      aria-label="Backlinks"
      className="rounded border border-slate-200 px-4 py-3 dark:border-slate-800"
    >
      <h2 className="mb-2 text-sm font-semibold">Backlinks</h2>
      {pending ? (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          Looking for what points here
        </p>
      ) : error ? (
        <p
          role="alert"
          className="rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {/*
            What the reader lost, then the server's own words for why: the
            detail is the only part of this that says anything specific, and
            every other error surface in this app prints it verbatim.
          */}
          What points here is unknown: {problemDetail(error)}
        </p>
      ) : backlinks.length === 0 ? (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          Nothing links here yet.
        </p>
      ) : (
        <ul className="flex flex-col gap-2 text-sm">
          {backlinks.map(({ node, relTypes }) => (
            <li
              key={`${node.domain}/${node.permalink}`}
              className={isRetired(node.status) ? RETIRED_CLASS : undefined}
            >
              <Link
                to={engramRoute(node.domain, node.permalink)}
                // Named by what it points at, the way every engram link in
                // this app is, so a screen reader does not read out the
                // relation types as part of the name.
                aria-label={`${node.title}, ${node.permalink}`}
                className="text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
              >
                {node.title}
              </Link>
              {relTypes.length > 0 && (
                <span className="ml-2 text-xs text-slate-500 dark:text-slate-400">
                  {relTypes.join(", ")}
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
      {truncated && (
        <p className="mt-2 text-xs text-slate-500 dark:text-slate-400">
          The neighborhood was capped, so this may not be all of them.
        </p>
      )}
    </section>
  );
}
