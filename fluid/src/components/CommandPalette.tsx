/**
 * The keyboard's way around the app.
 *
 * Cmd+K, or Ctrl+K on a keyboard without a Cmd key, anywhere in the app. What
 * it offers is the two things this instance is addressed by: the domains it
 * holds, filtered here because the whole list already arrived with the sidebar,
 * and engrams by title, which the server answers because only the index knows
 * what is in the other domains.
 *
 * Above both sits what the screen behind the palette can do right now, which
 * the screen itself registers (`commands.tsx`). It leads because it is the one
 * group that does not leave: every row under it is a jump somewhere else, and
 * the thing a reader most often opens the palette for is the thing in front of
 * them.
 *
 * What the frame registers is the exception, and it goes last. A row that is
 * identical on every screen is chrome rather than "what you are looking at",
 * and putting it first would have made the app's least specific action the
 * default Enter on the screens that offer nothing of their own.
 *
 * Titles only, on purpose. A palette is for reaching something whose name you
 * half remember, and a body-text match dressed as a title would send a reader
 * somewhere they did not ask for. What that leaves out is the last row: a query
 * with no title behind it is handed to the search screen, which searches the
 * text this never asked about. So the palette is never a dead end, and it never
 * pretends to have looked further than it did.
 *
 * Typing is debounced at the same pause the search screen uses, so a query is
 * one request rather than one per keystroke.
 *
 * This file is the shell and stays in the entry bundle: the shortcut, the text
 * typed so far and the way back to the focus. The palette itself, with cmdk
 * behind it, is `CommandPaletteBody.tsx` and is fetched the first time the
 * palette opens. Until it arrives a plain box takes the keys, so the first
 * press of the shortcut followed by typing loses nothing.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactElement } from "react";

import { isMac } from "../platform";
import { Loading } from "./Loading";
import { lazyPart } from "./lazyPart";

const PaletteBody = lazyPart(() => import("./CommandPaletteBody"));

/** The event the palette opens on besides its shortcut. */
const OPEN_EVENT = "fluid:palette";

/**
 * Open the palette from a control rather than the keyboard (the phone's
 * search icon), with the element focus goes back to when it closes.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function openPalette(invoker: HTMLElement): void {
  window.dispatchEvent(new CustomEvent(OPEN_EVENT, { detail: invoker }));
}

export function CommandPalette(): ReactElement | null {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  // Whatever had the focus when the shortcut fired, so closing can hand it
  // back. A dialog opened by a key has no trigger to return to on its own.
  const invoker = useRef<HTMLElement | null>(null);

  /**
   * Shut the palette, and let go of everything a shut palette must forget.
   *
   * Every way out goes through here, which is the point: a jump closes the
   * palette from the inside and never reaches the dialog's own `onOpenChange`,
   * so a reset hanging off that callback would fire on every exit except the
   * common one and the next Cmd+K would open onto the last question. The body
   * is unmounted while the palette is shut, which drops its own highlight and
   * pending lookup with it.
   */
  const close = useCallback(() => {
    setOpen(false);
    setQuery("");
  }, []);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      // A key something on the page already took is theirs: the editor's
      // Mod-k opens its Link popover and prevents the default, and the
      // palette opening over it would take the focus and close it again.
      //
      // Either modifier off a Mac, so a reader need not know which one this
      // app was built on. On a Mac Cmd+K only: Ctrl+K there deletes to the
      // end of the line in a text field (R111).
      if (
        event.defaultPrevented ||
        event.key.toLowerCase() !== "k" ||
        !(event.metaKey || (event.ctrlKey && !isMac()))
      ) {
        return;
      }
      event.preventDefault();
      if (open) {
        close();
        return;
      }
      invoker.current =
        document.activeElement instanceof HTMLElement
          ? document.activeElement
          : null;
      setOpen(true);
    }
    function onOpen(event: Event) {
      invoker.current = (event as CustomEvent<HTMLElement>).detail;
      setOpen(true);
    }
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener(OPEN_EVENT, onOpen);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener(OPEN_EVENT, onOpen);
    };
  }, [open, close]);

  // The focus, handed back after the dialog is gone rather than while closing
  // it: the palette traps focus for as long as it is mounted, and an invoker
  // focused a moment too early would be pulled straight back in.
  useEffect(() => {
    if (open) {
      return;
    }
    const back = invoker.current;
    invoker.current = null;
    // Only where it still exists: a jump replaces the screen the palette was
    // opened from, and focusing what is left of it would be focusing nothing.
    if (back?.isConnected === true) {
      back.focus();
    }
  }, [open]);

  if (!open) {
    return null;
  }
  return (
    <PaletteBody
      overlay
      fallback={
        <PaletteLoading
          query={query}
          onQueryChange={setQuery}
          onClose={close}
        />
      }
      query={query}
      onQueryChange={setQuery}
      onClose={close}
    />
  );
}

/**
 * What stands in for the palette while its chunk is on the way: the same box
 * in the same place, holding the keys already pressed, with the shared
 * loader beneath it. It is not a dialog, so the real one is the first
 * `dialog` anything finds.
 */
function PaletteLoading({
  query,
  onQueryChange,
  onClose,
}: {
  query: string;
  onQueryChange: (query: string) => void;
  onClose: () => void;
}) {
  return (
    <div className="fixed inset-0 z-40 bg-slate-950/25 dark:bg-slate-950/50">
      <div className="fixed top-24 left-1/2 z-50 w-[min(36rem,calc(100vw-2rem))] -translate-x-1/2 overflow-hidden rounded border border-slate-200 bg-white shadow-xl dark:border-slate-700 dark:bg-slate-900">
        <input
          autoFocus
          aria-label="Command palette"
          value={query}
          onChange={(event) => {
            onQueryChange(event.target.value);
          }}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              onClose();
            }
          }}
          placeholder="Jump to a domain, or find an engram by title"
          className="w-full border-b border-slate-200 bg-transparent px-3 py-3 text-sm outline-none dark:border-slate-800"
        />
        <Loading compact label="Loading the command palette" className="p-3" />
      </div>
    </div>
  );
}
