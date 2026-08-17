/**
 * The launcher's own seam: what every "New engram" button imports.
 *
 * The dialog's own weight - the Radix dialog primitive, the folder-picker
 * tree walk, the create mutation - lives behind a lazy import in
 * `CreateEngramDialogBody.tsx`, so an app that never opens the dialog never
 * pays for it. `open` is always true while this is mounted; the launcher
 * mounts it only once "New engram" has been clicked and unmounts it again
 * through `onClose`.
 */

import type { ReactElement } from "react";
import { Suspense, lazy } from "react";

const CreateEngramDialogBody = lazy(() => import("./CreateEngramDialogBody"));

export interface CreateEngramDialogProps {
  domain: string;
  /** The folder the launcher was looking at; "" is the root. */
  initialFolder: string;
  onClose: () => void;
}

export function CreateEngramDialog(
  props: CreateEngramDialogProps,
): ReactElement {
  return (
    <Suspense
      fallback={
        // Plain markup rather than another Radix dialog: reaching for the
        // primitive here would defeat the point of keeping it out of this
        // chunk. A slow link still sees something happen instead of a
        // silent pause between the click and the form.
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40">
          <p className="rounded border border-slate-200 bg-white px-4 py-2 text-sm text-slate-600 shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
            Opening the new engram form
          </p>
        </div>
      }
    >
      <CreateEngramDialogBody {...props} />
    </Suspense>
  );
}
