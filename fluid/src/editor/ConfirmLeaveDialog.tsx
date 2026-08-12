/**
 * The one question this editor asks before an in-app navigation.
 *
 * Done promises the work is kept, and on a buffer the findings refuse there is
 * no save to keep it with. Leaving silently was the older behavior and it was
 * the wrong shape of quiet: the text survives, but in the draft store rather
 * than in the file, and nobody was told which. So the exit stays open and
 * becomes a choice, with the count of what blocks the save and the fate of the
 * text both on screen.
 *
 * Keep editing is the default and the safe one, which is why it comes first and
 * wears the tier that reads as the answer. Leaving is the deliberate act here,
 * so it says what it is.
 *
 * The same Radix dialog `ConflictDialog` next door uses, drawn the same way: a
 * second dialog idiom on one screen would be two answers to the same question,
 * and both of these live in the editor's own chunk already.
 */

import { Dialog } from "radix-ui";
import type { ReactElement } from "react";

import { BUTTON } from "../components/primitives";

export interface ConfirmLeaveDialogProps {
  /** How many hard errors hold the save back, for the count in the body. */
  hardErrors: number;
  /** Stay here. The dialog closes and nothing else happens. */
  onKeepEditing: () => void;
  /** Go anyway, with the buffer snapshotted into the draft store first. */
  onLeave: () => void;
}

export function ConfirmLeaveDialog({
  hardErrors,
  onKeepEditing,
  onLeave,
}: ConfirmLeaveDialogProps): ReactElement {
  return (
    <Dialog.Root
      open
      onOpenChange={(next) => {
        if (!next) {
          // Escape and the scrim both mean "not that", which is the same
          // answer Keep editing gives.
          onKeepEditing();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-slate-900/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-lg font-semibold">
            This engram cannot be saved
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-600 dark:text-slate-300">
            {String(hardErrors)} hard{" "}
            {hardErrors === 1 ? "error blocks" : "errors block"} saving. Your
            text is kept as a local draft.
          </Dialog.Description>
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            {/*
              The default, and the one the keyboard lands on: a dialog that
              opened because something was about to be lost should not have the
              losing answer under the first Enter.
            */}
            <button
              type="button"
              autoFocus
              onClick={onKeepEditing}
              className={BUTTON.primary}
            >
              Keep editing
            </button>
            <button
              type="button"
              onClick={onLeave}
              className={BUTTON.secondary}
            >
              Leave anyway
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
