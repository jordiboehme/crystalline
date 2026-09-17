/**
 * How wide the content is allowed to be: the reading measure, or the whole
 * window.
 *
 * A choice about the shape of the frame rather than about one screen, and the
 * frame holds it, the way it holds whether the sidebar is folded. It does two
 * things at once because neither alone is what anybody asked for: the details
 * column beside the content goes, and the measure the prose is capped at goes
 * with it. Hiding the column while the text stays at seventy characters would
 * hand back the width and then refuse to use it.
 *
 * Kept out of `Layout.tsx` for the reason `commands.tsx` is kept out of its
 * provider: a module that exports a component may export nothing else if fast
 * refresh is to work, and the hook is what the screens import.
 */

import { createContext, use } from "react";

/**
 * Where the choice is remembered. Its two values are "full" and "reading";
 * anything else, including nothing at all, reads as the reading measure.
 */
export const LAYOUT_WIDTH_KEY = "fluid.layout.width";

/**
 * How wide the content was last left, read once at mount.
 *
 * A browser that refuses storage - a private window with cookies off, an
 * embedded view - is not a reason to fail to draw the frame, so it gets the
 * default and keeps whatever it chooses for the session.
 */
export function storedFullWidth(): boolean {
  try {
    return localStorage.getItem(LAYOUT_WIDTH_KEY) === "full";
  } catch {
    return false;
  }
}

/** What a screen reads: which width is on, and how to switch it. */
export interface LayoutWidth {
  /** Whether the content is taking the whole window. */
  fullWidth: boolean;
  /** Switch to the other width and remember it. */
  toggleFullWidth: () => void;
}

/**
 * The default is the reading measure with nothing to press, which is what a
 * screen rendered outside the frame gets: a preview, a test of one panel.
 * There is no wiring bug to throw over here - a screen with no frame around it
 * is simply a screen at its own width - so this context has a value rather
 * than a null the hook has to guard.
 */
export const LayoutWidthContext = createContext<LayoutWidth>({
  fullWidth: false,
  toggleFullWidth: () => undefined,
});

/** The width the frame is in, wherever a screen is drawn inside it. */
export function useFullWidth(): LayoutWidth {
  return use(LayoutWidthContext);
}
