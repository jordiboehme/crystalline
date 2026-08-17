/**
 * The launcher's own seam for the guided retire flow.
 *
 * The dialog primitive, the successor picker and the delete-behind-retire
 * warning all live behind a lazy import in `RetireDialogBody.tsx`, so an
 * engram page that never opens "Retire" never pays for Radix's dialog code.
 * `open` is always true while this is mounted; the launcher mounts it only
 * once "Retire" has been clicked and unmounts it again through `onClose`.
 */

import type { ReactElement } from "react";
import { Suspense, lazy } from "react";

import type { EngramDetail } from "../api/engram";
import type { Backlink } from "../api/graph";

const RetireDialogBody = lazy(() => import("./RetireDialogBody"));

/**
 * The three statuses this dialog offers - the retire endpoint's own
 * contract, not the free-form status list the frontmatter form suggests.
 * Held in `retirement.ts` rather than declared here: a component file that
 * exports anything besides a component breaks fast refresh.
 */
export { RETIREMENT_STATUSES } from "../retirement";

export interface RetireDialogProps {
  /** The engram being retired, checksum included for the delete path. */
  engram: EngramDetail;
  /** Who points here, for the delete warning. */
  backlinks: Backlink[];
  onClose: () => void;
}

export function RetireDialog(props: RetireDialogProps): ReactElement {
  return (
    <Suspense
      fallback={
        // Plain markup rather than another Radix dialog: reaching for the
        // primitive here would defeat the point of keeping it out of this
        // chunk.
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40">
          <p className="rounded border border-slate-200 bg-white px-4 py-2 text-sm text-slate-600 shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
            Opening the retire dialog
          </p>
        </div>
      }
    >
      <RetireDialogBody {...props} />
    </Suspense>
  );
}
