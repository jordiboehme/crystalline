/**
 * How large the text is drawn: the app's own size, or a size built for
 * standing across a room from the screen.
 *
 * A frame-level choice for the same reason the width is: a presenter switches
 * it once for a session, and every document and every editor under it follows
 * without being told. Browser zoom does the same job worse - it re-measures a
 * fitted diagram along with the prose and clips the labels inside it, where
 * this leaves diagrams alone on purpose.
 *
 * Kept out of `Layout.tsx` for the reason `layoutWidth.ts` is: a module that
 * exports a component may export nothing else if fast refresh is to work, and
 * the hook is what the screens import.
 */

import { createContext, use } from "react";

/**
 * Where the choice is remembered. Its two values are "large" and "regular";
 * anything else, including nothing at all, reads as the regular size.
 */
export const LAYOUT_TEXT_KEY = "fluid.layout.text";

/**
 * How the text was last left, read once at mount.
 *
 * A browser that refuses storage - a private window with cookies off, an
 * embedded view - is not a reason to fail to draw the frame, so it gets the
 * default and keeps whatever it chooses for the session.
 */
export function storedLargeText(): boolean {
  try {
    return localStorage.getItem(LAYOUT_TEXT_KEY) === "large";
  } catch {
    return false;
  }
}

/** What a screen reads: which size is on, and how to switch it. */
export interface LayoutText {
  /** Whether the frame is at the large size. */
  largeText: boolean;
  /** Switch to the other size and remember it. */
  toggleLargeText: () => void;
}

/**
 * The default is the regular size with nothing to press, which is what a
 * screen rendered outside the frame gets: a preview, a test of one panel.
 * There is no wiring bug to throw over here - a screen with no frame around it
 * is simply a screen at its own size - so this context has a value rather
 * than a null the hook has to guard.
 */
export const LayoutTextContext = createContext<LayoutText>({
  largeText: false,
  toggleLargeText: () => undefined,
});

/** The text size the frame is in, wherever a screen is drawn inside it. */
export function useLargeText(): LayoutText {
  return use(LayoutTextContext);
}
