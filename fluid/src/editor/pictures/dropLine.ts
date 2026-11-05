/**
 * The drop line: one thin accent line across the content column, at the
 * bottom edge of the block a dropped file or a dragged picture lands below.
 *
 * It reads the pointer the way the drops themselves do (`landingAt`), so
 * wherever it is drawn is exactly where the file or the picture then goes:
 * below a heading's hidden id line, a drawn table, a fence, past the folded
 * frontmatter, after the last line (Review Focus 2). It is drawn only while
 * the drag carries files (the type: during a dragover Chromium lists no
 * files yet) or a picture from this view, on a page that takes writes, and
 * never for a text drag. It goes on a drop, when the drag ends, and when it
 * leaves the editor.
 *
 * Native listeners on `contentDOM` (P12), beside the native drop listeners
 * of the uploads and the drag-move, which take a drop anywhere in the
 * content (P38), so a line is drawn only where a drop is then taken there;
 * never in an open table cell editor (P37). The line itself takes no
 * pointer, so a drop over it still reaches the text below. It is redrawn on
 * every dragover, so a chunk loaded mid-drag draws it from the next one.
 */

import type { Extension } from "@codemirror/state";
import { EditorView, ViewPlugin } from "@codemirror/view";

import { caretPlaced } from "../attachments";
import { writable } from "../elementEdit";
import { carriesPicture, draggingPictureIn } from "./dragMove";
import type { Landing } from "./landing";
import { insideOtherEditor, landingAt } from "./landing";

/**
 * Whether a drag over `view` is one the line is drawn for: exactly the drags
 * a native drop listener of this view then takes and lands at the line
 * (P38). A file needs the upload handlers (`attachmentUploads`); neither
 * lands in an open table cell editor (P37).
 */
function linesFor(view: EditorView, event: DragEvent): boolean {
  if (!writable(view.state) || insideOtherEditor(view, event)) return false;
  if (carriesPicture(event)) return draggingPictureIn(view);
  const types = Array.from(event.dataTransfer?.types ?? []);
  return (
    types.includes("Files") &&
    view.state.field(caretPlaced, false) !== undefined
  );
}

/** A computed padding in pixels, 0 where the browser gives none. */
function pixels(value: string): number {
  const number = Number.parseFloat(value);
  return Number.isFinite(number) ? number : 0;
}

const plugin = ViewPlugin.fromClass(
  class {
    readonly view: EditorView;
    line: HTMLElement | null = null;

    readonly over = (event: DragEvent): void => {
      const landing = linesFor(this.view, event)
        ? landingAt(this.view, event)
        : null;
      if (landing === null) {
        this.hide();
      } else {
        this.show(landing);
      }
    };

    readonly leave = (event: DragEvent): void => {
      const to = event.relatedTarget;
      if (!(to instanceof Node) || !this.view.contentDOM.contains(to)) {
        this.hide();
      }
    };

    readonly end = (): void => {
      this.hide();
    };

    constructor(view: EditorView) {
      this.view = view;
      const dom = view.contentDOM;
      dom.addEventListener("dragenter", this.over);
      dom.addEventListener("dragover", this.over);
      dom.addEventListener("dragleave", this.leave);
      dom.addEventListener("drop", this.end);
      dom.addEventListener("dragend", this.end);
    }

    /** Draws the line at the bottom of the block `landing` names. */
    show(landing: Landing): void {
      const { view } = this;
      const scroller = view.scrollDOM;
      const block = view.lineBlockAt(landing.pos);
      const outer = scroller.getBoundingClientRect();
      const content = view.contentDOM.getBoundingClientRect();
      const style = getComputedStyle(view.contentDOM);
      const left = pixels(style.paddingLeft);
      const right = pixels(style.paddingRight);
      if (this.line === null) {
        const line = document.createElement("div");
        line.className = "cm-drop-line";
        line.setAttribute("aria-hidden", "true");
        scroller.appendChild(line);
        this.line = line;
      }
      const top = view.documentTop - outer.top + scroller.scrollTop;
      this.line.style.top = `${String(top + block.bottom)}px`;
      this.line.style.left = `${String(
        content.left - outer.left + scroller.scrollLeft + left,
      )}px`;
      this.line.style.width = `${String(Math.max(0, content.width - left - right))}px`;
    }

    hide(): void {
      this.line?.remove();
      this.line = null;
    }

    destroy(): void {
      const dom = this.view.contentDOM;
      dom.removeEventListener("dragenter", this.over);
      dom.removeEventListener("dragover", this.over);
      dom.removeEventListener("dragleave", this.leave);
      dom.removeEventListener("drop", this.end);
      dom.removeEventListener("dragend", this.end);
      this.hide();
    }
  },
);

/** The line's look: 2px of the accent, centred on the block's bottom edge. */
const theme = EditorView.baseTheme({
  ".cm-drop-line": {
    position: "absolute",
    height: "2px",
    transform: "translateY(-1px)",
    background: "var(--color-accent-600)",
    borderRadius: "1px",
    pointerEvents: "none",
    zIndex: "3",
  },
  "&dark .cm-drop-line": { background: "var(--color-accent-400)" },
});

/** The drop line for files and dragged pictures. */
export const dropLine: Extension = [plugin, theme];
