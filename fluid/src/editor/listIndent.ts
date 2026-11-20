/**
 * Tab and Shift+Tab on list items: nest an item under the one before it, or
 * lift it to its parent's level, with everything inside it.
 *
 * The keys are taken only when every line the selection touches starts with
 * a list marker (`-`, `*`, `+`, `1.`, `1)`, a task item included). Anywhere
 * else the command returns false and Tab keeps moving the focus, which is
 * how the page reaches a diagram, the toolbar and everything after the
 * editor. Escape and then Tab still moves the focus out of a list, through
 * CodeMirror's own tab focus mode.
 *
 * The geometry comes from the syntax tree, the CommonMark way:
 *
 * - Tab gives the item the content column of its previous sibling (the
 *   column after that item's marker and the spaces behind it), so `- a` and
 *   `- b` become `- a` and `  - b`, and under `1. a` the indent is three
 *   spaces. An item with no previous sibling cannot nest; the key is taken
 *   and nothing changes, so the focus does not jump away in the middle of a
 *   list.
 * - Shift+Tab gives the item its parent item's marker column. An item at the
 *   top level stays; the key is taken there too. The siblings after a lifted
 *   item become its children, as CommonMark reads the indentation.
 * - The whole item moves: its continuation lines and its children keep their
 *   indentation relative to the marker. Blank lines are never touched.
 * - A list number is never changed (the editor never renumbers).
 *
 * Several selected items move together; an item inside another selected
 * item moves with that one and not again. One press is one undo step. It is
 * part of the surface, outside the preview compartment, so it works in Raw
 * too, and the table cell editor does not have it (a cell keeps its own Tab).
 */

import { completionStatus } from "@codemirror/autocomplete";
import { syntaxTree } from "@codemirror/language";
import type {
  ChangeSpec,
  EditorState,
  Extension,
  Line,
} from "@codemirror/state";
import { Prec } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { keymap } from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";

import { frontmatterRegion } from "./frontmatterRegion";
import { PARSE_BUDGET, deadlineIn, viewParsedTo } from "./marks/parse";
import { dispatchIsolated } from "./roomUndo";

/** The column of `offset` characters into `text`, tabs to the next stop. */
function columnOf(text: string, offset: number, tabSize: number): number {
  let column = 0;
  for (let i = 0; i < offset; i++) {
    column =
      text[i] === "\t" ? column + tabSize - (column % tabSize) : column + 1;
  }
  return column;
}

/** The leading blanks of `text`. */
function leading(text: string): string {
  return /^[ \t]*/.exec(text)?.[0] ?? "";
}

/** The `ListItem` node whose marker starts `line`, or null. */
function itemOn(state: EditorState, line: Line): SyntaxNode | null {
  const at = line.from + leading(line.text).length;
  if (at >= line.to) return null;
  const mark = syntaxTree(state).resolveInner(at, 1);
  if (mark.name !== "ListMark" || mark.from !== at) return null;
  const item = mark.parent;
  return item?.name === "ListItem" ? item : null;
}

/** The marker column of the item `node`. */
function markerColumn(state: EditorState, node: SyntaxNode): number {
  const line = state.doc.lineAt(node.from);
  return columnOf(line.text, node.from - line.from, state.tabSize);
}

/**
 * The content column of the item `node`: after its marker and the spaces
 * behind it, or one past the marker when it has none or five and more.
 */
function contentColumn(state: EditorState, node: SyntaxNode): number {
  const mark = node.getChild("ListMark");
  const line = state.doc.lineAt(node.from);
  if (!mark) return markerColumn(state, node);
  const end = columnOf(line.text, mark.to - line.from, state.tabSize);
  const rest = line.text.slice(mark.to - line.from);
  const blanks = columnOf(rest, leading(rest).length, state.tabSize);
  const behind = rest.length === blanks ? 0 : blanks;
  return end + (behind >= 1 && behind <= 4 ? behind : 1);
}

