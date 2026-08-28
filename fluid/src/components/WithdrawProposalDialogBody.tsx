/**
 * Taking a proposal back: a second press, and a checkbox that decides whether
 * the shared files come back with it.
 *
 * The checkbox is off by default on purpose. Withdrawing is about the proposal
 * on the forge; restoring the working tree from the origin is a second thing
 * somebody asks for rather than a consequence they discover afterwards. Files a
 * reviewer amended on the proposal branch are left alone either way - the
 * engine reports them as skipped rather than overwriting somebody's edit.
 *
 * A revert re-indexes the domain, so what it invalidates is not only the sync
 * status: restored and deleted files move the tree, the listings and the engram
 * count every sidebar and card draws. A withdraw that only closed a pull
 * request leaves all of that alone, which is why the receipt is read rather
 * than the request's own flag trusted.
 *
 * A layer with open layers on top of it is the one case where withdrawing does
 * something to work other than its own: the chain is rebuilt around the hole,
 * which re-bases every layer above. That is said before the press rather than
 * discovered from the review threads afterwards.
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useState } from "react";

import { syncStatusKey, withdrawProposal } from "../api/admin";
import { problemDetail } from "../api/client";
import { domainTreeKey } from "../api/domain";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { domainEngramsRoot } from "../api/engrams";
import { plural } from "../format";
import { BUTTON } from "./primitives";
import type { WithdrawProposalDialogProps } from "./WithdrawProposalDialog";

export default function WithdrawProposalDialogBody({
  domain,
  proposal,
  layersAbove,
  onClose,
  onNotice,
  onProblem,
}: WithdrawProposalDialogProps): ReactElement {
  const queryClient = useQueryClient();
  const [revert, setRevert] = useState(false);

  const withdraw = useMutation({
    mutationFn: () => withdrawProposal(domain, proposal.number, revert),
    onSuccess: (receipt) => {
      onClose();
      // The one thing a withdraw can fail at while succeeding: a file whose
      // pre-share content is nowhere to be had cannot be put back, and
      // somebody has to be told which ones rather than finding a gap later.
      if (receipt.skippedReverts.length > 0) {
        onNotice(`Could not restore: ${receipt.skippedReverts.join(", ")}`);
      }
      // Always: this proposal is not open any more, and the card that lists it
      // and the card that counts it both read this one status.
      void queryClient.invalidateQueries({ queryKey: syncStatusKey(domain) });
      if (receipt.restored.length === 0 && receipt.deleted.length === 0) {
        return;
      }
      // A revert that actually moved files re-indexed the domain, so
      // everything drawn from what is in it is read again: the folders the
      // navigation walks, every listing either view of the domain screen has
      // paged, and the listing every sidebar, card and switcher counts
      // engrams from.
      void queryClient.invalidateQueries({ queryKey: domainTreeKey(domain) });
      void queryClient.invalidateQueries({
        queryKey: domainEngramsRoot(domain),
      });
      void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
    },
    onError: (error: Error) => {
      onProblem(problemDetail(error));
    },
  });

  return (
    <Dialog.Root
      open
      onOpenChange={(next) => {
        // Escape and the overlay mean what Cancel means: nothing was
        // withdrawn, and the row is still the row.
        if (!next) {
          onClose();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-slate-900/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 w-[min(26rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-lg font-semibold">
            Withdraw proposal #{String(proposal.number)}
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            Closes the proposal on the origin and records it as withdrawn. The
            review stays readable there.
          </Dialog.Description>
          {layersAbove > 0 && (
            <p className="mt-2 rounded bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:bg-amber-950 dark:text-amber-200">
              Closes #{String(proposal.number)} and re-bases{" "}
              {plural(layersAbove, "layer", "layers")} above it.
            </p>
          )}
          <label className="mt-3 flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={revert}
              onChange={(event) => {
                setRevert(event.target.checked);
              }}
            />
            Restore shared files
          </label>
          <div className="mt-3 flex justify-end gap-2">
            <button
              type="button"
              onClick={onClose}
              className={BUTTON.secondary}
            >
              Cancel
            </button>
            {/* Destructive: this closes a thread other people are reading,
                and on the revert path it rewrites files on disk. */}
            <button
              type="button"
              disabled={withdraw.isPending}
              onClick={() => {
                withdraw.mutate();
              }}
              className={BUTTON.destructive}
            >
              Withdraw proposal
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
