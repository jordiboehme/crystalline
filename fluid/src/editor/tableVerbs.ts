/**
 * Where the caret is in a table, and the dispatch that turns one of
 * `tableModel`'s change lists into a single transaction on the live buffer.
 *
 * This is the whole seam between the pure model and CodeMirror. The model
 * knows tables and nothing else; this file knows the document, the syntax
 * tree and the view, and knows no table grammar at all - it slices a span,
 * hands it over, and maps what comes back.
 *
 * THE CALLING CONVENTION, restated here because it is what makes the mapping
 * one addition. The span is read with `state.doc.sliceString(node.from,
 * node.to)` and NEVER with `state.sliceDoc`: a line break is ONE document
 * position however many characters the separator spells, while `sliceDoc`
 * re-joins with `state.lineBreak`, so on a CRLF document its string is longer
 * than the range it came from and every offset past the first break is
 * inflated - every verb would then edit the wrong place. Read the sanctioned
 * way, a span offset IS a document offset and `node.from + change.from` is the
 * entire translation. The `separator` argument the model takes is insertion
 * TEXT only - `state.lineBreak` - and nothing here derives a position from its
 * length: the new row's caret is read off the line the change actually
 * produced, which is a spelling a CRLF document cannot break.
 *
 * Detection trusts the same syntax tree `fencePreviews` does, at point-lookup
 * cost rather than a document walk: a pipe line inside a fence is a fence, and
 * the parser is the one thing that already knows that. Every verb re-derives
 * span, row and column from `view.state` at dispatch time rather than closing
 * over what the toolbar last rendered, so a click on a stale control degrades
 * to a `false` and never to an edit in the wrong place.
 *
 * WHERE THAT TRUST STOPS. A `Table` node's span is not always table syntax
 * from end to end: under a blockquote whose marks are written on some lines
 * and left off others, the span carries the `> ` of the lines that kept it,
 * and the model - which knows pipes and nothing else - reads that mark as the
 * first cell's content. So the span is checked for the marks of the construct
 * around it before anything is derived from it, and a contaminated span is
 * refused. `quoteMarked` is that check and states the case.
 */

import { syntaxTree } from "@codemirror/language";
import type { ChangeSpec, EditorState, Extension } from "@codemirror/state";
import { ChangeSet, EditorSelection } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { SyntaxNode } from "@lezer/common";

import type { Align, SpanChange, TableModel } from "./tableModel";
import {
  addColumnAfter,
  addRowBelow,
  columnAt,
  deleteColumn,
  deleteRow,
  parseTable,
  prettify,
  setAlignment,
} from "./tableModel";
import { clearOfFoldedFrontmatter } from "./toolbar";

/** Where the caret sits inside a table, in document terms. */
export interface TableContext {
  /** The table span's own document range. */
  from: number;
  to: number;
  /** The caret's line within the span: 0 header, 1 delimiter, 2.. data. */
  row: number;
  /** The caret's column. */
  column: number;
}

/**
 * The innermost enclosing `Table` node at `pos`, or null.
 *
 * Both sides are tried, and the order is load-bearing rather than defensive.
 * A position is a gap BETWEEN two characters and `resolveInner` needs to be
 * told which one to look at: side -1 looks left, side 1 looks right. At the
 * table's very first character - the Home-key caret on the header row - the
 * character to the left belongs to whatever came before the table, so side -1
 * resolves outside it and only the side 1 retry finds the table. One position
 * past the table's last character the situation is exactly mirrored, and side
 * -1 is the one that carries it. Neither side alone covers both ends.
 */
function tableNodeAt(state: EditorState, pos: number): SyntaxNode | null {
  const tree = syntaxTree(state);
  for (const side of [-1, 1] as const) {
    let node: SyntaxNode | null = tree.resolveInner(pos, side);
    while (node !== null) {
      if (node.name === "Table") {
        return node;
      }
      node = node.parent;
    }
  }
  return null;
}

