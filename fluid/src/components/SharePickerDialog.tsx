/**
 * The frame's seam for choosing which domain to share from.
 *
 * The list itself lives behind a lazy import in `SharePickerDialogBody.tsx`,
 * the way every other dialog in this app does: this is the door the top bar's
 * share action opens when the reader is not standing in a team domain, and a
 * session that never presses it should not pay for Radix's dialog code. `open`
 * is always true while this is mounted; the frame mounts it on the press and
 * unmounts it again through `onPick` or `onClose`.
 */

import type { ReactElement } from "react";
import { Suspense, lazy } from "react";

const SharePickerDialogBody = lazy(() => import("./SharePickerDialogBody"));

export interface SharePickerDialogProps {
  /** The domain that was chosen; the frame opens the share dialog on it. */
  onPick: (domain: string) => void;
  /** Leave without choosing: cancelled or dismissed. */
  onClose: () => void;
}

export function SharePickerDialog(props: SharePickerDialogProps): ReactElement {
  return (
    <Suspense
      fallback={
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40">
          <p className="rounded border border-slate-200 bg-white px-4 py-2 text-sm text-slate-600 shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
            Opening the share dialog
          </p>
        </div>
      }
    >
      <SharePickerDialogBody {...props} />
    </Suspense>
  );
}
