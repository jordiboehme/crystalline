/**
 * Whether Markdown marks are hidden on a line, in the one place it is asked.
 *
 * Two facets and one question. `marksOnEditLine` is the person's setting (off
 * hides marks on the edited line too, on shows them there); `hiddenMarkLayerOn`
 * says the hidden-mark layer is installed at all, which Raw is not.
 */

import type { EditorState, SelectionRange } from "@codemirror/state";
import { Facet, StateField } from "@codemirror/state";
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
 * Provided by the picture tools (`pictures/selectKeys.ts`, a lazy chunk):
 * the keys around a picture are theirs (rulings 3 and 5), so the hidden-mark
 * layer's own arrows and deletes leave a step onto a picture to them
 * (`pictureForKeys`). Until the chunk is in, or when it never loads, the
 * hidden-mark layer keeps its own keys there.
 */
export const pictureKeysOn = Facet.define<boolean, boolean>({
  combine: (values) => values.some(Boolean),
});

/**
 * The picture the tools' keys act on from the edge `pos`: the one that
 * starts at `pos` going `forward`, or ends there going back. Null without
 * the picture layer or the tools, and for a picture in a link or a table
 * cell, which keep their own keys (P2, P5). `pos` is the caret already
 * moved past a run of hidden marks on that side.
 */
export function pictureForKeys(
  state: EditorState,
  pos: number,
  forward: boolean,
): PictureRef | null {
  if (
    !state.facet(pictureKeysOn) ||
    !state.facet(picturesDrawn) ||
    (forward ? pos >= state.doc.length : pos <= 0)
  ) {
    return null;
  }
  // One in from the edge, so a picture that only touches `pos` from the
  // other side is not the one found.
  const ref = pictureAt(state, forward ? pos + 1 : pos - 1);
  if (ref === null || ref.inLink || ref.inCell) {
    return null;
  }
  return (forward ? ref.from : ref.to) === pos ? ref : null;
}

/** Whether `range` is exactly one picture outside a table, as written. */
function onePicture(
  state: EditorState,
  range: { from: number; to: number },
): boolean {
  if (
    range.from === range.to ||
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
 * The main selection when it is exactly one picture that was drawn under its
 * edited line (a block) when it was selected, else null. Selecting a picture
 * never swaps the element under the pointer: one selected where it is drawn
 * in place is no edit of its line, and one selected under its edited line
 * keeps that line edited, so it stays the block it was (P25). Provided by
 * `imagePreviews()`.
 */
export const blockPictureSelection = StateField.define<{
  from: number;
  to: number;
} | null>({
  create: () => null,
  update(value, tr) {
    if (tr.selection === undefined) {
      if (value === null || !tr.docChanged) return value;
      const from = tr.changes.mapPos(value.from, 1);
      const to = tr.changes.mapPos(value.to, -1);
      return from < to ? { from, to } : null;
    }
    const { main } = tr.state.selection;
    if (!onePicture(tr.state, main)) return null;
    // How the picture was drawn just before this selection reached it.
    const back = tr.changes.invert(tr.startState.doc);
    const before = pictureAt(tr.startState, back.mapPos(main.from + 1));
    return before !== null && !pictureInline(tr.startState, before)
      ? { from: main.from, to: main.to }
      : null;
  },
});

/**
 * Whether `range` is a one-picture selection that is no edit of its line:
 * exactly one picture, drawn in place when it was selected.
 */
function selectsInlinePicture(
  state: EditorState,
  range: SelectionRange,
): boolean {
  if (!onePicture(state, range)) return false;
  const block = state.field(blockPictureSelection, false);
  return !(block?.from === range.from && block.to === range.to);
}

/**
 * Whether `line` (1-based) is the edited line of the show-marks mode: the
 * setting is on and a revealing selection range touches the line, other than
 * a range that is exactly one picture drawn in place (`blockPictureSelection`). A selection parked behind an
 * open table cell reveals nothing.
 */
function lineEdited(state: EditorState, line: number): boolean {
  if (!state.facet(marksOnEditLine)) {
    return false;
  }
  const at = state.doc.line(line);
  return revealRanges(state).some(
    (range) =>
      range.from <= at.to &&
      range.to >= at.from &&
      !selectsInlinePicture(state, range),
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
