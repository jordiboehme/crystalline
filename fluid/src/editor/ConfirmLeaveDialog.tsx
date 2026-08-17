/**
 * The one question this editor asks before an in-app navigation.
 *
 * Close leaves; it does not save on the way. That makes a dirty buffer a fork
 * rather than a promise, and all three ways out of it are named here rather
 * than implied: keep the work, throw it away, or stay. Leaving silently was the
 * older behavior and it was the wrong shape of quiet - the text survived, but
 * in the draft store rather than in the file, and nobody was told which.
 *
 * Keeping the work leads, wears the tier that reads as the answer and takes the
 * keyboard, because a dialog that opened because something was about to be lost
 * should have the keeping answer under the first Enter. Discarding says what it
 * is and wears the destructive face: it clears the recovery draft too, so it is
 * the one answer here after which the text is genuinely gone.
 *
 * The count of what blocks the save is on screen whenever anything does, and it
 * says what Save and close will do about it: that answer stays pressable rather
 * than being greyed out, because "this is why nothing happened" is worth more
 * than a control that cannot say why it is dead.
 *
 * The same Radix dialog `ConflictDialog` next door uses, drawn the same way: a
 * second dialog idiom on one screen would be two answers to the same question,
 * and both of these live in the editor's own chunk already.
 */

import { Dialog } from "radix-ui";
import type { ReactElement } from "react";

import { BUTTON } from "../components/primitives";

export interface ConfirmLeaveDialogProps {
  /** How many hard errors hold the save back, or zero when none do. */
  hardErrors: number;
  /** Save the buffer, and leave on the server's receipt. */
  onSaveAndClose: () => void;
  /** Leave without saving, and take the recovery draft with it. */
  onDiscard: () => void;
  /** Stay here. The dialog closes and nothing else happens. */
  onKeepEditing: () => void;
}

export function ConfirmLeaveDialog({
  hardErrors,
  onSaveAndClose,
  onDiscard,
  onKeepEditing,
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
          {/*
            A question rather than a statement, and deliberately not the words
            the header already carries: "Unsaved changes" is the standing line
            beside the Save button, and a dialog wearing the same phrase would
            be the same text in two places saying two different things.
          */}
          <Dialog.Title className="text-lg font-semibold">
            Close the editor?
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-600 dark:text-slate-300">
            {hardErrors > 0
              ? `${String(hardErrors)} hard ${
                  hardErrors === 1 ? "error blocks" : "errors block"
                } saving, so Save and close will keep you here with the findings. Discard changes throws this text away.`
              : "This buffer holds changes the file does not. Save and close writes them; Discard changes throws them away."}
          </Dialog.Description>
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            <button
              type="button"
              onClick={onKeepEditing}
              className={BUTTON.secondary}
            >
              Keep editing
            </button>
            <button
              type="button"
              onClick={onDiscard}
              className={BUTTON.destructive}
            >
              Discard changes
            </button>
            <button
              type="button"
              autoFocus
              onClick={onSaveAndClose}
              className={BUTTON.primary}
            >
              Save and close
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
