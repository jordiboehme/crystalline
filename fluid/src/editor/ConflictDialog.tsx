/**
 * The 412 view: what the server holds now beside what this editor holds, and
 * three ways out, none of which loses the text on either side silently.
 *
 * "Save mine over it" moves the If-Match token forward and PUTs my text -
 * a deliberate overwrite of theirs, chosen with both versions on screen.
 * "Take the server version" replaces the buffer, after the caller snapshots
 * mine to the draft store. Closing does nothing at all.
 */

import { Dialog } from "radix-ui";
import type { ReactElement } from "react";

import type { SaveConflict } from "../api/writes";

export interface ConflictDialogProps {
  conflict: SaveConflict;
  mine: string;
  onOverwrite: () => void;
  onTakeServer: () => void;
  onClose: () => void;
}

const PANE_CLASSES =
  "max-h-64 overflow-auto rounded border border-slate-200 bg-slate-50 p-2 font-mono text-xs whitespace-pre-wrap dark:border-slate-700 dark:bg-slate-900";

export function ConflictDialog({
  conflict,
  mine,
  onOverwrite,
  onTakeServer,
  onClose,
}: ConflictDialogProps): ReactElement {
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
            Someone else saved this engram first
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-600 dark:text-slate-300">
            {conflict.detail}
          </Dialog.Description>
          <div className="mt-3 grid gap-3 sm:grid-cols-2">
            <div>
              <h3 className="mb-1 text-sm font-semibold">
                What the server holds now
              </h3>
              <pre className={PANE_CLASSES}>{conflict.currentContent}</pre>
            </div>
            <div>
              <h3 className="mb-1 text-sm font-semibold">What you have here</h3>
              <pre className={PANE_CLASSES}>{mine}</pre>
            </div>
          </div>
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            <button
              type="button"
              onClick={onClose}
              className="rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 dark:border-slate-700 dark:hover:bg-slate-800"
            >
              Keep editing
            </button>
            <button
              type="button"
              onClick={onTakeServer}
              className="rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 dark:border-slate-700 dark:hover:bg-slate-800"
            >
              Take the server version
            </button>
            <button
              type="button"
              onClick={onOverwrite}
              className="rounded border border-amber-400 bg-amber-50 px-3 py-1 text-sm text-amber-900 hover:bg-amber-100 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-100"
            >
              Save mine over it
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
