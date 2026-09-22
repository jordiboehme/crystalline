/**
 * The engram page's seam for looking at what this copy holds that the team's
 * copy does not.
 *
 * The dialog itself lives behind a lazy import in `ChangeDialogBody.tsx`, the
 * way every other dialog in this app does, and for a sharper reason than most:
 * what it hosts is the merge view, which is the heaviest code this app can
 * load. A reader who never asks what changed never pays for it. `open` is
 * always true while this is mounted; the chip's menu mounts it and unmounts it
 * again through `onClose`.
 */

import type { ReactElement } from "react";
import { Suspense, lazy } from "react";

const ChangeDialogBody = lazy(() => import("./ChangeDialogBody"));

export interface ChangeDialogProps {
  /** The domain the file belongs to. */
  domain: string;
  /** The file, as the change routes address it. */
  path: string;
  /** Leave the dialog: dismissed, or closed from its own button. */
  onClose: () => void;
}

export function ChangeDialog(props: ChangeDialogProps): ReactElement {
  return (
    <Suspense
      fallback={
        // Plain markup rather than another Radix dialog: reaching for the
        // primitive here would defeat the point of keeping it out of this
        // chunk.
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40">
          <p className="rounded border border-slate-200 bg-white px-4 py-2 text-sm text-slate-600 shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
            Opening the diff
          </p>
        </div>
      }
    >
      <ChangeDialogBody {...props} />
    </Suspense>
  );
}
