/**
 * The sync card's seam for settling one conflict.
 *
 * The dialog itself lives behind a lazy import in `ConflictDialogBody.tsx`, the
 * way every other dialog in this app does: most team domains never have a
 * conflict at all, and a session that only reads the card should not pay for
 * Radix's dialog code plus two more admin verbs to see a number. `open` is
 * always true while this is mounted; the card mounts it once a conflicting path
 * has been pressed and unmounts it again through `onClose`.
 */

import type { ReactElement } from "react";
import { lazyPart } from "./lazyPart";

const ConflictDialogBody = lazyPart(() => import("./ConflictDialogBody"));

export interface ConflictDialogProps {
  /** The team domain the conflict belongs to. */
  domain: string;
  /** The conflict's own id, which is what both routes are addressed by. */
  conflictId: string;
  /** Leave the dialog: cancelled, dismissed, or a conflict that was settled. */
  onClose: () => void;
  /**
   * Set for one of this session's own drafts that does not fit the team's
   * text, and absent for the folder's conflict. With it the dialog reads and
   * settles through the draft routes and says the kind in plain words; what
   * it knows of the conflict before the sides arrive comes from here.
   */
  draft?: {
    kind: string;
    movedFrom: string | null;
    other: string | null;
    file: boolean;
  };
}

export function ConflictDialog(props: ConflictDialogProps): ReactElement {
  return (
    <ConflictDialogBody
      overlay
      fallback={
        // Plain markup rather than another Radix dialog: reaching for the
        // primitive here would defeat the point of keeping it out of this
        // chunk.
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40">
          <p className="rounded border border-slate-200 bg-white px-4 py-2 text-sm text-slate-600 shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
            Opening the conflict
          </p>
        </div>
      }
      {...props}
    />
  );
}
