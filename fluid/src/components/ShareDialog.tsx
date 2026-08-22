/**
 * The proposals card's seam for sharing what this copy knows with the team.
 *
 * The dialog itself lives behind a lazy import in `ShareDialogBody.tsx`, the
 * way every other dialog in this app does: sharing is a deliberate act at the
 * end of a session's work, and a session that only reads the card should not
 * pay for Radix's dialog code plus two more admin verbs to see a row. `open` is
 * always true while this is mounted; the card mounts it once "Share changes"
 * has been pressed and unmounts it again through `onClose`.
 */

import type { ReactElement } from "react";
import { Suspense, lazy } from "react";

const ShareDialogBody = lazy(() => import("./ShareDialogBody"));

export interface ShareDialogProps {
  /** The team domain whose local changes would be shared. */
  domain: string;
  /** Leave the dialog: cancelled, dismissed, or a share that landed. */
  onClose: () => void;
}

export function ShareDialog(props: ShareDialogProps): ReactElement {
  return (
    <Suspense
      fallback={
        // Plain markup rather than another Radix dialog: reaching for the
        // primitive here would defeat the point of keeping it out of this
        // chunk.
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40">
          <p className="rounded border border-slate-200 bg-white px-4 py-2 text-sm text-slate-600 shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
            Opening the share dialog
          </p>
        </div>
      }
    >
      <ShareDialogBody {...props} />
    </Suspense>
  );
}
