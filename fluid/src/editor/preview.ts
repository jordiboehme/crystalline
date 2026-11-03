/**
 * The live-preview decoration layer: a read-model over the buffer, never a
 * write-model. Syntax markers (heading #s, emphasis stars, backticks, link
 * brackets and targets) and HTML comments, which the reading page never
 * draws, fold away except on the lines a selection touches, so
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
import type { SyntaxNodeRef } from "@lezer/common";

import { htmlCommentRanges } from "../markdown/htmlComments";
import { isIdLine } from "../reflectionText";
import { frontmatterRegion } from "./frontmatterRegion";
import { parkingChanged, revealRanges } from "./parkedSelection";
import { parseAdvanced } from "./parseProgress";

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

/**
 * The not-applicable marker `[~]`, drawn as a struck box. It is no checkbox
 * and toggles nothing: the third state is written by hand, and a click is a
 * click into the text, which puts the cursor on the line and shows `[~]` as
 * written.
 */
class NotApplicableWidget extends WidgetType {
  override eq(): boolean {
    return true;
  }

  override toDOM(): HTMLElement {
    const box = document.createElement("span");
    box.className = "cm-task-na";
    box.setAttribute("role", "img");
    box.setAttribute("aria-label", "Not applicable");
    return box;
  }

  override ignoreEvent(): boolean {
    return false;
  }
}

/** `[~]` after a list mark and its space, as the reading page reads it. */
const NOT_APPLICABLE = /^[ \t]\[~\](?:[ \t]|$)/;

/**
 * A fenced code block's own delimiter line, which stays visible.
 *
 * `CodeMark` is both an inline backtick and a fence, and folding the fence
 * away strands the info string: the language name is left reading as a
 * paragraph above a block of code with no affordance saying it is one. Until
 * a widget draws the block properly, the fence is the affordance.
 */
function isFence(node: SyntaxNodeRef): boolean {
  return node.name === "CodeMark" && node.node.parent?.name === "FencedCode";
}

/**
 * The line numbers any selection range touches. None while a table cell is
 * open: main's selection is stale then, and the line it sits on is not the
 * one being edited (`mainSelectionParked`).
 */
function activeLines(view: EditorView): Set<number> {
  const lines = new Set<number>();
  for (const range of revealRanges(view.state)) {
    const from = view.state.doc.lineAt(range.from).number;
    const to = view.state.doc.lineAt(range.to).number;
    for (let line = from; line <= to; line += 1) {
      lines.add(line);
    }
  }
  return lines;
}

function buildDecorations(view: EditorView): DecorationSet {
  /*
   * An opening `#` mark folds away with the space after it, so a heading
   * starts at the same left edge as the text under it rather than one space
   * in. Only a space or tab on the same line: a setext underline's mark is a
   * line of its own, and its line break is not the mark's to take.
   */
  const markEnd = (node: SyntaxNodeRef): number => {
    if (node.name !== "HeaderMark") {
      return node.to;
    }
    const line = view.state.doc.lineAt(node.from);
    const next = view.state.doc.sliceString(node.to, node.to + 1);
    return node.from === line.from && (next === " " || next === "\t")
      ? node.to + 1
      : node.to;
  };
  const decorations: Range<Decoration>[] = [];
  // A node that straddles two visible ranges is entered by both passes, and
  // the same replacement twice over is a decoration set the view rejects.
  const placed = new Set<string>();
  const doc = view.state.doc;
  const active = activeLines(view);
  // The frontmatter block ends here; -1 when there is none, which no node
  // start can fall at or below.
  const frontmatterEnd = frontmatterRegion(doc)?.to ?? -1;
  /*
   * An HTML comment is a note for whoever edits the file, and the reading
   * page never draws it (`htmlComments.ts`): off the lines a selection
   * touches it folds away the same way, line by line, since a replacement
   * may not cross a line break. Text after the comment on its line stays,
   * as on the page. A Reflection section's id line is left to
   * `hiddenIdLines`. A comment in code is no comment node and never comes
   * here.
   */
  const hideComments = (from: number, to: number) => {
    for (const comment of htmlCommentRanges(doc.sliceString(from, to))) {
      const start = from + comment.from;
      const end = from + comment.to;
      const first = doc.lineAt(start).number;
      const last = doc.lineAt(end).number;
      for (let number = first; number <= last; number += 1) {
        const line = doc.line(number);
        const pieceFrom = Math.max(start, line.from);
        const pieceTo = Math.min(end, line.to);
        const key = `comment:${pieceFrom}:${pieceTo}`;
        if (
          pieceFrom >= pieceTo ||
          active.has(number) ||
          isIdLine(line.text) ||
          placed.has(key)
        ) {
          continue;
        }
        placed.add(key);
        decorations.push(Decoration.replace({}).range(pieceFrom, pieceTo));
      }
    }
  };

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
        if (node.name === "Comment" || node.name === "CommentBlock") {
          hideComments(node.from, node.to);
          return false;
        }
        if (active.has(doc.lineAt(node.from).number)) {
          return;
        }
        if (
          node.name === "ListMark" &&
          NOT_APPLICABLE.test(doc.sliceString(node.to, node.to + 5))
        ) {
          const key = `na:${node.to}`;
          if (!placed.has(key)) {
            placed.add(key);
            decorations.push(
              Decoration.replace({ widget: new NotApplicableWidget() }).range(
                node.to + 1,
                node.to + 4,
              ),
            );
          }
          return;
        }
        const isMark = FOLDING_MARKS.has(node.name) && !isFence(node);
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
            : Decoration.replace({}).range(node.from, markEnd(node)),
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
      if (
        update.docChanged ||
        update.selectionSet ||
        update.viewportChanged ||
        parseAdvanced(update) ||
        parkingChanged(update)
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
  ".cm-task-toggle": { verticalAlign: "middle", margin: "0 0.15em" },
  // A box the size of the checkbox beside it, struck through once.
  ".cm-task-na": {
    display: "inline-block",
    width: "0.8em",
    height: "0.8em",
    margin: "0 0.2em",
    verticalAlign: "-0.05em",
    border: "1px solid var(--color-slate-400)",
    borderRadius: "2px",
    background:
      "linear-gradient(var(--color-slate-500), var(--color-slate-500)) center / 100% 1px no-repeat",
  },
});

/** The whole layer, handed to the caller's on/off compartment. */
export function livePreview(): Extension {
  return [previewPlugin, frontmatterTint, previewTheme];
}
