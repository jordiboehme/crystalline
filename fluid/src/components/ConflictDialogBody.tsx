/**
 * Settling one conflict from the browser: both sides read side by side, and
 * three ways out of them.
 *
 * The two sides are shown as they are stored rather than as a merged file with
 * markers in it. A conflict here is one whole engram against another whole
 * engram - the engine never writes a half-merged file into the domain - so the
 * decision is "which of these two", and the hand merge is the escape hatch for
 * the times the answer is neither.
 *
 * A side that arrives empty is said in words rather than drawn as an empty
 * pane, and the two reasons it can be empty are two different decisions: a file
 * the other side deleted, which taking that side would delete here too, and
 * bytes this browser cannot show. The report's `note` is what tells them apart -
 * it is set exactly when a side could not be decoded - and it is quoted whole,
 * because it is also the only sentence that says WHICH side that was.
 *
 * What a resolve invalidates is not only the sync status, and the reason is the
 * engine rather than the file: `origin_resolve` re-syncs the domain after every
 * resolve, including the one that writes nothing at all - keeping this copy's
 * own side is an empty arm on the engine's side. So the tree, the listings and
 * the engram count every sidebar and card draws are all about to be stale
 * whichever way the conflict was settled, which is why this invalidates
 * unconditionally where the withdraw dialog's revert branch asks the receipt
 * first.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useId, useState } from "react";

import { fetchConflict, resolveConflict, syncStatusKey } from "../api/admin";
import { problemDetail } from "../api/client";
import { domainTreeKey } from "../api/domain";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { domainEngramsRoot } from "../api/engrams";
import type { ConflictDialogProps } from "./ConflictDialog";
import { BUTTON } from "./primitives";

/** The refusal face, the same one every other screen announces a problem in. */
const ALERT_CLASSES =
  "rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200";

/** One side's pane: a scrollable box that never grows past half the dialog. */
const PANE_CLASSES =
  "max-h-56 overflow-auto rounded border border-slate-200 p-2 font-mono text-xs whitespace-pre-wrap dark:border-slate-800";

