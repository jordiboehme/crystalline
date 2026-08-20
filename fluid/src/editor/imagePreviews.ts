/**
 * The picture below the line that references it.
 *
 * An author who pastes a screenshot gets a path in the buffer, and a path is
 * not what they pasted. This draws the file itself under the line, from the
 * same files route the reading page reads it from, honoring the same placement
 * fragment - so what the editor shows and what a reader will see are one
 * picture rather than two guesses.
 *
 * A widget AFTER the source rather than in place of it, like the mermaid and
 * table previews next door: the reference stays on screen and editable, which
 * is the model's known trade-off and the reason a preview never has to be
 * turned off to fix a typo in a path.
 *
 * A `StateField` rather than a `ViewPlugin`, for the reason `fencePreviews`
 * states: CodeMirror refuses block decorations from a plugin's dynamic source,
 * because a block widget changes how a line is laid out and the editor has to
 * know about it before it draws one.
 *
 * References are read off the syntax tree rather than off the text, which is
 * what keeps a path inside a code fence a path: an `Image` node exists where
 * markdown means an image, and nowhere else.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension, Range } from "@codemirror/state";
import { StateField } from "@codemirror/state";
import type { DecorationSet } from "@codemirror/view";
import { Decoration, EditorView, WidgetType } from "@codemirror/view";

import { attachmentUrl } from "../api/files";
import type { ImageFormat } from "./imageFormat";
import { imageRefsIn, imageStyle } from "./imageFormat";
import { parseAdvanced } from "./parseProgress";

/** What a file that will not load says, in the editor's own quiet voice. */
const UNAVAILABLE = "This image is not available.";

/** One attachment image, drawn under the line that references it. */
class ImagePreviewWidget extends WidgetType {
  // Assigned in the body rather than declared as constructor parameters: the
  // build erases types, it does not run a TypeScript transform, and parameter
  // properties are syntax that would need one.
  readonly src: string;
  readonly format: ImageFormat;

  constructor(src: string, format: ImageFormat) {
    super();
    this.src = src;
    this.format = format;
  }

  /**
   * The whole of what this widget draws is its address and its placement, so
   * two widgets agreeing on both are the same picture - which is what lets a
   * line pushed down the document keep its already-loaded image instead of
   * fetching it again on every Enter above it.
   */
  override eq(other: ImagePreviewWidget): boolean {
    return (
      other.src === this.src &&
      other.format.align === this.format.align &&
      other.format.width === this.format.width
    );
  }

  override toDOM(): HTMLElement {
    const box = document.createElement("div");
    box.className = "cm-image-preview";
    const image = document.createElement("img");
    image.src = this.src;
    // Empty rather than the alt text from the document: the reference itself
    // is one line above with that text in it, and a screen reader reading the
    // decoration would say it twice.
    image.alt = "";
    Object.assign(image.style, imageStyle(this.format));
    // A path that points at nothing is an ordinary state while a reference is
    // being typed, and a browser's own broken-image glyph in a floated box
    // tears the line up. One quiet sentence instead, in place of the picture.
    image.addEventListener("error", () => {
      image.remove();
      const caption = document.createElement("div");
      caption.className = "cm-image-error";
      caption.textContent = UNAVAILABLE;
      box.appendChild(caption);
    });
    box.appendChild(image);
    return box;
  }

  override ignoreEvent(): boolean {
    return true;
  }
}

function buildPreviews(state: EditorState, domain: string): DecorationSet {
  const widgets: Range<Decoration>[] = [];
  const doc = state.doc;
  syntaxTree(state).iterate({
    enter: (node) => {
      if (node.name !== "Image") {
        return;
      }
      for (const ref of imageRefsIn(doc.sliceString(node.from, node.to))) {
        widgets.push(
          Decoration.widget({
            widget: new ImagePreviewWidget(
              // The fragment is a view concern the files route never sees, so
              // only the path travels; the format stays here and is drawn.
              attachmentUrl(domain, ref.path),
              ref.format,
            ),
            block: true,
            side: 1,
            // The picture belongs under the whole line rather than in the
            // middle of a sentence that carries two of them.
          }).range(doc.lineAt(node.to).to),
        );
      }
    },
  });
  return Decoration.set(
    widgets.sort((a, b) => a.from - b.from),
    // Two images on one line produce two widgets at the same position, which
    // is a legal set only when equal starts are admitted explicitly.
    true,
  );
}

/**
 * The previews' own chrome.
 *
 * `.cm-image-preview` and `.cm-image-error` are fresh classes with no
 * competing rule anywhere, written by this module alone, so the cascade trap
 * the fence previews document (base style modules mount REVERSED, equal
 * specificity decided by that order) has nothing to bite on. The caption is
 * the muted slate the fence previews use, for the same reason: a half-typed
 * path is "broken" almost continuously, so this is a hint rather than an
 * alarm.
 */
const previewTheme = EditorView.baseTheme({
  ".cm-image-preview": { padding: "0.5rem 0.75rem" },
  ".cm-image-error": {
    color: "var(--color-slate-500)",
    fontSize: "var(--text-caption, 0.75rem)",
    lineHeight: "var(--text-caption--line-height, 1rem)",
  },
  "&dark .cm-image-error": { color: "var(--color-slate-400)" },
});

/** Inline previews of the attachment images `domain` holds. */
export function imagePreviews(domain: string): Extension {
  const field = StateField.define<DecorationSet>({
    create: (state) => buildPreviews(state, domain),
    update: (value, tr) =>
      tr.docChanged || parseAdvanced(tr)
        ? buildPreviews(tr.state, domain)
        : value,
    provide: (f) => EditorView.decorations.from(f),
  });
  return [field, previewTheme];
}
