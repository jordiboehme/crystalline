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
 *
 * With `draft` set the conflict is one of this session's own drafts in a
 * domain that reviews changes, and three things change. It is read and settled
 * through the draft routes, which any writer may reach. Its words are the
 * plan's table for a draft: a sentence per kind, "Your draft" and "The team's
 * text", and a hint under each choice saying what it does, hung on the button
 * as its description so the button's own name stays the same. And two choices
 * can be off: Keep mine for an address another team page holds (sharing it
 * would land a second page at that address), and Edit merged for a file, which
 * has no text to merge. A receipt that carries a sentence of the server's (a
 * kept copy, a draft that landed but not on disk) is shown before the dialog
 * goes, since it is the only place that says it.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useId, useState } from "react";

import type {
  ConflictDetail,
  DraftConflictDetail,
  DraftResolveReceipt,
} from "../api/admin";
import {
  draftConflictKey,
  draftConflictsKey,
  fetchConflict,
  fetchDraftConflict,
  resolveConflict,
  resolveDraftConflict,
  sharePlanKey,
  syncStatusKey,
} from "../api/admin";
import { ApiProblem, problemDetail } from "../api/client";
import { domainTreeKey } from "../api/domain";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { domainEngramsRoot } from "../api/engrams";
import { keptKey } from "../api/kept";
import type { ConflictDialogProps } from "./ConflictDialog";
import { BUTTON } from "./primitives";
import { refreshAfterWrite } from "../query/refreshAfterWrite";

/** The refusal face, the same one every other screen announces a problem in. */
const ALERT_CLASSES =
  "rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200";

/** One side's pane: a scrollable box that never grows past half the dialog. */
const PANE_CLASSES =
  "max-h-56 overflow-auto rounded border border-slate-200 p-2 font-mono text-xs whitespace-pre-wrap dark:border-slate-800";

/** The quiet face a hint under a choice wears. */
const HINT_CLASSES = "text-caption text-slate-500 dark:text-slate-400";

/**
 * What a draft conflict is, in one sentence: the plan's table, word for word.
 * A kind this side has not been taught says only which path it is about.
 */
function draftKindSentence(
  kind: string,
  path: string,
  movedFrom: string | null,
  other: string | null,
): string {
  switch (kind) {
    case "EditEdit":
      return movedFrom === null
        ? `You and the team changed the same lines of ${path}.`
        : `You moved ${movedFrom} to ${path}, and the team changed it. The changes do not fit together.`;
    case "EditDelete":
      return `You changed ${path}. The team deleted it.`;
    case "DeleteEdit":
      return `You deleted ${path}. The team changed it.`;
    case "AddAdd":
      return `You and the team both have a file at ${path}, with different text.`;
    case "UnknownStart":
      return "This draft is from an older version, so we cannot tell what the team changed since. Compare the two texts.";
    case "AddressTaken":
      return `The team's file ${other ?? "another file"} now uses the same address as your draft of ${path}.`;
    default:
      return `Your draft of ${path} and the team's text do not fit together.`;
  }
}

/** The hint under Take theirs: an old draft's text is kept, any other is not. */
function theirsHint(kind: string, local: string | null, file: boolean): string {
  // An old draft is kept first because it exists nowhere else; a deletion
  // has no text to keep.
  return kind === "UnknownStart" && (file || local !== null)
    ? "Your draft ends. A copy stays in your kept texts."
    : "Your draft ends and your text is gone.";
}

