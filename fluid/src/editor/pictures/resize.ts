/**
 * Free width by dragging a corner handle, in the lazy tools chunk.
 *
 * A native `pointerdown` on `contentDOM` for a `.cm-picture-handle` of a
 * selected attachment takes pointer capture and starts a resize. The press
 * itself is taken (default prevented), so it neither moves the selection
 * nor starts a drag of the picture (P12). While the pointer moves only the
 * image's inline width changes; nothing is dispatched (Review Focus 5).
 * On release the width is written once as whole percent of the column,
 * clamped to 10..100 (ruling 10), through `setPictureWidth`, which is one
 * gesture and so one undo step solo and in a room.
 *
 * A drag ends with nothing written when Escape is pressed, when the pointer
 * capture is lost without a release, when the drawn picture is gone (Raw,
 * a new state, a remote delete) or when the text there is no longer an
 * attachment picture. The picture is followed through every change by its
 * position, so a peer typing above it never moves the write elsewhere.
 */

import type { Extension } from "@codemirror/state";
import type { EditorView, ViewUpdate } from "@codemirror/view";
import { ViewPlugin } from "@codemirror/view";

import { pictureOfElement } from "../imagePreviews";
import { pictureAt } from "./refs";
import { setPictureWidth } from "./verbs";

const MIN_PERCENT = 10;
const MAX_PERCENT = 100;

interface Drag {
  box: HTMLElement;
  image: HTMLElement;
  /** The inline width the image had before the drag, put back at the end. */
  inline: string;
  pointerId: number;
  handle: HTMLElement;
  /** Whether the handle is on the east side: growing to the right. */
  east: boolean;
  /** How far the image grows per pixel of pointer: 2 when it grows on both sides. */
  gain: number;
  /** The share the picture is written with now, if it is a percent. */
  current: number | null;
  startX: number;
  startWidth: number;
  column: number;
  /** The picture's reference as it stands now, mapped through changes. */
  from: number;
  to: number;
  /** The width the pointer asks for now, in pixels. */
  width: number;
}

/** The share a written width is, when it is a percent. */
function percentOf(width: string | undefined): number | null {
  if (width === undefined || !width.endsWith("%")) return null;
  const share = Number.parseFloat(width);
  return Number.isFinite(share) ? share : null;
}

/** The content width of `element`: its width less its horizontal padding. */
function contentWidth(element: HTMLElement): number {
  const style = element.ownerDocument.defaultView?.getComputedStyle(element);
  const pad =
    (Number.parseFloat(style?.paddingLeft ?? "") || 0) +
    (Number.parseFloat(style?.paddingRight ?? "") || 0);
  return element.clientWidth - pad;
}

/**
 * The width `w=N%` is a share of (P41): the content box of the image's
 * containing block, which is the block picture's own box and for a picture
 * in a line the line. The editor column stands in where no layout says.
 */
function columnWidth(content: HTMLElement, box: HTMLElement): number {
  const container = box.classList.contains("cm-picture-block")
    ? box
    : box.closest<HTMLElement>(".cm-line");
  const width = container ? contentWidth(container) : 0;
  return width > 0 ? width : contentWidth(content);
}