export default function ConflictDialogBody({
  domain,
  conflictId,
  onClose,
}: ConflictDialogProps): ReactElement {
  const queryClient = useQueryClient();
  const mergedField = useId();
  // Whether the merge editor is showing, kept APART from the text in it. One
  // piece of state would mean closing the editor threw the text away, and the
  // toggle sits one button away from "Save merged": a misclick would delete a
  // hand-written merge with no undo and no warning. So closing hides the
  // editor and nothing else, and reopening shows what was written rather than
  // the prefill again.
  const [mergeOpen, setMergeOpen] = useState(false);
  // `null` is "nobody has opened the editor yet", which is what makes the
  // prefill happen once; a string is the text, empty string included - a merge
  // that deletes everything is a decision, not an unseeded editor.
  const [merged, setMerged] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  // Filed under this domain's own prefix, which is safe here for a reason
  // worth naming: reading a conflict is a plain GET with no side effect, so a
  // bulk `["domains"]` invalidation re-reading it costs a round trip and
  // nothing else. The share plan next door is filed outside that family
  // precisely because reading IT pulls the origin.
  //
  // No retry: the refusals this call can carry - read-only, GitHub off, a
  // conflict somebody else already settled - are immediate and final, and a
  // backoff would only hold the dialog on a spinner.
  const detail = useQuery({
    queryKey: ["domains", domain, "conflict", conflictId],
    queryFn: () => fetchConflict(domain, conflictId),
    retry: false,
  });

  const resolve = useMutation({
    mutationFn: (choice: { resolution: string; content?: string }) =>
      resolveConflict(domain, conflictId, choice.resolution, choice.content),
    onSuccess: () => {
      onClose();
      // The status the card that opened this is drawn from, and then
      // everything drawn from what is in the domain: the engine re-syncs after
      // every resolve, so what is in it has moved even when this side of the
      // choice wrote nothing.
      void queryClient.invalidateQueries({ queryKey: syncStatusKey(domain) });
      void queryClient.invalidateQueries({ queryKey: domainTreeKey(domain) });
      void queryClient.invalidateQueries({
        queryKey: domainEngramsRoot(domain),
      });
      void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  const conflict = detail.data ?? null;
  const readProblem =
    detail.error === null ? null : problemDetail(detail.error);

  function settle(resolution: string, content?: string): void {
    if (resolve.isPending) {
      return;
    }
    setProblem(null);
    resolve.mutate(
      content === undefined ? { resolution } : { resolution, content },
    );
  }

  return (
    <Dialog.Root
      open
      onOpenChange={(next) => {
        // Escape and the overlay mean what Cancel means: nothing was settled,
        // and the conflict is still on the card.
        if (!next) {
          onClose();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-slate-900/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 w-[min(46rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-lg font-semibold">
            Settle conflict
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            {conflict === null
              ? "Reading both sides of this conflict."
              : `${conflict.path} - this copy and the team's changed the same engram.`}
          </Dialog.Description>

          {(problem ?? readProblem) !== null && (
            <p role="alert" className={`mt-3 ${ALERT_CLASSES}`}>
              {problem ?? readProblem}
            </p>
          )}

          {conflict !== null && (
            <>
              {/* Quoted whole, and above the panes rather than inside one:
                  the sentence names the side it is about, and this side has
                  no way to work out which that was. */}
              {conflict.note !== null && (
                <p className="mt-3 text-sm text-slate-500 dark:text-slate-400">
                  {conflict.note}
                </p>
              )}
              <div className="mt-3 grid gap-3 sm:grid-cols-2">
                <Side
                  label="Mine (local)"
                  text={conflict.local}
                  unreadable={conflict.note !== null}
                />
                <Side
                  label="Theirs (upstream)"
                  text={conflict.upstream}
                  unreadable={conflict.note !== null}
                />
              </div>

              {mergeOpen && (
                <div className="mt-3 flex flex-col gap-1 text-sm">
                  <label htmlFor={mergedField}>Merged content</label>
                  <textarea
                    id={mergedField}
                    rows={8}
                    value={merged ?? ""}
                    onChange={(event) => {
                      setMerged(event.target.value);
                    }}
                    className="w-full rounded border border-slate-300 bg-transparent px-2 py-1 font-mono text-xs focus-visible:ring-2 focus-visible:ring-accent-600 focus-visible:outline-none dark:border-slate-700 dark:focus-visible:ring-accent-400"
                  />
                </div>
              )}

              <div className="mt-3 flex flex-wrap justify-end gap-2">
                <button
                  type="button"
                  onClick={onClose}
                  className={BUTTON.secondary}
                >
                  Cancel
                </button>
                {/* The escape hatch, and a toggle rather than a mode: opening
                    it does not take the two straight answers away, because
                    reading the merge is often what decides one of them. */}
                <button
                  type="button"
                  aria-expanded={mergeOpen}
                  onClick={() => {
                    // Seeded once, on the first opening: after that the text
                    // is whatever it was left as, so reopening never
                    // overwrites a merge with the prefill it started from.
                    setMerged((text) => text ?? conflict.local ?? "");
                    setMergeOpen((open) => !open);
                  }}
                  className={BUTTON.secondary}
                >
                  Edit merged
                </button>
                <button
                  type="button"
                  disabled={resolve.isPending}
                  onClick={() => {
                    settle("mine");
                  }}
                  className={BUTTON.secondary}
                >
                  Keep mine
                </button>
                <button
                  type="button"
                  disabled={resolve.isPending}
                  onClick={() => {
                    settle("theirs");
                  }}
                  className={BUTTON.secondary}
                >
                  Take theirs
                </button>
                {/* The primary tier only on the merge, because it is the only
                    one of the three that commits something somebody wrote
                    rather than picking a side that already exists. */}
                {mergeOpen && (
                  <button
                    type="button"
                    disabled={resolve.isPending}
                    onClick={() => {
                      settle("merged", merged ?? "");
                    }}
                    className={BUTTON.primary}
                  >
                    Save merged
                  </button>
                )}
              </div>
            </>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * One side of the conflict, or the reason there is nothing to show.
 *
 * The empty case is deliberately two sentences rather than one. "(file
 * deleted)" is a fact - the other side removed this engram, and taking that
 * side removes it here - while a side that could not be decoded is still there
 * and still theirs, and calling that a deletion would talk somebody into
 * throwing work away. Which one applies is decided by the report's `note`,
 * which is set exactly when some side would not decode; a report carrying a
 * note cannot say which side it meant, so an empty side under one is reported
 * as unreadable rather than guessed at.
 */
function Side({
  label,
  text,
  unreadable,
}: {
  label: string;
  text: string | null;
  unreadable: boolean;
}): ReactElement {
  return (
    <div className="flex flex-col gap-1">
      <h3 className="text-caption text-slate-500 dark:text-slate-400">
        {label}
      </h3>
      {text === null ? (
        <p className="text-sm text-slate-500 italic dark:text-slate-400">
          {unreadable ? "(no readable content)" : "(file deleted)"}
        </p>
      ) : (
        <pre className={PANE_CLASSES}>{text}</pre>
      )}
    </div>
  );
}