export default function ConflictDialogBody({
  domain,
  conflictId,
  onClose,
  draft,
  onSettled,
}: ConflictDialogProps): ReactElement {
  const queryClient = useQueryClient();
  const mergedField = useId();
  const mineHint = useId();
  const theirsHintId = useId();
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
  // A draft's receipt that carried a sentence of the server's, held so the
  // dialog can say it before it goes. Null while nothing is settled.
  const [receipt, setReceipt] = useState<DraftResolveReceipt | null>(null);

  // Filed under this domain's own prefix, which is safe here for a reason
  // worth naming: reading a conflict is a plain GET with no side effect, so a
  // bulk `["domains"]` invalidation re-reading it costs a round trip and
  // nothing else. The share plan next door is filed outside that family
  // precisely because reading IT pulls the origin.
  //
  // No retry: the refusals this call can carry - read-only, GitHub off, a
  // conflict somebody else already settled - are immediate and final, and a
  // backoff would only hold the dialog on a spinner.
  //
  // A draft's sides are read off the draft route, and the read stops once it
  // is settled: the invalidations below reach this key too, and a read of a
  // conflict that is no longer one would put a 404 over the receipt.
  //
  // And a draft's sides hold still while the merge editor is open (C25): the
  // merge is written against the team text on the screen, so neither a
  // window coming back nor a bulk invalidation may swap that text under it.
  // The resolve names the team side that was shown; when the server says it
  // moved (409), the sides are read again on purpose, below.
  const detail = useQuery<ConflictDetail | DraftConflictDetail>({
    queryKey:
      draft === undefined
        ? ["domains", domain, "conflict", conflictId]
        : draftConflictKey(domain, conflictId),
    queryFn: () =>
      draft === undefined
        ? fetchConflict(domain, conflictId)
        : fetchDraftConflict(domain, conflictId),
    retry: false,
    refetchOnWindowFocus: draft === undefined,
    enabled: receipt === null && !(draft !== undefined && mergeOpen),
  });

  const resolve = useMutation({
    mutationFn: async (choice: {
      resolution: string;
      content?: string;
      expectedUpstream?: string | null;
    }) =>
      draft === undefined
        ? (await resolveConflict(
            domain,
            conflictId,
            choice.resolution,
            choice.content,
          ),
          null)
        : resolveDraftConflict(
            domain,
            conflictId,
            choice.resolution,
            choice.content,
            choice.expectedUpstream,
          ),
    onSuccess: (settled) => {
      onSettled?.();
      // A receipt with a sentence of its own stays up until it is read; any
      // other settle leaves at once, as the folder's always did.
      if (
        settled !== null &&
        (settled.keptNote !== null || settled.draftWarning !== null)
      ) {
        setReceipt(settled);
      } else {
        onClose();
      }
      // The status the card that opened this is drawn from, and then
      // everything drawn from what is in the domain: the engine re-syncs after
      // every resolve, so what is in it has moved even when this side of the
      // choice wrote nothing.
      void refreshAfterWrite(queryClient, { queryKey: syncStatusKey(domain) });
      void refreshAfterWrite(queryClient, { queryKey: domainTreeKey(domain) });
      void refreshAfterWrite(queryClient, {
        queryKey: domainEngramsRoot(domain),
      });
      void refreshAfterWrite(queryClient, { queryKey: DOMAINS_QUERY_KEY });
      if (draft !== undefined) {
        // A draft's own list, the share plan that took the file's box away,
        // and the kept texts when a copy was kept there.
        void refreshAfterWrite(queryClient, {
          queryKey: draftConflictsKey(domain),
        });
        void refreshAfterWrite(queryClient, { queryKey: sharePlanKey(domain) });
        if (settled?.keptCopy === true) {
          void refreshAfterWrite(queryClient, { queryKey: keptKey(domain) });
        }
      }
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
      // The team's side moved since it was shown: read it again, so "look
      // at it again" shows what is there now. A merge being written stays.
      if (
        draft !== undefined &&
        error instanceof ApiProblem &&
        error.status === 409
      ) {
        void detail.refetch();
      }
    },
  });

  const conflict = detail.data ?? null;
  // What this conflict is, from the sides just read once they are here, and
  // from the list that opened the dialog until then: the kind can have moved
  // in between (a team rename makes an address taken).
  // With `draft` the query reads the draft route, so its answer is a draft's.
  const sides: DraftConflictDetail | null =
    draft !== undefined && conflict !== null
      ? (conflict as DraftConflictDetail)
      : null;
  const shown =
    draft === undefined
      ? undefined
      : sides !== null && sides.kind !== ""
        ? {
            kind: sides.kind,
            movedFrom: sides.movedFrom,
            other: sides.other,
            file: sides.file,
          }
        : draft;
  const file = shown?.file === true;
  const addressTaken = shown?.kind === "AddressTaken";
  const readProblem =
    detail.error === null ? null : problemDetail(detail.error);

  function settle(resolution: string, content?: string): void {
    if (resolve.isPending) {
      return;
    }
    setProblem(null);
    resolve.mutate({
      resolution,
      ...(content === undefined ? {} : { content }),
      // The team side on the screen right now, which is what she decided on.
      expectedUpstream: sides?.upstreamSha256 ?? null,
    });
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
              : shown === undefined
                ? `${conflict.path} - this copy and the team's changed the same engram.`
                : draftKindSentence(
                    shown.kind,
                    conflict.path,
                    shown.movedFrom,
                    shown.other,
                  )}
          </Dialog.Description>

          {(problem ?? readProblem) !== null && (
            <p role="alert" className={`mt-3 ${ALERT_CLASSES}`}>
              {problem ?? readProblem}
            </p>
          )}

          {receipt !== null && (
            <div className="mt-3 flex flex-col gap-3">
              {/* The server's own sentences, as it sent them: the only place
                  that says where a kept copy went, or that the draft landed
                  but not on this server's disk. */}
              {[receipt.keptNote, receipt.draftWarning]
                .filter((line): line is string => line !== null)
                .map((line) => (
                  <p key={line} role="status" className="text-sm">
                    {line}
                  </p>
                ))}
              <div className="flex justify-end">
                <button
                  type="button"
                  autoFocus
                  onClick={onClose}
                  className={BUTTON.primary}
                >
                  Close
                </button>
              </div>
            </div>
          )}

          {receipt === null && conflict !== null && shown !== undefined && (
            <>
              {conflict.note !== null && (
                <p className="mt-3 text-sm text-slate-500 dark:text-slate-400">
                  {conflict.note}
                </p>
              )}
              {/* A file has no text to put side by side; its note says so. */}
              {!file && (
                <div className="mt-3 grid gap-3 sm:grid-cols-2">
                  <Side
                    label="Your draft"
                    text={conflict.local}
                    empty={
                      conflict.note !== null
                        ? "(no readable content)"
                        : "You deleted this file."
                    }
                  />
                  <Side
                    label="The team's text"
                    text={conflict.upstream}
                    empty={
                      conflict.note !== null
                        ? "(no readable content)"
                        : "The team deleted this file."
                    }
                  />
                </div>
              )}

              {mergeOpen && !file && (
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

              {/* One choice per row, each with what it does beside it. */}
              <ul className="mt-3 flex flex-col gap-2 text-sm">
                <li className="flex flex-wrap items-center gap-2">
                  {/* Off for a taken address, but still reachable, so a
                      screen reader hears the hint that says why. */}
                  <button
                    type="button"
                    disabled={resolve.isPending}
                    aria-disabled={addressTaken ? true : undefined}
                    aria-describedby={mineHint}
                    onClick={() => {
                      if (!addressTaken) {
                        settle("mine");
                      }
                    }}
                    className={`${BUTTON.secondary} aria-disabled:cursor-default aria-disabled:opacity-50`}
                  >
                    Keep mine
                  </button>
                  <span id={mineHint} className={HINT_CLASSES}>
                    {addressTaken
                      ? "Change the address in Edit merged, or take theirs."
                      : "Your text replaces the team's when you share."}
                  </span>
                </li>
                <li className="flex flex-wrap items-center gap-2">
                  <button
                    type="button"
                    disabled={resolve.isPending}
                    aria-describedby={theirsHintId}
                    onClick={() => {
                      settle("theirs");
                    }}
                    className={BUTTON.secondary}
                  >
                    Take theirs
                  </button>
                  <span id={theirsHintId} className={HINT_CLASSES}>
                    {theirsHint(shown.kind, conflict.local, file)}
                  </span>
                </li>
                <li className="flex flex-wrap items-center gap-2">
                  {file ? (
                    <span className={HINT_CLASSES}>
                      A file cannot be merged here. Keep one of the two.
                    </span>
                  ) : (
                    <>
                      <button
                        type="button"
                        aria-expanded={mergeOpen}
                        onClick={() => {
                          // Her text, or the team's where her side is a
                          // deletion: an empty start would save nothing.
                          setMerged(
                            (text) =>
                              text ?? conflict.local ?? conflict.upstream ?? "",
                          );
                          setMergeOpen((open) => !open);
                        }}
                        className={BUTTON.secondary}
                      >
                        Edit merged
                      </button>
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
                    </>
                  )}
                </li>
              </ul>
              <div className="mt-3 flex justify-end">
                <button
                  type="button"
                  onClick={onClose}
                  className={BUTTON.secondary}
                >
                  Cancel
                </button>
              </div>
            </>
          )}

          {receipt === null && conflict !== null && draft === undefined && (
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
                  empty={
                    conflict.note !== null
                      ? "(no readable content)"
                      : "(file deleted)"
                  }
                />
                <Side
                  label="Theirs (upstream)"
                  text={conflict.upstream}
                  empty={
                    conflict.note !== null
                      ? "(no readable content)"
                      : "(file deleted)"
                  }
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
  empty,
}: {
  label: string;
  text: string | null;
  /** What to say when there is no text: a deletion, or bytes not shown. */
  empty: string;
}): ReactElement {
  return (
    <div className="flex flex-col gap-1">
      <h3 className="text-caption text-slate-500 dark:text-slate-400">
        {label}
      </h3>
      {text === null ? (
        <p className="text-sm text-slate-500 italic dark:text-slate-400">
          {empty}
        </p>
      ) : (
        <pre className={PANE_CLASSES}>{text}</pre>
      )}
    </div>
  );
}
