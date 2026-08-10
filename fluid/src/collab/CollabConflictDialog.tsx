/**
 * The room's conflict view: what the file holds now beside what the session
 * holds, and two ways out, neither of which loses a side silently.
 *
 * The dialog is presentational. The SERVER owns the resolution - it holds the
 * pending conflict, applies the first choice that arrives and ignores the
 * later ones - so every participant's view clears when the matching control
 * lands rather than when their own button was the one pressed. What this
 * component guarantees is that nobody picks blind: both texts are on screen
 * while the choice is made, and the caller snapshots the session text into the
 * draft store before handing "theirs" over, exactly as the solo 412 flow does.
 */

import { Dialog } from "radix-ui";
import type { ReactElement } from "react";

import type { CollabConflict } from "./useCollabSession";

export interface CollabConflictDialogProps {
  conflict: CollabConflict;
  /** The live buffer, in session space. */
  mine: string;
  onResolve: (choice: "mine" | "theirs") => void;
  /** Keep editing: the conflict stays pending and saving stays suspended. */
  onClose: () => void;
}

const PANE_CLASSES =
  "max-h-64 overflow-auto rounded border border-slate-200 bg-slate-50 p-2 font-mono text-xs whitespace-pre-wrap dark:border-slate-700 dark:bg-slate-900";

const PLAIN_BUTTON =
  "rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800";

/** The overwrite-shaped choice, marked as one. */
const AMBER_BUTTON =
  "rounded border border-amber-400 bg-amber-50 px-3 py-1 text-sm text-amber-900 hover:bg-amber-100 focus-visible:ring-2 focus-visible:ring-amber-500 focus-visible:outline-none dark:border-amber-700 dark:bg-amber-950 dark:text-amber-100";

/**
 * What stands in for their pane when the file's own text never reached this
 * tab: a client that joined DURING a conflict was not subscribed when the
 * conflict was broadcast, and the re-derivation behind it can fail. An empty
 * pane would read as "their file is empty", which is a different fact.
 */
const THEIRS_UNKNOWN = "The file's text could not be read from here.";

export function CollabConflictDialog({
  conflict,
  mine,
  onResolve,
  onClose,
}: CollabConflictDialogProps): ReactElement {
  const deleted = conflict.kind === "deleted";
  return (
    <Dialog.Root
      open
      onOpenChange={(next) => {
        if (!next) {
          onClose();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-slate-900/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 w-[min(56rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-lg font-semibold">
            {deleted
              ? "This engram's file was deleted outside the session"
              : "This engram changed outside the session"}
          </Dialog.Title>
          {/* The server's own words, verbatim: it knows what happened and
              this tab only knows that something did. */}
          <Dialog.Description className="mt-1 text-sm text-slate-600 dark:text-slate-300">
            {conflict.detail}
          </Dialog.Description>
          <div className={deleted ? "mt-3" : "mt-3 grid gap-3 sm:grid-cols-2"}>
            {!deleted && (
              <div>
                <h3 className="mb-1 text-sm font-semibold">
                  What the file holds now
                </h3>
                <pre className={PANE_CLASSES}>
                  {conflict.theirs ?? THEIRS_UNKNOWN}
                </pre>
              </div>
            )}
            <div>
              <h3 className="mb-1 text-sm font-semibold">
                What the session holds
              </h3>
              <pre className={PANE_CLASSES}>{mine}</pre>
            </div>
          </div>
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            <button type="button" onClick={onClose} className={PLAIN_BUTTON}>
              Keep editing
            </button>
            <button
              type="button"
              onClick={() => {
                onResolve("theirs");
              }}
              className={PLAIN_BUTTON}
            >
              {deleted ? "Accept the deletion" : "Take the file version"}
            </button>
            <button
              type="button"
              onClick={() => {
                onResolve("mine");
              }}
              className={AMBER_BUTTON}
            >
              {deleted
                ? "Restore with the session text"
                : "Keep the session text"}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
