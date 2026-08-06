/**
 * One reference to another engram, drawn the same way everywhere it appears.
 *
 * Three surfaces show references - the prose in the body, the relation list and
 * the lifecycle banner's supersedes chain - and they have to agree about what
 * each of the three states looks like. The body builds hast nodes rather than
 * elements so it cannot share this component itself, but it shares the rule
 * underneath (`referenceState` in `wikilinks.ts`) and matches the presentation
 * here.
 *
 * The middle state is why this exists. A reference the index resolved but the
 * graph has not placed yet is named plainly: linking it would be a guess about
 * where it goes, and marking it unresolved would be a false claim that would
 * flicker into a link a moment later. On an engram page the graph is asked for
 * only once the detail has landed, so that window is the ordinary load path
 * rather than an edge case.
 */

import { Link } from "react-router";

import type { ReferenceState } from "../wikilinks";

export interface ReferenceLinkProps {
  /** What to call it: the target's title, or the target as it was written. */
  label: string;
  /** Where it goes, known only in the resolved state. */
  href: string | null;
  /** Which of the three states it is in. */
  state: ReferenceState;
}

export function ReferenceLink({ label, href, state }: ReferenceLinkProps) {
  if (state === "resolved" && href !== null) {
    return (
      <Link
        to={href}
        className="text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
      >
        {label}
      </Link>
    );
  }
  if (state === "unresolved") {
    return (
      <span
        title="not resolved"
        className="underline decoration-dotted underline-offset-2 opacity-70"
      >
        {label}
      </span>
    );
  }
  // Pending, or resolved without an address, which is the same thing to a
  // reader: named, with nothing claimed about it either way.
  return <span>{label}</span>;
}
