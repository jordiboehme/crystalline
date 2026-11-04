/**
 * Moving a picture by dragging it, in the lazy tools chunk.
 *
 * A drag that starts on a drawn picture (not one in a link or a table cell,
 * not one of its handles), as an element drag or as the browser's drag of
 * a selection that holds it, selects it and carries only its reference text as
 * `text/plain`, its address as `text/uri-list` and an opaque token under
 * {@link PICTURE_TYPE}; the token and where the picture began are kept in a
 * module slot. Outside Fluid the browser keeps the plain text and the
 * address. A drop in the same view below a block runs `movePicture` to the
 * landing the drop line marked (`landingAt`): one gesture, judged by the
 * hidden-mark checks, the picture selected where it lands.
 *
 * The dragstart and dragend are native listeners on `contentDOM` (P12): the
 * picture widget keeps both from CodeMirror's handlers. The dragover and the
 * drop are `Prec.highest` handlers, ahead of CodeMirror's own text drop
 * (which would put a copy of the reference beside the move) and of the
 * upload handlers. Every drop that carries the picture type is taken, so a
 * refused or empty move writes nothing at all: one in another Fluid view, one
 * whose drag ended or started elsewhere, one whose picture a peer changed
 * meanwhile (the slot no longer matches the text).
 */

import type { Extension } from "@codemirror/state";
import { Prec } from "@codemirror/state";
import { EditorView, ViewPlugin } from "@codemirror/view";

import { pictureOfElement } from "../imagePreviews";
import { landingAt } from "./landing";
import { pictureAt } from "./refs";
import { movePicture } from "./verbs";

/** The type a picture drag from a Fluid view carries: an opaque token. */
export const PICTURE_TYPE = "application/x-crystalline-picture";

/** The picture being dragged: where it began, in which view, and its text. */
interface Slot {
  view: EditorView;
  from: number;
  /** The reference as it read when the drag began. */
  text: string;
  token: string;
}

let slot: Slot | null = null;

/** Whether a picture of `view` is being dragged now. */
export function draggingPictureIn(view: EditorView): boolean {
  return slot?.view === view;
}

/** Whether a drag carries a picture from a Fluid view. */
export function carriesPicture(event: DragEvent): boolean {
  return Array.from(event.dataTransfer?.types ?? []).includes(PICTURE_TYPE);
}

/** A fresh opaque token. */
function freshToken(): string {
  return `${String(Date.now())}-${Math.random().toString(36).slice(2)}`;
}

/** The absolute form of a picture's address, for `text/uri-list`. */
function absolute(src: string): string {
  try {
    return new URL(src, document.baseURI).href;
  } catch {
    return src;
  }
}

const listeners = ViewPlugin.fromClass(
  class {
    readonly view: EditorView;

    readonly dragstart = (event: DragEvent): void => {
      // A drag from a handle was cancelled by the picture layer (resize).
      if (event.defaultPrevented) return;
      // From a press inside the selection Chrome drags the selection, not
      // the element, and names a text node of the picture as the target.
      const node = event.target;
      const target =
        node instanceof Element
          ? node
          : node instanceof Node
            ? node.parentElement
            : null;
      if (target === null) return;
      if (target.closest(".cm-picture-handle") !== null) return;
      const box = target.closest<HTMLElement>(".cm-picture");
      if (!box || box.closest(".cm-content") !== this.view.contentDOM) return;
      const ref = pictureOfElement(this.view, box);
      if (ref === null || ref.inLink || ref.inCell) return;
      const { view } = this;
      const main = view.state.selection.main;
      // A selection drag of more than the picture is a text drag.
      if (
        !(node instanceof Element) &&
        (main.from !== ref.from || main.to !== ref.to)
      ) {
        return;
      }
      const text = view.state.sliceDoc(ref.from, ref.to);
      const token = freshToken();
      slot = { view, from: ref.from, text, token };
      if (main.from !== ref.from || main.to !== ref.to) {
        view.dispatch({
          selection: { anchor: ref.from, head: ref.to },
          userEvent: "select.pointer",
        });
      }
      const transfer = event.dataTransfer;
      if (!transfer) return;
      // The drawn words a selection drag carries never leave Fluid.
      transfer.clearData();
      transfer.setData("text/plain", text);
      if (ref.src !== null) {
        transfer.setData("text/uri-list", absolute(ref.src));
      }
      transfer.setData(PICTURE_TYPE, token);
      transfer.effectAllowed = "move";
      transfer.setDragImage(box.querySelector("img") ?? box, 0, 0);
    };

    readonly dragend = (): void => {
      if (slot?.view === this.view) slot = null;
    };

    constructor(view: EditorView) {
      this.view = view;
      view.contentDOM.addEventListener("dragstart", this.dragstart);
      view.contentDOM.addEventListener("dragend", this.dragend);
    }

    destroy(): void {
      this.view.contentDOM.removeEventListener("dragstart", this.dragstart);
      this.view.contentDOM.removeEventListener("dragend", this.dragend);
      if (slot?.view === this.view) slot = null;
    }
  },
);

const handlers = Prec.highest(
  EditorView.domEventHandlers({
    dragover(event, view) {
      if (!carriesPicture(event)) return false;
      // Taken (CodeMirror cancels the event), since no drop fires unless
      // the dragover was cancelled. Another view takes no picture, and its
      // "none" tells the browser so.
      if (event.dataTransfer) {
        event.dataTransfer.dropEffect = slot?.view === view ? "move" : "none";
      }
      return true;
    },
    drop(event, view) {
      if (!carriesPicture(event)) return false;
      const taken = slot;
      // The source element may be gone after the move, and its dragend
      // with it, so the slot is cleared here too.
      slot = null;
      const token = event.dataTransfer?.getData(PICTURE_TYPE) ?? "";
      if (taken === null || taken.view !== view || taken.token !== token) {
        return true;
      }
      // Where the picture began, as the text reads now: a peer that
      // changed or moved it meanwhile refuses the move.
      const ref = pictureAt(view.state, taken.from + 1);
      if (
        ref === null ||
        ref.from !== taken.from ||
        view.state.sliceDoc(ref.from, ref.to) !== taken.text
      ) {
        return true;
      }
      const landing = landingAt(view, event);
      if (landing !== null && movePicture(view, ref.from, landing)) {
        view.focus();
      }
      return true;
    },
  }),
);

/** The drag-move of a picture: the native listeners and the drop. */
export const dragMove: Extension = [listeners, handlers];
