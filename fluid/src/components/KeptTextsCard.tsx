/**
 * The domain's kept texts, on its settings: what co-editing rooms held when
 * they could not save, and comments a refused write kept. Each row says
 * which page, what it is, who it came from when the server knows, and when;
 * Show opens it and Drop deletes it after a confirm.
 *
 * The rows are the ones the server gives this person: a room's text to who
 * may write its source, a draft's text to its owner, a comment to its sender
 * or to the Reflection's editors. The screen asks for the listing and draws
 * this card only when the answer is a list, so a person with no right never
 * meets the section at all.
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useState } from "react";

import { problemDetail } from "../api/client";
import type { KeptTextEntry } from "../api/kept";
import {
  dropKeptText,
  keptFrom,
  keptKey,
  keptKindLabel,
  keptTextKey,
} from "../api/kept";
import { useAuth } from "../auth/AuthContext";
import { formatInstant } from "../format";
import { DestructiveAction, READ_ONLY_REASON } from "./DestructiveAction";
import { KeptTextDialog } from "./KeptTextDialog";
import { BUTTON } from "./primitives";
import { refreshAfterWrite } from "../query/refreshAfterWrite";

export function KeptTextsCard({
  domain,
  entries,
}: {
  domain: string;
  entries: readonly KeptTextEntry[];
}): ReactElement {
  const { capabilities } = useAuth();
  const queryClient = useQueryClient();
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
  const readOnly = capabilities.readOnly ? READ_ONLY_REASON : undefined;
  // Newest first: the one somebody is looking for is usually the last one.
  const rows = [...entries].reverse();

  return (
    <section aria-labelledby="domain-kept" className="flex flex-col gap-4">
      <h2 id="domain-kept" className="text-section">
        Kept texts
      </h2>
      <p className="text-sm text-ink-3">
        Text the server kept when a shared editing room could not save it, and
        comments that could not be saved. Copy what you need back, then drop it.
      </p>
      {problem !== null && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problem}
        </p>
      )}
      {rows.length === 0 ? (
        <p className="text-sm text-ink-3">No kept texts.</p>
      ) : (
        <ul
          aria-label="Kept texts"
          className="flex flex-col divide-y divide-hair"
        >
          {rows.map((entry) => {
            const from = keptFrom(entry);
            return (
              <li
                key={entry.id}
                className="flex flex-col gap-2 py-3 first:pt-0 last:pb-0 sm:flex-row sm:items-center sm:justify-between sm:gap-4"
              >
                <div className="flex min-w-0 flex-col gap-0.5">
                  <span className="font-mono text-sm break-all text-ink">
                    {entry.path}
                  </span>
                  <span className="text-caption text-ink-3">
                    {[
                      keptKindLabel(entry.kind),
                      from,
                      formatInstant(entry.kept_at),
                    ]
                      .filter((part) => part !== null)
                      .join(" · ")}
                  </span>
                </div>
                <div className="flex shrink-0 flex-wrap items-center gap-2">
                  <button
                    type="button"
                    aria-label={`Show the kept ${entry.kind === "comment" ? "comment" : "text"} for ${entry.path}`}
                    onClick={() => {
                      setShowing(entry);
                    }}
                    className={BUTTON.secondary}
                  >
                    Show
                  </button>
                  <DestructiveAction
                    label="Drop"
                    ariaLabel={`Drop the kept ${entry.kind === "comment" ? "comment" : "text"} for ${entry.path}`}
                    confirmLabel="Drop it"
                    pending={drop.isPending && drop.variables === entry.id}
                    disabledReason={readOnly}
                    onConfirm={() => {
                      setProblem(null);
                      drop.mutate(entry.id);
                    }}
                  />
                </div>
              </li>
            );
          })}
        </ul>
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
    </section>
  );
}
