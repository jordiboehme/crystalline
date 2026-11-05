/**
 * The focus handed back to an editor from a control outside it (the
 * floating bar), with the document selection put back first.
 *
 * Chrome puts the caret at the start of a contenteditable that is focused
 * while the document selection is outside it, which it is after a text
 * field had the focus. CodeMirror's `focus()` writes the selection to the
 * DOM only when its cached DOM selection differs from its own; when the
 * cache still reads the editor's selection (the field's `selectionchange`
 * had not arrived yet), it writes nothing, and its observer then reads
 * Chrome's caret at 0,0 as one the person made (I1). With the document
 * selection already where the editor's is, the browser has nothing to move.
 */

import type { EditorView } from "@codemirror/view";

/** Focuses `view` with the document selection at its main selection. */
export function focusBack(view: EditorView): void {
  if (!view.hasFocus) {
    const { main } = view.state.selection;
    try {
      const anchor = view.domAtPos(main.anchor);
      const head = view.domAtPos(main.head);
      view.dom.ownerDocument
        .getSelection()
        ?.setBaseAndExtent(anchor.node, anchor.offset, head.node, head.offset);
    } catch {
      // A position outside the drawn viewport: CodeMirror's focus decides.
    }
  }
  view.focus();
}
