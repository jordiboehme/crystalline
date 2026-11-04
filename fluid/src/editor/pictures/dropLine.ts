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
 * Native listeners on `contentDOM` (P12), where the drop handlers run, so a
 * line is drawn only where a drop is taken; the line itself takes no
 * pointer, so a drop over it still reaches the text below. It is redrawn on
 * every dragover, so a chunk loaded mid-drag draws it from the next one.
 */

import type { Extension } from "@codemirror/state";
import { EditorView, ViewPlugin } from "@codemirror/view";

import { writable } from "../elementEdit";
import { carriesPicture, draggingPictureIn } from "./dragMove";
import type { Landing } from "./landing";
import { landingAt } from "./landing";

/** Whether a drag over `view` is one the line is drawn for. */
function linesFor(view: EditorView, event: DragEvent): boolean {
  if (!writable(view.state)) return false;
  const types = Array.from(event.dataTransfer?.types ?? []);
  if (carriesPicture(event)) return draggingPictureIn(view);
  return types.includes("Files");
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
        // A file drag is a copy into the page; the picture move sets its
        // own drop effect.
        if (!carriesPicture(event)) {
          event.preventDefault();
          if (event.dataTransfer) event.dataTransfer.dropEffect = "copy";
        }
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
