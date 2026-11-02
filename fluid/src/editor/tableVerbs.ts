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

import { gridDraws, gridLoaded } from "./grid/draws";
import type { Align, SpanChange, TableModel } from "./tableModel";
import {
  addColumnAfter,
  addColumnBefore,
  addRowAbove,
  addRowBelow,
  columnAt,
  deleteColumn,
  deleteRow,
  moveColumn,
  moveRow,
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

/**
 * The context and the model behind it, parsed once for both readers. `at` names
 * the document position to read the cell from; omitted, it is the main caret.
 * Callers other than the caret (the table grid) pass the position of the cell
 * they mean, and every verb below takes it through unchanged.
 */
function analyze(
  state: EditorState,
  at?: number,
): { context: TableContext; model: TableModel } | null {
  const head = at ?? state.selection.main.head;
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
 * What the format bar needs to know about the caret's table: whether it is in
 * one, and whether the table grid draws that table (the grid is loaded, not
 * in Raw, and `gridDraws` accepts the table). A drawn table's verbs live in
 * the grid's own menu; the bar keeps them for every other table.
 */
export interface TableUiContext {
  inTable: boolean;
  drawn: boolean;
}

function tableUiContext(state: EditorState): TableUiContext {
  const inTable = tableContextAt(state) !== null;
  if (!inTable || !state.facet(gridLoaded)) return { inTable, drawn: false };
  const node = tableNodeAt(state, state.selection.main.head);
  return { inTable, drawn: node !== null && gridDraws(node, state.doc) };
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
  onChange: (context: TableUiContext) => void,
): Extension {
  let last: TableUiContext | null = null;
  return EditorView.updateListener.of((update) => {
    // A parse pass is the third reason to re-derive, beside a moved caret and
    // an edited document: on a document big enough for the language plugin to
    // parse in the background, the tree that first says "this is a table"
    // arrives in an update that changed neither of the other two. The grid
    // loading (or Raw taking it away) is the fourth: it changes `drawn`
    // under a caret that stayed where it was.
    if (
      !update.selectionSet &&
      !update.docChanged &&
      syntaxTree(update.startState) === syntaxTree(update.state) &&
      update.startState.facet(gridLoaded) === update.state.facet(gridLoaded)
    ) {
      return;
    }
    const now = tableUiContext(update.state);
    if (
      last !== null &&
      now.inTable === last.inTable &&
      now.drawn === last.drawn
    ) {
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
/**
 * How a verb runs. `focus: false` leaves the focus where it is: the table
 * grid edits from its own cell editor, and the main view must not take the
 * focus with its caret inside a table it draws (it would publish that caret
 * to a room).
 */
export interface VerbOptions {
  focus?: boolean;
}

function target(
  view: EditorView,
  at?: number,
): { context: TableContext; model: TableModel } | null {
  if (!clearOfFoldedFrontmatter(view)) {
    return null;
  }
  return analyze(view.state, at);
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
  options: VerbOptions,
): boolean {
  if (changes === null || changes.length === 0) {
    return false;
  }
  view.dispatch({ changes: absolute(from, changes), userEvent: "input" });
  if (options.focus ?? true) view.focus();
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
 * `change.from + 1` is the model's own span-relative spelling; the code below
 * looks the line up at `at + 1`, where `at` is that offset already carried
 * into document terms by the same `node.from + change.from` that `absolute`
 * applies to the change. Nothing here reads a span offset as a document one.
 *
 * That means the change is applied twice - once to a scratch `Text` to find
 * the line, once by the transaction - and the duplication is the price, not an
 * oversight. The arithmetic that would avoid it reads the separator's length,
 * which is the one thing this module never does: on a CRLF buffer it would
 * land the caret AT the second line's `|` - the last position still inside
 * the first cell, so typing there gives `|  X|` - on every added row.
 */
export function tableAddRowBelow(
  view: EditorView,
  at?: number,
  options: VerbOptions = {},
): boolean {
  const found = target(view, at);
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
  const inserted = found.context.from + change.from;
  const made = ChangeSet.of(spec, state.doc.length, state.lineBreak).apply(
    state.doc,
  );
  const line = made.lineAt(inserted + 1);
  view.dispatch({
    changes: spec,
    selection: EditorSelection.cursor(line.from + indentWidth(line.text) + 2),
    userEvent: "input",
  });
  if (options.focus ?? true) view.focus();
  return true;
}

/** A new column after the caret's, one insertion per line of the table. */
export function tableAddColumnAfter(
  view: EditorView,
  at?: number,
  options: VerbOptions = {},
): boolean {
  const found = target(view, at);
  if (found === null) {
    return false;
  }
  return dispatch(
    view,
    found.context.from,
    addColumnAfter(found.model, found.context.column),
    options,
  );
}

/** The caret's row. The header and the rule refuse: a GFM table needs both. */
export function tableDeleteRow(
  view: EditorView,
  at?: number,
  options: VerbOptions = {},
): boolean {
  const found = target(view, at);
  if (found === null) {
    return false;
  }
  return dispatch(
    view,
    found.context.from,
    deleteRow(found.model, found.context.row, view.state.lineBreak),
    options,
  );
}

/** The caret's column. The last column refuses: a table needs one. */
export function tableDeleteColumn(
  view: EditorView,
  at?: number,
  options: VerbOptions = {},
): boolean {
  const found = target(view, at);
  if (found === null) {
    return false;
  }
  return dispatch(
    view,
    found.context.from,
    deleteColumn(found.model, found.context.column),
    options,
  );
}

/** The caret's column, aligned - one delimiter cell, nothing else moves. */
export function tableAlignColumn(
  view: EditorView,
  align: Align,
  at?: number,
  options: VerbOptions = {},
): boolean {
  const found = target(view, at);
  if (found === null) {
    return false;
  }
  return dispatch(
    view,
    found.context.from,
    setAlignment(found.model, found.context.column, align),
    options,
  );
}

/** Every pipe lined up. The one verb that rewrites lines, by request. */
export function tablePrettify(
  view: EditorView,
  at?: number,
  options: VerbOptions = {},
): boolean {
  const found = target(view, at);
  if (found === null) {
    return false;
  }
  return dispatch(
    view,
    found.context.from,
    prettify(found.model, view.state.lineBreak),
    options,
  );
}

/**
 * Dispatch a change list as ONE transaction and put the caret inside a named
 * cell of the table AS IT READS AFTERWARDS.
 *
 * The cell is found by applying the changes to a scratch `Text`, mapping the
 * table's end through them, and parsing that span again with the sanctioned
 * read: no cell offset is computed by hand, so the caret is right for every
 * shape the model edits (edge pipes, ragged rows, an indent) and for a CRLF
 * document, where a break is one position whatever it spells. The caret goes
 * one past the cell's opening blank when it has one, which is the interior of
 * an empty `|  |` cell. Should the table not parse afterwards the selection is
 * left to the transaction's own mapping rather than guessed.
 */
function dispatchInCell(
  view: EditorView,
  found: { context: TableContext },
  changes: SpanChange[] | null,
  row: number,
  column: number,
  options: VerbOptions,
): boolean {
  if (changes === null || changes.length === 0) {
    return false;
  }
  const { state } = view;
  const { from, to } = found.context;
  const spec = absolute(from, changes);
  const set = ChangeSet.of(spec, state.doc.length, state.lineBreak);
  const made = set.apply(state.doc);
  const model = parseTable(made.sliceString(from, set.mapPos(to, 1)));
  const line = model?.lines[row];
  const cell = line?.cells[column];
  if (line === undefined || cell === undefined) {
    view.dispatch({ changes: spec, userEvent: "input" });
  } else {
    const blank = cell.raw.startsWith(" ") ? 1 : 0;
    view.dispatch({
      changes: spec,
      selection: EditorSelection.cursor(
        from + line.start + Math.min(cell.from + blank, cell.to),
      ),
      userEvent: "input",
    });
  }
  if (options.focus ?? true) view.focus();
  return true;
}

/**
 * A new empty row above the caret's, with the caret in its first cell. The
 * header and the delimiter refuse: nothing goes above either.
 */
export function tableAddRowAbove(
  view: EditorView,
  at?: number,
  options: VerbOptions = {},
): boolean {
  const found = target(view, at);
  if (found === null) {
    return false;
  }
  return dispatchInCell(
    view,
    found,
    addRowAbove(found.model, found.context.row, view.state.lineBreak),
    found.context.row,
    0,
    options,
  );
}

/** A new column before the caret's, the caret in the new cell of its row. */
export function tableAddColumnBefore(
  view: EditorView,
  at?: number,
  options: VerbOptions = {},
): boolean {
  const found = target(view, at);
  if (found === null) {
    return false;
  }
  return dispatchInCell(
    view,
    found,
    addColumnBefore(found.model, found.context.column),
    found.context.row,
    found.context.column,
    options,
  );
}

/**
 * The caret's data row, one step up (-1) or down (1); the caret follows it.
 * The header and the delimiter neither move nor are passed.
 */
export function tableMoveRow(
  view: EditorView,
  delta: -1 | 1,
  at?: number,
  options: VerbOptions = {},
): boolean {
  const found = target(view, at);
  if (found === null) {
    return false;
  }
  const { row, column } = found.context;
  return dispatchInCell(
    view,
    found,
    moveRow(found.model, row, row + delta, view.state.lineBreak),
    row + delta,
    column,
    options,
  );
}

/** The caret's column, one step left (-1) or right (1); the caret follows it. */
export function tableMoveColumn(
  view: EditorView,
  delta: -1 | 1,
  at?: number,
  options: VerbOptions = {},
): boolean {
  const found = target(view, at);
  if (found === null) {
    return false;
  }
  const { row, column } = found.context;
  return dispatchInCell(
    view,
    found,
    moveColumn(found.model, column, column + delta),
    row,
    column + delta,
    options,
  );
}
