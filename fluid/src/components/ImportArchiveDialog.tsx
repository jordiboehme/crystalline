/**
 * The domain screen's seam for importing an archive.
 *
 * The dialog itself lives behind a lazy import in
 * `ImportArchiveDialogBody.tsx`, the way every other dialog in this app does:
 * an import is the rarest write there is - an admin restoring or seeding a
 * domain - and no session should pay for Radix's dialog code, the report table
 * or the archive verbs to open a screen. `open` is always true while this is
 * mounted; the screen mounts it once "Import archive" has been pressed and
 * unmounts it again through `onClose`.
 */

import type { ReactElement } from "react";
import { Suspense, lazy } from "react";

const ImportArchiveDialogBody = lazy(() => import("./ImportArchiveDialogBody"));

export interface ImportArchiveDialogProps {
  /** The domain the archive would be written into. */
  domain: string;
  onClose: () => void;
}

export function ImportArchiveDialog(
  props: ImportArchiveDialogProps,
): ReactElement {
  return (
    <Suspense
      fallback={
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40">
          <p className="rounded border border-slate-200 bg-white px-4 py-2 text-sm text-slate-600 shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
            Opening the import dialog
          </p>
        </div>
      }
    >
      <ImportArchiveDialogBody {...props} />
    </Suspense>
  );
}
