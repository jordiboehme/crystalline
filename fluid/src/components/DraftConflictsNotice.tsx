/**
 * This session's own drafts that do not fit the team's text, on the domain
 * page, and the drafts a merge changed since they last settled.
 *
 * It reads the draft routes and nothing else. Any writer may reach them, the
 * editor whose role cannot share included, where the sync status beside it
 * needs the share role: a person must see and settle her own conflict either
 * way. So this is mounted outside the team banners, which fold on a phone and
 * are only for a session that may share, and it is the domain page's warning
 * for her drafts: the sync card counts the folder's conflicts only.
 *
 * Each conflict is a path that opens the dialog that settles it, eight of
 * them and then a count, as the sync card draws the folder's. A merge is said
 * in plain words, one line per draft, until the draft ends. A list the server
 * could not read is said in the server's own sentence; a refusal that only
 * means "nothing for you here" (signed out, no right, no such domain) draws
 * nothing.
 */

import { useQuery } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useState } from "react";

import type { DraftConflict } from "../api/admin";
import { draftConflictsKey, fetchDraftConflicts } from "../api/admin";
import { ApiProblem, problemDetail } from "../api/client";
import { ConflictDialog } from "./ConflictDialog";
import { FOCUS_RING } from "./primitives";

/** How many paths the notice draws before it counts the rest. */
const SHOWN = 8;

/**
 * The caution pair the sync card's stale warning wears: amber-800 on
 * amber-100 is 6.41:1 and amber-300 on amber-950 is 10.37:1.
 */
const NOTICE_CLASSES =
  "flex flex-col gap-1 rounded-lg bg-amber-100 px-4 py-3 text-sm text-amber-800 dark:bg-amber-950 dark:text-amber-300";

/** The refusal face, the same one every other screen announces a problem in. */
const ALERT_CLASSES =
  "rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200";

const PATH_CLASSES = `font-mono underline ${FOCUS_RING}`;

export function DraftConflictsNotice({
  domain,
}: {
  domain: string;
}): ReactElement | null {
  const [open, setOpen] = useState<DraftConflict | null>(null);
  // No retry: every refusal here is immediate and final.
  const drafts = useQuery({
    queryKey: draftConflictsKey(domain),
    queryFn: () => fetchDraftConflicts(domain),
    retry: false,
  });

  const problem =
    drafts.error === null || nothingForYou(drafts.error)
      ? null
      : problemDetail(drafts.error);
  const conflicts = drafts.data?.conflicts ?? [];
  const merged = drafts.data?.merged ?? [];
  const settleNote = drafts.data?.settleNote ?? null;
  // The dialog stays mounted while it is open, even once the list it was
  // opened from has emptied: a receipt it is still showing is the server's
  // only word on where a kept copy went.
  if (
    problem === null &&
    conflicts.length === 0 &&
    merged.length === 0 &&
    open === null
  ) {
    return null;
  }

  function pathButton(conflict: DraftConflict): ReactElement {
    return (
      <button
        key={conflict.id}
        type="button"
        onClick={() => {
          setOpen(conflict);
        }}
        className={PATH_CLASSES}
      >
        {conflict.path}
      </button>
    );
  }

  const [only] = conflicts;
  return (
    <section aria-label="Your drafts" className={NOTICE_CLASSES}>
      {problem !== null && (
        <p role="alert" className={ALERT_CLASSES}>
          {problem}
        </p>
      )}
      {conflicts.length === 1 && only !== undefined && (
        <p role="status">
          Your draft of {pathButton(only)} and the team&apos;s text do not fit
          together. Settle it before you share.
        </p>
      )}
      {conflicts.length > 1 && (
        <p className="flex flex-wrap items-center gap-x-2">
          {/* Only the sentence is the live region: a change to the list
              announces it once, not every path button with it. */}
          <span role="status">
            {`${String(conflicts.length)} of your drafts do not fit with the team's text. Settle them before you share.`}
          </span>
          {conflicts.slice(0, SHOWN).map(pathButton)}
          {/* Plain text: there is nothing behind it to press, and the rest
              come up as the ones above are settled. */}
          {conflicts.length > SHOWN && (
            <span>{`and ${String(conflicts.length - SHOWN)} more`}</span>
          )}
        </p>
      )}
      {/* What she cannot do with them here, in the server's words: it
          alone knows a read-only server, a missing write right, a locked
          domain or an archived repository apart. */}
      {settleNote !== null && conflicts.length > 0 && <p>{settleNote}</p>}
      {merged.map((path) => (
        <p
          key={path}
        >{`Your draft of ${path} now also has the team's change.`}</p>
      ))}
      {open !== null && (
        <ConflictDialog
          domain={domain}
          conflictId={open.id}
          draft={{
            kind: open.kind,
            movedFrom: open.movedFrom,
            other: open.other,
            file: open.file,
          }}
          onClose={() => {
            setOpen(null);
          }}
        />
      )}
    </section>
  );
}

/**
 * Whether a refusal only says "nothing for you here": signed out (401), no
 * right on this domain (403), or no such domain (404).
 */
function nothingForYou(error: Error): boolean {
  return (
    error instanceof ApiProblem &&
    (error.status === 401 || error.status === 403 || error.status === 404)
  );
}
