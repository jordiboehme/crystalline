/**
 * What a save is close to.
 *
 * The server answers a create or a save with the nearest existing engrams by
 * meaning and one instruction on what to do about them (`SIMILAR_GUIDANCE`,
 * the server's own words: read the one that fits, and then merge, supersede,
 * link, or nothing). Nothing here decides anything - the person reads, and
 * the four verbs the guidance names are theirs to take, not this panel's.
 *
 * Dismiss is local state on the editor rather than anything the server is
 * told: the next save asks again, and a dismissed panel from one save never
 * suppresses the next one's advisory.
 *
 * Renders nothing when `similar` is empty, which is also how a quiet receipt
 * (the keys absent, per `readEngramDetail`) reaches here - the editor only
 * ever hands this component a non-null advisory when there is something to
 * show.
 */

import { X } from "lucide-react";
import { Link } from "react-router";

import type { SimilarEngram } from "../api/engram";
import { engramRoute } from "../paths";
import { Chip, FOCUS_RING, IconButton, statusVariant } from "./primitives";

/**
 * The app's own link face: `CreateDomainDialogBody.tsx`'s "Connect GitHub in
 * settings" is where it is measured (accent-700 on white is 5.47:1,
 * accent-400 on slate-900 is 9.59:1), and every non-document link in the app
 * wears it rather than the browser default.
 */
const LINK = `text-accent-700 underline underline-offset-2 hover:no-underline dark:text-accent-400 ${FOCUS_RING}`;

export interface SimilarEngramsPanelProps {
  similar: SimilarEngram[];
  guidance: string | null;
  onDismiss: () => void;
}

export function SimilarEngramsPanel({
  similar,
  guidance,
  onDismiss,
}: SimilarEngramsPanelProps) {
  if (similar.length === 0) {
    return null;
  }
  return (
    <aside
      role="status"
      aria-label="Similar engrams"
      className="flex flex-col gap-2 rounded border border-accent-300 bg-accent-50 p-3 text-sm dark:border-accent-800 dark:bg-accent-950"
    >
      <div className="flex items-start justify-between gap-2">
        <h2 className="font-medium text-accent-900 dark:text-accent-100">
          Similar engrams
        </h2>
        <IconButton label="Dismiss" icon={X} onClick={onDismiss} />
      </div>
      {guidance !== null && (
        <p className="text-slate-700 dark:text-slate-300">{guidance}</p>
      )}
      <ul className="flex flex-col gap-1">
        {similar.map((entry) => (
          <li
            key={`${entry.domain}/${entry.permalink}`}
            className="flex flex-wrap items-center gap-2"
          >
            <Link
              to={engramRoute(entry.domain, entry.permalink)}
              className={LINK}
            >
              {entry.title}
            </Link>
            <span className="text-slate-500 dark:text-slate-400">
              {entry.domain}
            </span>
            {entry.status !== "" && (
              <Chip variant={statusVariant(entry.status)}>{entry.status}</Chip>
            )}
            {entry.type !== "" && <Chip>{entry.type}</Chip>}
          </li>
        ))}
      </ul>
    </aside>
  );
}