/** The lines the selection touches; a range ending at a line start skips it. */
function touchedLines(state: EditorState): number[] {
  const lines = new Set<number>();
  for (const range of state.selection.ranges) {
    const first = state.doc.lineAt(range.from).number;
    let last = state.doc.lineAt(range.to).number;
    if (
      !range.empty &&
      last > first &&
      state.doc.line(last).from === range.to
    ) {
      last -= 1;
    }
    for (let n = first; n <= last; n++) lines.add(n);
  }
  return [...lines].sort((a, b) => a - b);
}

/** The items to move: the item of every touched line, outermost only. */
function selectedItems(state: EditorState): SyntaxNode[] | null {
  const fence = frontmatterRegion(state.doc)?.to ?? -1;
  const items: SyntaxNode[] = [];
  for (const number of touchedLines(state)) {
    const line = state.doc.line(number);
    if (line.from <= fence) return null;
    const item = itemOn(state, line);
    if (!item) return null;
    items.push(item);
  }
  return items.filter(
    (item) =>
      !items.some(
        (other) =>
          other !== item && other.from <= item.from && item.to <= other.to,
      ),
  );
}

/** The changes that shift the lines of `item` by `delta` columns. */
function shift(
  state: EditorState,
  item: SyntaxNode,
  delta: number,
  out: ChangeSpec[],
): void {
  if (delta === 0) return;
  const first = state.doc.lineAt(item.from).number;
  const last = state.doc.lineAt(item.to).number;
  for (let n = first; n <= last; n++) {
    const line = state.doc.line(n);
    if (line.text.trim() === "") continue;
    const blanks = leading(line.text);
    if (!blanks.includes("\t")) {
      if (delta > 0) {
        out.push({ from: line.from, insert: " ".repeat(delta) });
      } else if (blanks.length > 0) {
        out.push({
          from: line.from,
          to: line.from + Math.min(-delta, blanks.length),
        });
      }
      continue;
    }
    const column = columnOf(blanks, blanks.length, state.tabSize);
    out.push({
      from: line.from,
      to: line.from + blanks.length,
      insert: " ".repeat(Math.max(0, column + delta)),
    });
  }
}

/** How far Tab moves `item`: to its previous sibling's content column. */
function nestDelta(state: EditorState, item: SyntaxNode): number {
  const sibling = item.prevSibling;
  if (sibling?.name !== "ListItem") return 0;
  return contentColumn(state, sibling) - markerColumn(state, item);
}

/** How far Shift+Tab moves `item`: to its parent item's marker column. */
function liftDelta(state: EditorState, item: SyntaxNode): number {
  const parent = item.parent?.parent;
  if (parent?.name !== "ListItem") return 0;
  return markerColumn(state, parent) - markerColumn(state, item);
}

function moveItems(
  view: EditorView,
  delta: (state: EditorState, item: SyntaxNode) => number,
  userEvent: string,
): boolean {
  if (view.state.readOnly || completionStatus(view.state) === "active") {
    return false;
  }
  const { doc, selection } = view.state;
  const last = selection.ranges.at(-1) ?? selection.main;
  viewParsedTo(view, doc.lineAt(last.to).to, deadlineIn(PARSE_BUDGET.key));
  const { state } = view;
  const items = selectedItems(state);
  if (!items) return false;
  const changes: ChangeSpec[] = [];
  for (const item of items) shift(state, item, delta(state, item), changes);
  if (changes.length === 0) return true;
  const set = state.changes(changes);
  dispatchIsolated(view, {
    changes: set,
    // Associated forward, so a caret at a line start stays on its text and
    // does not end up before the new indentation.
    selection: state.selection.map(set, 1),
    userEvent,
    scrollIntoView: true,
  });
  return true;
}

/** Tab on list items: nest each under its previous sibling. */
export function nestListItems(view: EditorView): boolean {
  return moveItems(view, nestDelta, "indent.more");
}

/** Shift+Tab on list items: lift each to its parent's level. */
export function liftListItems(view: EditorView): boolean {
  return moveItems(view, liftDelta, "indent.less");
}

/** Tab and Shift+Tab on list items, before the default keymap. */
export const listIndentKeymap: Extension = Prec.high(
  keymap.of([
    { key: "Tab", run: nestListItems },
    { key: "Shift-Tab", run: liftListItems },
  ]),
);