/**
 * Whether a quote mark lands INSIDE the table's own span, which makes the span
 * something other than table text.
 *
 * CommonMark's lazy continuation is what produces it: a blockquote's paragraph
 * content continues on a following line that carries no `> ` at all, so
 *
 *     > | a | b |
 *     | --- | --- |
 *     > | 1 | 2 |
 *
 * is one quoted table, and the parser opens the `Table` node after the FIRST
 * line's mark - leaving the third line's `> ` sitting inside the span. The
 * model would read it as the first cell's content: the caret in `1` would
 * report column 1, an added column would land in the wrong place, and prettify
 * would promote the mark into a cell and lift that row out of the quote
 * entirely. None of that is recoverable from pipes alone, so the whole table
 * is refused instead - the same answer the fully quoted form already gets from
 * the model, which sees an undelimited second line.
 *
 * The question is asked of the TREE rather than of the text, which is what
 * keeps every legal pipe-less form working: a GFM table may drop its leading
 * pipes (`abc | def`), and a rule that refused any line with text before its
 * first pipe would refuse those honest tables along with this dishonest span.
 * A quote mark is the only mark of an enclosing construct that can repeat
 * inside a span this way - a list's marker is written once, on the line the
 * item opens with, which is never inside the table that follows it.
 */
function quoteMarked(state: EditorState, node: SyntaxNode): boolean {
  let found = false;
  syntaxTree(state).iterate({
    from: node.from,
    to: node.to,
    enter: (child) => {
      if (child.name === "QuoteMark") {
        found = true;
      }
      return !found;
    },
  });
  return found;
}

/** The context and the model behind it, parsed once for both readers. */
function analyze(
  state: EditorState,
): { context: TableContext; model: TableModel } | null {
  const head = state.selection.main.head;
  const node = tableNodeAt(state, head);
  if (node === null || quoteMarked(state, node)) {
    return null;
  }
  // The sanctioned read: see the convention at the head of this file.
  const model = parseTable(state.doc.sliceString(node.from, node.to));
  if (model === null) {
    return null;
  }
  const offset = head - node.from;
  for (let row = 0; row < model.lines.length; row += 1) {
    const line = model.lines[row];
    if (line === undefined || offset > line.start + line.text.length) {
      continue;
    }
    return {
      context: {
        from: node.from,
        to: node.to,
        row,
        column: columnAt(line, offset - line.start),
      },
      model,
    };
  }
  // Past every line the model kept - the span's trailing break, which is the
  // break and not a row.
  return null;
}

/**
 * The table the caret is in, or null. Null is the whole refusal channel, so a
 * caller has exactly one thing to check: prose, a pipe line inside a fence, a
 * quote-marked span and a span the model refuses all answer the same way.
 *
 * Deliberately NOT guarded by `clearOfFoldedFrontmatter`, where every verb is:
 * this answers where the caret IS, and moving the caret is not a question's
 * job. The asymmetry is safe in both directions - a folded frontmatter block
 * yields no `Table` node, so the segment does not appear over one, and a verb
 * run from anywhere unfoldable refuses on its own guard a moment later.
 */
export function tableContextAt(state: EditorState): TableContext | null {
  return analyze(state)?.context ?? null;
}

/**
 * Tell a screen when the caret enters or leaves a table.
 *
 * Only transitions are reported, so a toolbar's `useState` is written once per
 * crossing rather than once per keystroke. The remembered value starts at
 * "unknown" rather than at false, because a listener is built fresh every time
 * a buffer is rebuilt while the React state it feeds survives that rebuild: a
 * listener that assumed false would stay silent on the first look after a
 * rebuild that left the caret outside a table, and the segment would go on
 * being drawn until the next genuine crossing. The first look therefore always
 * reports, and a `false` that React already holds costs a bailed-out setState.
 */
export function tableContextListener(
  onChange: (inTable: boolean) => void,
): Extension {
  let last: boolean | null = null;
  return EditorView.updateListener.of((update) => {
    // A parse pass is the third reason to re-derive, beside a moved caret and
    // an edited document: on a document big enough for the language plugin to
    // parse in the background, the tree that first says "this is a table"
    // arrives in an update that changed neither of the other two.
    if (
      !update.selectionSet &&
      !update.docChanged &&
      syntaxTree(update.startState) === syntaxTree(update.state)
    ) {
      return;
    }
    const now = tableContextAt(update.state) !== null;
    if (now === last) {
      return;
    }
    last = now;
    onChange(now);
  });
}

/**
 * Span-relative changes, moved to where the span actually starts - the whole
 * translation, under the convention at the head of this file.
 *
 * An absent `to` is a pure insertion and an absent `insert` a pure deletion,
 * and each key is LEFT OFF rather than set to `undefined`: the two are
 * different things to this project's strict optional types, and a `to:
 * undefined` would not be a change spec at all.
 */
function absolute(from: number, changes: SpanChange[]): ChangeSpec[] {
  return changes.map((change) => {
    const spec: { from: number; to?: number; insert?: string } = {
      from: from + change.from,
    };
    if (change.to !== undefined) {
      spec.to = from + change.to;
    }
    if (change.insert !== undefined) {
      spec.insert = change.insert;
    }
    return spec;
  });
}

