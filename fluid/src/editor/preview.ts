/**
 * The live-preview decoration layer: a read-model over the buffer, never a
 * write-model. Syntax markers (heading #s, emphasis stars, backticks, link
 * brackets and targets, list, task and quote prefixes) and HTML comments,
 * which the reading page never draws, fold away wherever the hidden-mark
 * model says a line hides its marks (`marks/hiddenMarks.ts`): every line by
 * default, and with the setting on every line but the ones a selection
 * touches, which show exactly what is written. Nothing here rewrites the
 * document: a decoration is a view over the bytes, and the bytes come back out
 * of the buffer exactly as they went in.
 *
 * The list, task and quote marks are drawn by `marks/blocks.ts` (bullets,
 * numbers, task boxes, quote bars), so they are left to it here.
 *
 * The frontmatter block is left entirely alone - the parser misreads its
 * fences as thematic breaks, and the form panel is that region's structured
 * view.
 */

import type { Extension, Range } from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import { Decoration, EditorView, ViewPlugin } from "@codemirror/view";

import { frontmatterRegion } from "./frontmatterRegion";
import type { MarkKind } from "./marks/hiddenMarks";
import { marksOnLine } from "./marks/hiddenMarks";
import { markModeChanged, marksOnEditLine } from "./marks/mode";
import { parkingChanged } from "./parkedSelection";
import { parseAdvanced } from "./parseProgress";

/** The marks `marks/blocks.ts` draws in their place. */
const BLOCK_MARKS = new Set<MarkKind>(["list", "task", "quote"]);

/**
 * The hidden marks of the visible lines, from the model (`marks/hiddenMarks.ts`):
 * each one folds away as a plain replace, except the block marks (list, task
 * and quote), which `marks/blocks.ts` draws.
 */
function buildDecorations(view: EditorView): DecorationSet {
  const { state } = view;
  const doc = state.doc;
  const decorations: Range<Decoration>[] = [];
  // A line two visible ranges both reach is drawn once.
  let drawnTo = 0;
  for (const { from, to } of view.visibleRanges) {
    let number = Math.max(doc.lineAt(from).number, drawnTo + 1);
    const last = doc.lineAt(to).number;
    for (; number <= last; number += 1) {
      for (const mark of marksOnLine(state, number)) {
        if (BLOCK_MARKS.has(mark.kind)) {
          continue;
        }
        decorations.push(Decoration.replace({}).range(mark.from, mark.to));
      }
    }
    drawnTo = last;
  }
  return Decoration.set(decorations, true);
}

const hiddenMarkPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = buildDecorations(view);
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        // A selection reveals marks only with the setting on.
        (update.selectionSet && update.state.facet(marksOnEditLine)) ||
        update.viewportChanged ||
        parseAdvanced(update) ||
        parkingChanged(update) ||
        markModeChanged(update)
      ) {
        this.decorations = buildDecorations(update.view);
      }
    }
  },
  {
    decorations: (value) => value.decorations,
    // Hidden text is not a place a cursor may sit: arrowing through a folded
    // mark steps over it rather than into the middle of it.
    provide: (plugin) =>
      EditorView.atomicRanges.of(
        (view) => view.plugin(plugin)?.decorations ?? Decoration.none,
      ),
  },
);

/** A quiet tint over the frontmatter block, so the region reads as metadata. */
const frontmatterTint = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = this.tint(view);
    }

    update(update: ViewUpdate) {
      if (update.docChanged) {
        this.decorations = this.tint(update.view);
      }
    }

    tint(view: EditorView): DecorationSet {
      const region = frontmatterRegion(view.state.doc);
      if (!region) {
        return Decoration.none;
      }
      const lines: Range<Decoration>[] = [];
      const last = view.state.doc.lineAt(region.to).number;
      for (let number = 1; number <= last; number += 1) {
        const line = view.state.doc.line(number);
        lines.push(
          Decoration.line({ class: "cm-frontmatter" }).range(line.from),
        );
      }
      return Decoration.set(lines);
    }
  },
  { decorations: (value) => value.decorations },
);

const previewTheme = EditorView.baseTheme({
  ".cm-frontmatter": { color: "var(--color-slate-500)", fontSize: "0.85em" },
});

/**
 * The hidden marks drawn, for `hiddenMarkLayer()`: outside `livePreview()`, so
 * the table cell editor, which reads the prose layers, does not get it yet.
 */
export const hiddenMarkDrawing: Extension = hiddenMarkPlugin;

/** The whole layer, handed to the caller's on/off compartment. */
export function livePreview(): Extension {
  return [frontmatterTint, previewTheme];
}
