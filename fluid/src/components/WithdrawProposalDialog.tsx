/**
 * The proposals card's seam for taking a proposal back.
 *
 * The dialog itself lives behind a lazy import in
 * `WithdrawProposalDialogBody.tsx`, the way every other dialog in this app
 * does: withdrawing is the rarest thing anybody does to a proposal, and a
 * session that only looks at the card should not pay for Radix's dialog code to
 * see a row. `open` is always true while this is mounted; the row mounts it
 * once "Withdraw" has been pressed and unmounts it again through `onClose`.
 */

import type { ReactElement } from "react";
import { Suspense, lazy } from "react";

import type { SyncProposal } from "../api/admin";

const WithdrawProposalDialogBody = lazy(
  () => import("./WithdrawProposalDialogBody"),
);

export interface WithdrawProposalDialogProps {
  /** The team domain the proposal belongs to. */
  domain: string;
  /** The proposal being taken back, for its number and its title. */
  proposal: SyncProposal;
  /** Leave the dialog: cancelled, dismissed, or a withdraw that landed. */
  onClose: () => void;
  /**
   * A refusal in the server's own words.
   *
   * Handed back rather than drawn here, because an open dialog `aria-hidden`s
   * the page behind it: a refusal announced under one is unreachable to
   * anything reading the screen. The row closes the dialog and says it.
   */
  onProblem: (detail: string) => void;
}

export function WithdrawProposalDialog(
  props: WithdrawProposalDialogProps,
): ReactElement {
  return (
    <Suspense
      fallback={
        // Plain markup rather than another Radix dialog: reaching for the
        // primitive here would defeat the point of keeping it out of this
        // chunk.
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40">
          <p className="rounded border border-slate-200 bg-white px-4 py-2 text-sm text-slate-600 shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
            Opening the withdraw dialog
          </p>
        </div>
      }
    >
      <WithdrawProposalDialogBody {...props} />
    </Suspense>
  );
}
