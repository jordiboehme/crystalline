/**
 * The launcher's own seam for the move flow.
 *
 * The dialog primitive lives behind a lazy import in `MoveDialogBody.tsx`, so
 * an engram page that never opens "Move" never pays for Radix's dialog code.
 * `open` is always true while this is mounted; the launcher mounts it only
 * once "Move" has been clicked and unmounts it again through `onClose`.
 */

import type { ReactElement } from "react";
import { Suspense, lazy } from "react";

import type { EngramDetail } from "../api/engram";

const MoveDialogBody = lazy(() => import("./MoveDialogBody"));

/**
 * What a move hands the page it lands on, in the navigation state: how many
 * references followed the engram there and across how many engrams. Absent
 * when none did, so an ordinary move says nothing. Declared on this seam
 * rather than in the lazy body, so the page that reads it never has to name
 * the chunk it is trying not to load.
 */
export interface MovedState {
  moved: { references: number; engrams: number };
}

export interface MoveDialogProps {
  engram: EngramDetail;
  /** Every registered domain name, for the optional cross-domain target. */
  domains: string[];
  onClose: () => void;
}

export function MoveDialog(props: MoveDialogProps): ReactElement {
  return (
    <Suspense
      fallback={
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40">
          <p className="rounded border border-slate-200 bg-white px-4 py-2 text-sm text-slate-600 shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
            Opening the move dialog
          </p>
        </div>
      }
    >
      <MoveDialogBody {...props} />
    </Suspense>
  );
}