/**
 * Everything a verb needs, read fresh from the view at the moment it runs.
 *
 * The fold guard comes first and the state is read AFTER it, because the guard
 * may have moved the caret out of a folded frontmatter block - the same order
 * every command in `toolbar.ts` uses.
 */
function target(
  view: EditorView,
): { context: TableContext; model: TableModel } | null {
  if (!clearOfFoldedFrontmatter(view)) {
    return null;
  }
  return analyze(view.state);
}

/**
 * One dispatch, tagged `userEvent: "input"` so the history keeps the verb its
 * own undo step rather than joining it into the typing around it, then the
 * caret goes back to the buffer.
 *
 * An empty change list is a refusal rather than an empty transaction: it is
 * what `prettify` returns for a table that is already canonical, and a
 * transaction that changes nothing would still cost an undo step.
 */
function dispatch(
  view: EditorView,
  from: number,
  changes: SpanChange[] | null,
): boolean {
  if (changes === null || changes.length === 0) {
    return false;
  }
  view.dispatch({ changes: absolute(from, changes), userEvent: "input" });
  view.focus();
  return true;
}

/** How much whitespace a line opens with, in characters. */
function indentWidth(text: string): number {
  return /^[ \t]*/.exec(text)?.[0].length ?? 0;
}

/**
 * A new empty row below the caret's, with the caret in its first cell.
 *
 * The caret is read off the line the change PRODUCED rather than derived from
 * the separator's length: the inserted break costs exactly one document
 * position whatever it spells, so the new row is the line at `change.from + 1`
 * and its first cell's interior sits two characters past that line's indent -
 * `line.from + indent + 2`, the `|` and the space that follows it. The doc the
 * position is measured in is the one the changes make, computed here so the
 * whole verb is still a single dispatch.
 *
 * That means the change is applied twice - once to a scratch `Text` to find
 * the line, once by the transaction - and the duplication is the price, not an
 * oversight. The arithmetic that would avoid it reads the separator's length,
 * which is the one thing this module never does: on a CRLF buffer it would put
 * the caret one character into the second line's `|` on every added row.
 */
export function tableAddRowBelow(view: EditorView): boolean {
  const found = target(view);
  if (found === null) {
    return false;
  }
  const { state } = view;
  const changes = addRowBelow(found.model, found.context.row, state.lineBreak);
  const change = changes?.[0];
  if (changes === null || change === undefined) {
    return false;
  }
  const spec = absolute(found.context.from, changes);
  const at = found.context.from + change.from;
  const made = ChangeSet.of(spec, state.doc.length, state.lineBreak).apply(
    state.doc,
  );
  const line = made.lineAt(at + 1);
  view.dispatch({
    changes: spec,
    selection: EditorSelection.cursor(line.from + indentWidth(line.text) + 2),
    userEvent: "input",
  });
  view.focus();
  return true;
}

/** A new column after the caret's, one insertion per line of the table. */
export function tableAddColumnAfter(view: EditorView): boolean {
  const found = target(view);
  if (found === null) {
    return false;
  }
  return dispatch(
    view,
    found.context.from,
    addColumnAfter(found.model, found.context.column),
  );
}

/** The caret's row. The header and the rule refuse: a GFM table needs both. */
export function tableDeleteRow(view: EditorView): boolean {
  const found = target(view);
  if (found === null) {
    return false;
  }
  return dispatch(
    view,
    found.context.from,
    deleteRow(found.model, found.context.row, view.state.lineBreak),
  );
}

/** The caret's column. The last column refuses: a table needs one. */
export function tableDeleteColumn(view: EditorView): boolean {
  const found = target(view);
  if (found === null) {
    return false;
  }
  return dispatch(
    view,
    found.context.from,
    deleteColumn(found.model, found.context.column),
  );
}

/** The caret's column, aligned - one delimiter cell, nothing else moves. */
export function tableAlignColumn(view: EditorView, align: Align): boolean {
  const found = target(view);
  if (found === null) {
    return false;
  }
  return dispatch(
    view,
    found.context.from,
    setAlignment(found.model, found.context.column, align),
  );
}

/** Every pipe lined up. The one verb that rewrites lines, by request. */
export function tablePrettify(view: EditorView): boolean {
  const found = target(view);
  if (found === null) {
    return false;
  }
  return dispatch(
    view,
    found.context.from,
    prettify(found.model, view.state.lineBreak),
  );
}
