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

import { HANDBOOK_URL } from "../handbook";
import { isMac, keyLabel } from "../platform";
import type { HelpOverlayProps } from "./HelpOverlay";

/** The map itself: what to press, and what it does, named as this keyboard names it. */
const SHORTCUTS: { keys: string; does: string }[] = [
  { keys: keyLabel("Mod-k"), does: "Command palette" },
  { keys: "?", does: "This help" },
  { keys: "\\", does: "Full width for the content" },
  { keys: keyLabel("Mod-s"), does: "Save (in the editor)" },
  { keys: keyLabel("Mod-f"), does: "Find in the document (in the editor)" },
  { keys: keyLabel("Mod-b"), does: "Bold (in the editor)" },
  { keys: keyLabel("Mod-i"), does: "Italic (in the editor)" },
  // A Mac has no menu key, and F10 needs fn there (R111).
  {
    keys: keyLabel(isMac() ? "Ctrl-Enter" : "Shift-F10"),
    does: "Menu for the item you are on (in the editor)",
  },
  {
    keys: keyLabel("Alt-F10"),
    does: "Toolbar for the selection (in the editor and on a Reflection page)",
  },
  { keys: keyLabel("Escape"), does: "Close dialogs and menus" },
];

export default function HelpOverlayBody({
  onClose,
  returnFocusTo,
}: Pick<HelpOverlayProps, "onClose" | "returnFocusTo">): ReactElement {
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
          onCloseAutoFocus={(event) => {
            const target = returnFocusTo;
            if (target) {
              event.preventDefault();
              target.focus();
            }
          }}
          className="fixed top-1/2 left-1/2 z-50 w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900"
        >
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
          <p className="mt-3 text-sm text-slate-500 dark:text-slate-400">
            More in the{" "}
            <a
              href={HANDBOOK_URL}
              target="_blank"
              rel="noreferrer"
              className="text-accent-700 underline hover:no-underline dark:text-accent-300"
            >
              Handbook <span className="sr-only">(opens in a new tab)</span>
            </a>
            .
          </p>
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
