/**
 * Whether Markdown marks are hidden on a line, in the one place it is asked.
 *
 * Two facets and one question. `marksOnEditLine` is the person's setting (off
 * hides marks on the edited line too, on shows them there); `hiddenMarkLayerOn`
 * says the hidden-mark layer is installed at all, which Raw is not.
 */

import type { EditorState, SelectionRange } from "@codemirror/state";
import { Facet } from "@codemirror/state";
import type { SyntaxNode } from "@lezer/common";

import { revealRanges } from "../parkedSelection";
import type { PictureRef } from "../pictures/refs";
import { pictureAt, pictureOfNode } from "../pictures/refs";

/** The setting: show marks on the line the selection touches. Default off. */
export const marksOnEditLine = Facet.define<boolean, boolean>({
  combine: (values) => values[0] ?? false,
});

/** Provided by `hiddenMarkLayer()`, so a buffer without the layer (Raw) answers false. */
export const hiddenMarkLayerOn = Facet.define<boolean, boolean>({
  combine: (values) => values.some(Boolean),
});

/**
 * Provided by `imagePreviews()`: the picture layer draws inline pictures in
 * place of their references, so the hidden-mark layer leaves the `Image`
 * spans it draws to it (`drawnAsPicture`). Absent in Raw and in a table
 * cell's editor, where the hidden-mark layer keeps its own image rules.
 */
export const picturesDrawn = Facet.define<boolean, boolean>({
  combine: (values) => values.some(Boolean),
});

/**
 * Whether `range` is exactly one picture the picture layer draws: selecting a
 * picture is no edit of its line, so the picture under the pointer stays the
 * picture rather than turning into its source.
 */
function onePicture(state: EditorState, range: SelectionRange): boolean {
  if (
    range.empty ||
    !state.facet(picturesDrawn) ||
    state.doc.sliceString(range.from, range.from + 2) !== "!["
  ) {
    return false;
  }
  const ref = pictureAt(state, range.from + 1);
  return (
    ref !== null &&
    !ref.inCell &&
    ref.from === range.from &&
    ref.to === range.to
  );
}

/**
 * Whether `line` (1-based) is the edited line of the show-marks mode: the
 * setting is on and a revealing selection range touches the line, other than
 * a range that is exactly one drawn picture. A selection parked behind an
 * open table cell reveals nothing.
 */
function lineEdited(state: EditorState, line: number): boolean {
  if (!state.facet(marksOnEditLine)) {
    return false;
  }
  const at = state.doc.line(line);
  return revealRanges(state).some(
    (range) =>
      range.from <= at.to && range.to >= at.from && !onePicture(state, range),
  );
}

/**
 * Whether the marks of `line` (1-based) are hidden. False without the layer.
 * In hidden mode every line hides; in show mode a line stays visible while a
 * revealing selection range touches it, unless that range is exactly one
 * drawn picture.
 */
export function marksHiddenOnLine(state: EditorState, line: number): boolean {
  return state.facet(hiddenMarkLayerOn) && !lineEdited(state, line);
}

/**
 * Whether the picture layer draws `ref` in place of its reference: never in
 * a table (the grid's cell draws it) nor on the edited line of the show-marks
 * mode (the source shows there, with the picture below the line).
 */
export function pictureInline(state: EditorState, ref: PictureRef): boolean {
  return (
    state.facet(picturesDrawn) &&
    !ref.inCell &&
    !lineEdited(state, state.doc.lineAt(ref.from).number)
  );
}

/**
 * Whether `node` is an `Image` the picture layer draws in place: then its span
 * is the picture's, and the hidden-mark layer reads no marks and no atom in it.
 * One test for both layers, so exactly one of them owns each span.
 */
export function drawnAsPicture(state: EditorState, node: SyntaxNode): boolean {
  if (node.name !== "Image" || !state.facet(picturesDrawn)) {
    return false;
  }
  const ref = pictureOfNode(state, node);
  return ref !== null && pictureInline(state, ref);
}

/**
 * Whether a transaction flipped the setting or put the layer in or out, so a
 * layer that asks `marksHiddenOnLine` draws again even with no selection
 * change in it.
 */
export function markModeChanged(tr: {
  startState: EditorState;
  state: EditorState;
}): boolean {
  return (
    tr.startState.facet(marksOnEditLine) !== tr.state.facet(marksOnEditLine) ||
    tr.startState.facet(hiddenMarkLayerOn) !==
      tr.state.facet(hiddenMarkLayerOn) ||
    tr.startState.facet(picturesDrawn) !== tr.state.facet(picturesDrawn)
  );
}
