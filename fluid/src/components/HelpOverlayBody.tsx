/**
 * The shortcut map, written down where somebody can find it.
 *
 * A list of keys rather than a tour: everything on it is a key this app binds
 * itself, so the overlay never promises a shortcut nobody implemented. The
 * palette is first because it is the one shortcut that reaches everything
 * else, including every write action a screen offers.
 */

import { Dialog } from "radix-ui";
import type { ReactElement } from "react";

import type { HelpOverlayProps } from "./HelpOverlay";

/** The map itself: what to press, and what it does. */
const SHORTCUTS: { keys: string; does: string }[] = [
  { keys: "Cmd/Ctrl K", does: "Command palette" },
  { keys: "?", does: "This help" },
  { keys: "Cmd/Ctrl S", does: "Save (in the editor)" },
  { keys: "Cmd/Ctrl F", does: "Find in the document (in the editor)" },
  { keys: "Cmd/Ctrl B", does: "Bold (in the editor)" },
  { keys: "Cmd/Ctrl I", does: "Italic (in the editor)" },
  { keys: "Esc", does: "Close dialogs and menus" },
];

export default function HelpOverlayBody({
  onClose,
}: Pick<HelpOverlayProps, "onClose">): ReactElement {
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
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-lg font-semibold">
            Keyboard shortcuts
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            Every write this screen offers is on the palette too.
          </Dialog.Description>
          <dl className="mt-3 flex flex-col gap-2 text-sm">
            {SHORTCUTS.map((shortcut) => (
              <div key={shortcut.keys} className="flex items-baseline gap-3">
                <dt className="w-32 shrink-0">
                  <kbd className="rounded border border-slate-300 px-1.5 py-0.5 font-mono text-xs dark:border-slate-700">
                    {shortcut.keys}
                  </kbd>
                </dt>
                <dd className="min-w-0">{shortcut.does}</dd>
              </div>
            ))}
          </dl>
          <div className="mt-4 flex justify-end">
            <Dialog.Close className="rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800">
              Close
            </Dialog.Close>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
