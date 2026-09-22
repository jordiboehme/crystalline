/**
 * Both sides of one page's unshared change, opened from the chip beside the
 * title. The dialog is titled by the path, hosts the pane the share dialog
 * hosts, and has one way out.
 */

import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useId } from "react";

import type { ChangeDialogProps } from "./ChangeDialog";
import { DiffPane } from "./DiffPaneLazy";
import { BUTTON } from "./primitives";

export default function ChangeDialogBody({
  domain,
  path,
  onClose,
}: ChangeDialogProps): ReactElement {
  const heading = useId();
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
        <Dialog.Content
          aria-labelledby={heading}
          className="fixed top-1/2 left-1/2 z-50 w-[min(64rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900"
        >
          <Dialog.Title id={heading} className="font-mono text-sm break-all">
            {path}
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            The team's copy against yours.
          </Dialog.Description>
          <div className="mt-3 flex flex-col gap-3">
            <DiffPane domain={domain} path={path} headingId={heading} />
            <div className="flex justify-end">
              <button
                type="button"
                autoFocus
                onClick={onClose}
                className={BUTTON.primary}
              >
                Close
              </button>
            </div>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
