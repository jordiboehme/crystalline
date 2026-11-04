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
 * Every listener is native on `contentDOM` (P12). The dragover and the drop
 * listen in the capture phase (P38), so they run ahead of CodeMirror's own
 * text drop (which would put a copy of the reference beside the move) and
 * also over the widgets whose events CodeMirror's handlers never see (a
 * drawn table, a code box header): wherever the drop line is drawn, the
 * drop is taken there. Every drop that carries the picture type is taken,
 * so a refused or empty move writes nothing at all: one in another Fluid
 * view or an open table cell editor (P37), one whose drag ended or started
 * elsewhere. Where the picture stands is mapped through every change
 * during the drag (P35), so a peer typing elsewhere never stops the move;
 * a picture a peer changed or deleted meanwhile is not moved, and the drop
 * says so.
 */

import type { Extension } from "@codemirror/state";
import type { ViewUpdate } from "@codemirror/view";
import { EditorView, ViewPlugin } from "@codemirror/view";

import { pictureOfElement } from "../imagePreviews";
import { insideOtherEditor, landingAt } from "./landing";
import { pictureAt } from "./refs";
import { movePicture } from "./verbs";

/** The type a picture drag from a Fluid view carries: an opaque token. */
export const PICTURE_TYPE = "application/x-crystalline-picture";

/**
 * The picture being dragged: where it stands, mapped through every change
 * while the drag goes on (P35), in which view, and its text.
 */
interface Slot {
  view: EditorView;
  from: number;
  to: number;
  /** The reference as it read when the drag began. */
  text: string;
  token: string;
}

/** Said when a drop is refused because the picture changed during the drag. */
export const PICTURE_CHANGED =
  "The picture changed while you dragged it, so it was not moved.";

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
      slot = { view, from: ref.from, to: ref.to, text, token };
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

    readonly dragover = (event: DragEvent): void => {
      if (!carriesPicture(event)) return;
      // Cancelled wherever the drop below runs, since no drop fires
      // otherwise. Another view and an open cell editor take no picture,
      // and their "none" tells the browser so.
      event.preventDefault();
      if (event.dataTransfer) {
        event.dataTransfer.dropEffect = takesPicture(this.view, event)
          ? "move"
          : "none";
      }
    };

    readonly drop = (event: DragEvent): void => {
      if (!carriesPicture(event)) return;
      // Taken whatever comes of it, so no editor reads the reference as
      // text and puts a copy in.
      event.preventDefault();
      const { view } = this;
      const taken = slot;
      const ours = takesPicture(view, event);
      // The source element may be gone after the move, and its dragend
      // with it, so the slot is cleared here too.
      slot = null;
      const token = event.dataTransfer?.getData(PICTURE_TYPE) ?? "";
      if (!ours || taken === null || taken.token !== token) return;
      // The picture where the drag's mapping says it stands now: one a
      // peer changed or deleted meanwhile is not moved, and that is said.
      const ref = pictureAt(view.state, taken.from + 1);
      if (
        ref?.from !== taken.from ||
        ref.to !== taken.to ||
        view.state.sliceDoc(ref.from, ref.to) !== taken.text
      ) {
        view.dispatch({ effects: EditorView.announce.of(PICTURE_CHANGED) });
        return;
      }
      const landing = landingAt(view, event);
      if (landing !== null && movePicture(view, ref.from, landing)) {
        view.focus();
      }
    };

    constructor(view: EditorView) {
      this.view = view;
      const dom = view.contentDOM;
      dom.addEventListener("dragstart", this.dragstart);
      dom.addEventListener("dragend", this.dragend);
      // Capture: ahead of CodeMirror's own handlers, which skip an event
      // already taken, and over the widgets they never see (P38).
      dom.addEventListener("dragover", this.dragover, true);
      dom.addEventListener("drop", this.drop, true);
    }

    update(update: ViewUpdate): void {
      if (slot?.view === this.view && update.docChanged) {
        slot = {
          ...slot,
          from: update.changes.mapPos(slot.from, 1),
          to: update.changes.mapPos(slot.to, -1),
        };
      }
    }

    destroy(): void {
      const dom = this.view.contentDOM;
      dom.removeEventListener("dragstart", this.dragstart);
      dom.removeEventListener("dragend", this.dragend);
      dom.removeEventListener("dragover", this.dragover, true);
      dom.removeEventListener("drop", this.drop, true);
      if (slot?.view === this.view) slot = null;
    }
  },
);

/** Whether a picture drag over `view` at `event` is one this view takes. */
function takesPicture(view: EditorView, event: DragEvent): boolean {
  return slot?.view === view && !insideOtherEditor(view, event);
}

/** The drag-move of a picture: the native listeners and the drop. */
export const dragMove: Extension = listeners;
