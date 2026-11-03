/**
 * Main's selection parked behind an open table cell. Eager and outside
 * `grid/` (the lazy chunk provides the facet, everything eager reads it):
 * the preview layers, which reveal no marks for a parked selection, and the
 * sticky Formatting bar, which acts on no parked selection.
 */

import type { EditorState, SelectionRange } from "@codemirror/state";
import { Facet } from "@codemirror/state";

/**
 * Whether main's own selection is parked behind an open table cell. While a
 * cell is open the person types in the cell, and main's selection is the
 * stale one from before (the cell sends only its caret), so the live preview
 * reveals no Markdown marks for it. Provided by the lazy chunk from its
 * open-cell intent; a cell view's own state never has it, so the cell keeps
 * revealing its marks.
 */
export const mainSelectionParked = Facet.define<boolean, boolean>({
  combine: (values) => values.some(Boolean),
});

/**
 * The selection ranges a preview layer reveals source for: main's own, or
 * none while they are parked behind an open cell.
 */
export function revealRanges(state: EditorState): readonly SelectionRange[] {
  return state.facet(mainSelectionParked) ? [] : state.selection.ranges;
}

/** Whether a transaction parked main's selection or let it go again. */
export function parkingChanged(tr: {
  startState: EditorState;
  state: EditorState;
}): boolean {
  return (
    tr.startState.facet(mainSelectionParked) !==
    tr.state.facet(mainSelectionParked)
  );
}
