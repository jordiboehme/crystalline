/**
 * Whether Markdown marks are hidden on a line, in the one place it is asked.
 *
 * Two facets and one question. `marksOnEditLine` is the person's setting (off
 * hides marks on the edited line too, on shows them there); `hiddenMarkLayerOn`
 * says the hidden-mark layer is installed at all, which Raw is not.
 */

import type { EditorState } from "@codemirror/state";
import { Facet } from "@codemirror/state";

import { revealRanges } from "../parkedSelection";

/** The setting: show marks on the line the selection touches. Default off. */
export const marksOnEditLine = Facet.define<boolean, boolean>({
  combine: (values) => values[0] ?? false,
});

/** Provided by `hiddenMarkLayer()`, so a buffer without the layer (Raw) answers false. */
export const hiddenMarkLayerOn = Facet.define<boolean, boolean>({
  combine: (values) => values.some(Boolean),
});

/**
 * Whether the marks of `line` (1-based) are hidden. False without the layer.
 * In hidden mode every line hides; in show mode a line stays visible while a
 * revealing selection range touches it. A selection parked behind an open
 * table cell reveals nothing.
 */
export function marksHiddenOnLine(state: EditorState, line: number): boolean {
  if (!state.facet(hiddenMarkLayerOn)) {
    return false;
  }
  if (!state.facet(marksOnEditLine)) {
    return true;
  }
  const at = state.doc.line(line);
  return !revealRanges(state).some(
    (range) => range.from <= at.to && range.to >= at.from,
  );
}