const resizeListeners = ViewPlugin.fromClass(
  class {
    readonly view: EditorView;
    drag: Drag | null = null;

    readonly pointerdown = (event: PointerEvent): void => {
      if (this.drag !== null || event.button !== 0) return;
      const target = event.target;
      if (!(target instanceof Element)) return;
      const handle = target.closest<HTMLElement>(".cm-picture-handle");
      const box = handle?.closest<HTMLElement>(".cm-picture") ?? null;
      if (
        !handle ||
        !box ||
        box.closest(".cm-content") !== this.view.contentDOM
      )
        return;
      const image = box.querySelector("img");
      const ref = pictureOfElement(this.view, box);
      if (!image || ref === null || ref.kind !== "attachment") return;
      const column = columnWidth(this.view.contentDOM, box);
      if (!(column > 0)) return;
      // The press is the handle's: no selection change, no drag of the
      // picture, no text selection.
      event.preventDefault();
      event.stopPropagation();
      try {
        handle.setPointerCapture(event.pointerId);
      } catch {
        // The pointer is gone already: the release below never comes either.
      }
      const startWidth = image.getBoundingClientRect().width;
      box.dataset["resizing"] = "true";
      this.drag = {
        box,
        image,
        inline: image.style.width,
        pointerId: event.pointerId,
        handle,
        east: handle.dataset["corner"]?.endsWith("e") === true,
        gain:
          ref.format?.align === "left" || ref.format?.align === "right" ? 1 : 2,
        current: percentOf(ref.format?.width),
        startX: event.clientX,
        startWidth,
        column,
        from: ref.from,
        to: ref.to,
        width: startWidth,
      };
      const doc = this.view.contentDOM.ownerDocument;
      doc.addEventListener("pointermove", this.pointermove);
      doc.addEventListener("pointerup", this.pointerup);
      doc.addEventListener("pointercancel", this.lost);
      doc.addEventListener("keydown", this.keydown, true);
      handle.addEventListener("lostpointercapture", this.lost);
    };

    readonly pointermove = (event: PointerEvent): void => {
      const drag = this.drag;
      if (drag === null || event.pointerId !== drag.pointerId) return;
      const delta = event.clientX - drag.startX;
      const wanted = drag.startWidth + drag.gain * (drag.east ? delta : -delta);
      const low = (drag.column * MIN_PERCENT) / 100;
      drag.width = Math.min(drag.column, Math.max(low, wanted));
      drag.image.style.width = `${String(drag.width)}px`;
    };

    readonly pointerup = (event: PointerEvent): void => {
      const drag = this.drag;
      if (drag === null || event.pointerId !== drag.pointerId) return;
      this.end();
      const percent = Math.min(
        MAX_PERCENT,
        Math.max(MIN_PERCENT, Math.round((drag.width / drag.column) * 100)),
      );
      this.restore(drag);
      if (
        Math.round(drag.width) === Math.round(drag.startWidth) ||
        percent === drag.current
      ) {
        return;
      }
      const ref = pictureAt(this.view.state, drag.from + 1);
      if (
        ref?.from !== drag.from ||
        ref.to !== drag.to ||
        ref.kind !== "attachment"
      ) {
        return;
      }
      setPictureWidth(this.view, percent, ref.from);
    };

    /** The pointer capture was lost without a release, or the pointer was cancelled. */
    readonly lost = (): void => {
      this.cancel();
    };

    readonly keydown = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      this.cancel();
    };

    /** Ends the drag with nothing written and the inline width put back. */
    cancel(): void {
      const drag = this.drag;
      if (drag === null) return;
      this.end();
      this.restore(drag);
      try {
        drag.handle.releasePointerCapture(drag.pointerId);
      } catch {
        // Not captured any more.
      }
    }

    /** Puts the width the format asks for back on the image. */
    restore(drag: Drag): void {
      delete drag.box.dataset["resizing"];
      drag.image.style.width = drag.box.dataset["width"] ?? drag.inline;
    }

    /** Forgets the drag and its listeners. */
    end(): void {
      const drag = this.drag;
      this.drag = null;
      const doc = this.view.contentDOM.ownerDocument;
      doc.removeEventListener("pointermove", this.pointermove);
      doc.removeEventListener("pointerup", this.pointerup);
      doc.removeEventListener("pointercancel", this.lost);
      doc.removeEventListener("keydown", this.keydown, true);
      drag?.handle.removeEventListener("lostpointercapture", this.lost);
    }

    update(update: ViewUpdate): void {
      const drag = this.drag;
      if (drag === null) return;
      // Raw, a reconfigure or a remote delete took the drawn picture away.
      if (!drag.box.isConnected || !drag.handle.isConnected) {
        this.cancel();
        return;
      }
      if (update.docChanged) {
        drag.from = update.changes.mapPos(drag.from, 1);
        drag.to = update.changes.mapPos(drag.to, -1);
      }
    }

    constructor(view: EditorView) {
      this.view = view;
      view.contentDOM.addEventListener("pointerdown", this.pointerdown, true);
    }

    destroy(): void {
      this.cancel();
      this.end();
      this.view.contentDOM.removeEventListener(
        "pointerdown",
        this.pointerdown,
        true,
      );
    }
  },
);

/** The resize handles' drag. */
export const resize: Extension = resizeListeners;
