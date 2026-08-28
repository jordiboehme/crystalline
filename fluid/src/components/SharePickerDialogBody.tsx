/**
 * Which domain to share from, when the frame cannot tell on its own.
 *
 * One row per team domain holding work the team has not seen, sized so the
 * choice between them is a choice rather than a guess. Nothing else: a domain
 * with nothing waiting is not an option here, and the way to a domain's own
 * proposals is the domain, not this list.
 *
 * It reads the same summary the top bar's action decided from, under the same
 * key and with the same freshness, so opening the picker costs nothing: the
 * answer is already in the cache, and re-reading it would probe every origin
 * again to draw a list that is already in hand.
 *
 * The empty state is unreachable through the button that opens this - a summary
 * with nothing to share is what disables it - and is written all the same: a
 * cache that went stale between the press and the render is not a reason to put
 * an empty panel in front of somebody.
 */

import { useQuery } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";

import type { SyncSummaryEntry } from "../api/admin";
import {
  SYNC_SUMMARY_KEY,
  SYNC_SUMMARY_STALE_MS,
  fetchSyncSummary,
} from "../api/admin";
import { plural } from "../format";
import type { SharePickerDialogProps } from "./SharePickerDialog";
import { BUTTON, Chip, FOCUS_RING } from "./primitives";

/** One row: the name, and how much is waiting in it. */
const ROW_CLASSES = `flex w-full items-baseline justify-between gap-3 rounded border border-slate-300 px-2 py-1.5 text-left text-sm hover:bg-slate-100 dark:border-slate-700 dark:hover:bg-slate-800 ${FOCUS_RING}`;

/** How much is waiting, in the words the count is read in. */
function pendingChanges(count: number): string {
  return plural(count, "pending change", "pending changes");
}

/**
 * What is wrong with this domain's chain of stacked proposals, or null when
 * nothing is.
 *
 * One badge rather than three, and wedged wins: a wedged chain cannot grow
 * until a declined layer is withdrawn or the chain is repaired, so it is the
 * one fact that changes what happens when this row is picked. The two debts
 * behind it are settled by the very share this picker leads to, so they are a
 * note rather than a warning - and the domain is still offered either way,
 * because the picker's job is to say which domain, not to refuse for a route
 * that can refuse for itself.
 */
function chainBadge(entry: SyncSummaryEntry): string | null {
  if (entry.stackWedged.length > 0) {
    return "stack wedged";
  }
  if (entry.repairPending) {
    return "repair pending";
  }
  return entry.stackLinkPending ? "stack link pending" : null;
}

/** The whole row said as one line, which is what a row is called. */
function rowLabel(domain: string, count: number, badge: string | null): string {
  const chain = badge === null ? "" : `, ${badge}`;
  return `${domain} - ${pendingChanges(count)}${chain}`;
}

export default function SharePickerDialogBody({
  onPick,
  onClose,
}: SharePickerDialogProps): ReactElement {
  const summary = useQuery({
    queryKey: SYNC_SUMMARY_KEY,
    queryFn: fetchSyncSummary,
    staleTime: SYNC_SUMMARY_STALE_MS,
    // Off the app's refetch-on-focus default for the reason the frame's own
    // observer is: reading this probes every origin at once, and coming back
    // to the tab is not a reason to do that to draw a list already on screen.
    refetchOnWindowFocus: false,
  });
  const waiting = (summary.data?.domains ?? []).filter(
    (entry) => entry.localChanges > 0,
  );

  return (
    <Dialog.Root
      open
      onOpenChange={(next) => {
        if (!next) {
          onClose();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-slate-900/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 max-h-[calc(100vh-4rem)] w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-lg font-semibold">
            Share from a domain
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            The team domains holding work nobody else has seen yet.
          </Dialog.Description>
          {waiting.length === 0 ? (
            <p className="mt-3 text-sm text-slate-500 dark:text-slate-400">
              Nothing to share.
            </p>
          ) : (
            <ul className="mt-3 flex flex-col gap-1">
              {waiting.map((entry) => {
                const badge = chainBadge(entry);
                return (
                  <li key={entry.domain}>
                    {/*
                      Named as the whole row rather than left to be assembled
                      out of the two ends of it. The name and the count sit at
                      opposite edges the way the sidebar's own domain rows do,
                      and a name computed from that is the two halves run
                      together; spelling it out is what makes the row read as
                      "eng - 2 pending changes" to anything listening - and it
                      is what carries the chain badge to a reader who hears
                      the row rather than sees it.
                    */}
                    <button
                      type="button"
                      aria-label={rowLabel(
                        entry.domain,
                        entry.localChanges,
                        badge,
                      )}
                      onClick={() => {
                        onPick(entry.domain);
                      }}
                      className={ROW_CLASSES}
                    >
                      <span className="flex min-w-0 items-baseline gap-2">
                        <span className="truncate font-medium">
                          {entry.domain}
                        </span>
                        {badge !== null && (
                          <Chip variant="caution">{badge}</Chip>
                        )}
                      </span>
                      <span className="text-caption shrink-0 text-slate-500 dark:text-slate-400">
                        {pendingChanges(entry.localChanges)}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
          <div className="mt-3 flex justify-end">
            <button
              type="button"
              onClick={onClose}
              className={BUTTON.secondary}
            >
              Cancel
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
