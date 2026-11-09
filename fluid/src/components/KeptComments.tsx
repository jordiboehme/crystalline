/**
 * The reader's own comments on this page that the server kept: a comment a
 * Reflection write could not land (its instance lost the domain's host lock
 * mid-write) is kept in the server's state folder for its sender.
 *
 * Shown in the threads column, where the comment was written, because a
 * person who only reads the Reflection has no editor and no domain settings
 * to find it in. The server decides who gets which entry; this only picks
 * the comments on this page whose sender is the reader, and offers the text
 * to copy back into a new comment, and a drop once that is done.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useState } from "react";

import { problemDetail } from "../api/client";
import type { KeptTextEntry } from "../api/kept";
import { dropKeptText, keptKey, keptQuery, keptTextKey } from "../api/kept";
import { useAuth } from "../auth/AuthContext";
import { formatInstant } from "../format";
import { DestructiveAction } from "./DestructiveAction";
import { KeptTextDialog } from "./KeptTextDialog";
import { FOCUS_RING } from "./primitives";
import { refreshAfterWrite } from "../query/refreshAfterWrite";

const ACTION = `rounded text-link underline decoration-(--link-line) underline-offset-[3px] hover:no-underline ${FOCUS_RING}`;

export function KeptComments({
  domain,
  path,
}: {
  domain: string;
  /** The page's path inside `reflections/`. */
  path: string;
}): ReactElement | null {
  const { user, capabilities } = useAuth();
  const queryClient = useQueryClient();
  const listing = useQuery(keptQuery(domain, user !== null));
  const [showing, setShowing] = useState<KeptTextEntry | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const drop = useMutation({
    mutationFn: (id: string) => dropKeptText(domain, id),
    onSuccess: (_, id) => {
      setProblem(null);
      queryClient.removeQueries({ queryKey: keptTextKey(domain, id) });
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
    onSettled: () => {
      void refreshAfterWrite(queryClient, { queryKey: keptKey(domain) });
    },
  });
  if (user === null) {
    return null;
  }
  const mine = (listing.data ?? []).filter(
    (entry) =>
      entry.kind === "comment" &&
      entry.path === path &&
      // The account the server matched the rights on, never the display
      // name: two people may share one. An entry with no recorded account
      // (an older kept file) is nobody's here; the settings still list it.
      entry.sender_account != null &&
      entry.sender_account === user.name,
  );
  if (mine.length === 0) {
    return null;
  }
  return (
    <aside
      aria-label="Your kept comments"
      className="flex flex-col gap-2 rounded-[10px] border border-hair bg-sunken px-3 py-2.5 text-[13px] text-ink-2"
    >
      <p>
        {mine.length === 1
          ? "A comment of yours was kept."
          : `${String(mine.length)} comments of yours were kept.`}{" "}
        The server could not save it. Copy it into a new comment, then drop it.
      </p>
      <ul className="flex flex-col gap-2">
        {mine.map((entry) => (
          <li key={entry.id} className="flex flex-wrap items-center gap-3">
            <span className="text-caption text-ink-3">
              {formatInstant(entry.kept_at)}
            </span>
            <button
              type="button"
              className={ACTION}
              onClick={() => {
                setShowing(entry);
              }}
            >
              Show
            </button>
            {!capabilities.readOnly && (
              <DestructiveAction
                label="Drop"
                ariaLabel="Drop the kept comment"
                confirmLabel="Drop it"
                pending={drop.isPending && drop.variables === entry.id}
                onConfirm={() => {
                  setProblem(null);
                  drop.mutate(entry.id);
                }}
              />
            )}
          </li>
        ))}
      </ul>
      {problem !== null && (
        <p role="alert" className="text-red-800 dark:text-red-200">
          {problem}
        </p>
      )}
      {showing !== null && (
        <KeptTextDialog
          domain={domain}
          entry={showing}
          onClose={() => {
            setShowing(null);
          }}
        />
      )}
    </aside>
  );
}
