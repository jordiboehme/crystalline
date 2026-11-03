/**
 * Where the bar at a selection on a Reflection page stands (`SelectionBar`),
 * measured by the threads when they read the selection.
 */

/** Marks the bar, for the threads' "is the focus in the bar" test. */
export const SELECTION_BAR_ATTR = "data-selection-bar";

/** Where the selection stands, in page coordinates (scroll included). */
export interface SelectionBox {
  top: number;
  bottom: number;
  left: number;
}

/** The selection's box in page coordinates; zeros where nothing can be measured. */
export function boxOf(range: Range): SelectionBox {
  // jsdom measures nothing; a real browser always can.
  if (typeof range.getBoundingClientRect !== "function") {
    return { top: 0, bottom: 0, left: 0 };
  }
  const all = range.getBoundingClientRect();
  const first =
    typeof range.getClientRects === "function"
      ? range.getClientRects()[0]
      : undefined;
  return {
    top: all.top + window.scrollY,
    bottom: all.bottom + window.scrollY,
    // Where the passage starts, not the left edge of every line it spans.
    left: (first ?? all).left + window.scrollX,
  };
}
