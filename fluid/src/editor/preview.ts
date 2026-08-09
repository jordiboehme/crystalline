/**
 * The live-preview decoration layer: a read-model over the buffer, never a
 * write-model. Syntax markers (heading #s, emphasis stars, backticks, link
 * brackets and targets) fold away except on the lines a selection touches, so
 * prose reads as prose while the line being edited shows exactly what is
 * written. Nothing here rewrites the document: a decoration is a view over the
 * bytes, and the bytes come back out of the buffer exactly as they went in.
 *
 * The one exception is deliberate and is an ordinary edit rather than a
 * decoration: a task marker is drawn as the checkbox it means, and toggling it
 * dispatches a transaction over the marker's own range, taken from the syntax
 * tree's node offsets. Those offsets are positions in the document, not
 * indexes into the file's string, and they never leave this module as
 * anything else.
 *
 * The frontmatter block is left entirely alone - the parser misreads its
 * fences as thematic breaks, and the form panel is that region's structured
 * view.
 */

import { syntaxTree } from "@codemirror/language";
import type { Extension, Range } from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
} from "@codemirror/view";

import { frontmatterRegion } from "./frontmatterRegion";

/** The formatting marks that fold away off the active lines. */
const FOLDING_MARKS = new Set([
  "HeaderMark",
  "EmphasisMark",
  "CodeMark",
  "LinkMark",
  "URL",
]);

/** A task marker drawn as the checkbox it means. The click IS a text edit. */
class CheckboxWidget extends WidgetType {
  // Assigned in the body rather than declared as constructor parameters:
  // the build erases types, it does not run a TypeScript transform, and
  // parameter properties are syntax that would need one.
  readonly checked: boolean;
  readonly from: number;
  readonly to: number;

  constructor(checked: boolean, from: number, to: number) {
    super();
    this.checked = checked;
    this.from = from;
    this.to = to;
  }

  override eq(other: CheckboxWidget): boolean {
    return (
      other.checked === this.checked &&
      other.from === this.from &&
      other.to === this.to
    );
  }

  /**
   * Flip the marker text in place. The range is the parsed marker's own span,
   * so what is replaced is exactly `[ ]` or `[x]` and nothing around it; the
   * decorations rebuild off the new document and this widget is replaced by
   * one that no longer compares equal.
   */
  private toggle(view: EditorView): void {
    view.dispatch({
      changes: {
        from: this.from,
        to: this.to,
        insert: this.checked ? "[ ]" : "[x]",
      },
    });
  }

  override toDOM(view: EditorView): HTMLElement {
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = this.checked;
    box.className = "cm-task-toggle";
    box.setAttribute(
      "aria-label",
      this.checked ? "Mark task open" : "Mark task done",
    );
    box.addEventListener("mousedown", (event) => {
      // The editor must not also read the press as a click into the hidden
      // marker text and move the selection there.
      event.preventDefault();
      this.toggle(view);
    });
    // Keyboard parity: a focused checkbox answers to Space, and the raw
    // marker is always reachable by putting the cursor on the line, where
    // the widget folds back into the `[ ]` it stands for.
    box.addEventListener("keydown", (event) => {
      if (event.key === " " || event.key === "Enter") {
        event.preventDefault();
        this.toggle(view);
      }
    });
    return box;
  }

  override ignoreEvent(): boolean {
    // The widget owns its events; the editor must not also treat the click
    // as a selection change on the hidden marker text.
    return true;
  }
}

/** The line numbers any selection range touches. */
function activeLines(view: EditorView): Set<number> {
  const lines = new Set<number>();
  for (const range of view.state.selection.ranges) {
    const from = view.state.doc.lineAt(range.from).number;
    const to = view.state.doc.lineAt(range.to).number;
    for (let line = from; line <= to; line += 1) {
      lines.add(line);
    }
  }
  return lines;
}

function buildDecorations(view: EditorView): DecorationSet {
  const decorations: Range<Decoration>[] = [];
  // A node that straddles two visible ranges is entered by both passes, and
  // the same replacement twice over is a decoration set the view rejects.
  const placed = new Set<string>();
  const doc = view.state.doc;
  const active = activeLines(view);
  // The frontmatter block ends here; -1 when there is none, which no node
  // start can fall at or below.
  const frontmatterEnd = frontmatterRegion(doc)?.to ?? -1;

  for (const { from, to } of view.visibleRanges) {
    syntaxTree(view.state).iterate({
      from,
      to,
      enter: (node) => {
        if (node.from <= frontmatterEnd) {
          // Undefined rather than false: the block's own children are skipped
          // by the same test, but the document node starts here too and its
          // later children must still be reached.
          return;
        }
        if (active.has(doc.lineAt(node.from).number)) {
          return;
        }
        const isMark = FOLDING_MARKS.has(node.name);
        const isTask = node.name === "TaskMarker";
        if (!isMark && !isTask) {
          return;
        }
        const key = `${node.from}:${node.to}`;
        if (placed.has(key)) {
          return;
        }
        placed.add(key);
        decorations.push(
          isTask
            ? Decoration.replace({
                widget: new CheckboxWidget(
                  doc.sliceString(node.from, node.to).toLowerCase() === "[x]",
                  node.from,
                  node.to,
                ),
              }).range(node.from, node.to)
            : Decoration.replace({}).range(node.from, node.to),
        );
      },
    });
  }
  return Decoration.set(
    decorations.sort((a, b) => a.from - b.from || a.to - b.to),
    true,
  );
}

const previewPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;

    constructor(view: EditorView) {
      this.decorations = buildDecorations(view);
    }

    update(update: ViewUpdate) {
      if (update.docChanged || update.selectionSet || update.viewportChanged) {
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
  ".cm-task-toggle": { verticalAlign: "middle", margin: "0 0.15em" },
});

/** The whole layer, handed to the caller's on/off compartment. */
export function livePreview(): Extension {
  return [previewPlugin, frontmatterTint, previewTheme];
}
