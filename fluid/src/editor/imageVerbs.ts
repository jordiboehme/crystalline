/**
 * Where the caret is in an attachment image, and the one edit the format menu
 * makes.
 *
 * The seam `tableVerbs` is for tables: the convention itself is pure and lives
 * in `imageFormat`, and this file knows the document, the syntax tree and the
 * view. Both verbs re-derive the reference from `view.state` at dispatch time
 * rather than closing over what the toolbar last rendered, so a click that
 * beats a stale render refuses instead of editing the wrong place.
 *
 * What is replaced is the TARGET and nothing else. The alt text is the
 * author's own sentence - it may hold brackets, parentheses, anything - and a
 * verb that rebuilt the whole reference from its parts would quietly normalize
 * somebody's words. The target's own span comes from the parsed reference, so
 * the change is exactly the characters between the parentheses.
 *
 * Detection trusts the syntax tree rather than the line text, the way the
 * previews do: an `Image` node exists where markdown means an image and
 * nowhere else, so a path written inside a code fence is left to be code.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { EditorView as View, ViewPlugin } from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";

import type { ImageAlign, ImageFormat, ImageRef } from "./imageFormat";
import { buildImageTarget, imageRefsIn } from "./imageFormat";
import { clearOfFoldedFrontmatter } from "./toolbar";

/** One attachment image the caret is on, in document terms. */
export type ImageContext = ImageRef;

/**
 * The innermost enclosing `Image` node at `pos`, or null.
 *
 * Both sides are tried, for the reason `tableVerbs` states: a position is a
 * gap BETWEEN two characters, and at the reference's first character only a
 * look to the right finds it while one position past its last only a look to
 * the left does.
 */
function imageNodeAt(state: EditorState, pos: number): SyntaxNode | null {
  const tree = syntaxTree(state);
  for (const side of [-1, 1] as const) {
    let node: SyntaxNode | null = tree.resolveInner(pos, side);
    while (node !== null) {
      if (node.name === "Image") {
        return node;
      }
      node = node.parent;
    }
  }
  return null;
}

/**
 * The attachment image the caret sits on, or null.
 *
 * Null for an external image and for a link to a document as well as for
 * prose: neither is a picture this app places, so the menu has nothing to
 * offer about either.
 */
export function imageContextAt(state: EditorState): ImageContext | null {
  const pos = state.selection.main.head;
  const node = imageNodeAt(state, pos);
  if (node === null) {
    return null;
  }
  const [ref] = imageRefsIn(state.doc.sliceString(node.from, node.to));
  if (ref === undefined) {
    return null;
  }
  return {
    from: node.from + ref.from,
    to: node.from + ref.to,
    targetFrom: node.from + ref.targetFrom,
    targetTo: node.from + ref.targetTo,
    written: ref.written,
    path: ref.path,
    format: ref.format,
  };
}

/**
 * Rewrite the target of the image the caret is on, or refuse.
 *
 * One dispatch tagged `input` rather than `input.type`: the history joins
 * adjacent events only for typing, so a format change is one undo step of its
 * own and never merges into the words around it.
 */
function rewrite(
  view: EditorView,
  next: (format: ImageFormat) => ImageFormat,
): boolean {
  if (!clearOfFoldedFrontmatter(view)) {
    return false;
  }
  const context = imageContextAt(view.state);
  if (context === null) {
    return false;
  }
  // Rebuilt on the path AS WRITTEN rather than on the resolved one: a
  // reference spelled `./assets/a.png` resolves to the same file and is the
  // author's spelling, so a placement change must not quietly normalize it.
  const target = buildImageTarget(context.written, next(context.format));
  if (
    target === view.state.doc.sliceString(context.targetFrom, context.targetTo)
  ) {
    // Nothing to say: an author picking the placement an image already has
    // should not put a no-op on the undo stack.
    view.focus();
    return true;
  }
  view.dispatch({
    changes: {
      from: context.targetFrom,
      to: context.targetTo,
      insert: target,
    },
    userEvent: "input",
  });
  // The bar cancels its own mousedown so the caret never left the buffer;
  // the menu that opened over it did take focus, and this hands it back.
  view.focus();
  return true;
}

/** Place the image, keeping whatever width it already carries. */
export function setImageAlign(view: EditorView, align: ImageAlign): boolean {
  return rewrite(view, (format) =>
    format.width === undefined ? { align } : { align, width: format.width },
  );
}

/**
 * Put the image back to the default: a centered block at its own size, which
 * is a bare path again.
 *
 * The menu's way out, and the reason it is a verb of its own rather than
 * `setImageAlign(view, "center")`: an author who tried a float at half width
 * and changed their mind means "as it was", and a centered image still
 * carrying `w=25%` is not what an upload wrote.
 */
export function clearImageFormat(view: EditorView): boolean {
  return rewrite(view, () => ({ align: "center" }));
}

/** Size the image, keeping wherever it already stands. `null` clears the width. */
export function setImageWidth(view: EditorView, width: string | null): boolean {
  return rewrite(view, (format) =>
    width === null ? { align: format.align } : { align: format.align, width },
  );
}

/**
 * Tell a screen when the caret crosses into or out of an attachment image, so
 * the format menu is drawn only where it has something to act on.
 *
 * Crossings only, like the table listener beside it: this costs a render when
 * the answer changes rather than one per keystroke.
 */
export function imageContextListener(
  onChange: (onImage: boolean) => void,
): Extension {
  let last: boolean | null = null;
  /** The crossing watch itself, shared by the plugin and the listener below. */
  const cross = (state: EditorState) => {
    const now = imageContextAt(state) !== null;
    if (now === last) {
      return;
    }
    last = now;
    onChange(now);
  };
  return [
    // A buffer can MOUNT with the caret already on an image - a screen that
    // restores a position, an editor reopened where it was left - and an
    // update listener is told about changes rather than about the state it
    // started in, so the menu would stay hidden until something moved. The
    // plugin's own construction is the one place that state is seen.
    ViewPlugin.define((view) => {
      cross(view.state);
      return {};
    }),
    View.updateListener.of((update) => {
      // A parse pass is the third reason to re-derive, beside a moved caret and
      // an edited document: on a document big enough to parse in the
      // background, the tree that first says "this is an image" arrives in an
      // update that changed neither of the other two.
      if (
        !update.selectionSet &&
        !update.docChanged &&
        syntaxTree(update.startState) === syntaxTree(update.state)
      ) {
        return;
      }
      cross(update.state);
    }),
  ];
}
