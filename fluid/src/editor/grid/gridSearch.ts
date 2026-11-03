/**
 * The main view's search and replace, reaching into the grid.
 *
 * The search panel moves the main view's selection, and inside a table that
 * selection sits under the grid, where nobody sees it. So:
 *
 * - **A hit inside a cell** (a selection set by `select.search` or
 *   `input.replace`) opens that cell with the hit selected, through
 *   `setOpenCell` and `hostOpenCell`. The cell takes the focus only when the
 *   main view or the open cell had it; with the focus in the search panel
 *   the panel keeps it, and the next hit moves the open cell. A table that
 *   is not drawn mounts the cell when it is (remount from intent).
 * - **A hit outside every table**, or a selection the user moved there in
 *   the main view, closes the open cell whatever has the focus. Selections
 *   set with no select event (an undo putting main's selection back) never
 *   close a cell.
 * - **Read-only (R17)**: a hit opens no editor; its cell is marked
 *   `cm-grid-hit` instead. On a writable page the same mark rings the open
 *   cell holding the hit, because neither view draws a selection it does not
 *   have the focus for.
 * - **Main taking the focus inside a table** (closing the panel focuses it
 *   on the last hit): the focus goes on to the cell there, so main never
 *   publishes a caret inside a table; see `handBack`.
 *
 * - **Replace inside a cell (R20)** escapes pipes and flattens line breaks
 *   in the same transaction, so a replacement never changes a table's shape.
 *
 * The cell view's own search keys (Mod-f, F3, Mod-g) live in `cellEditor.ts`.
 */

import { syntaxTree } from "@codemirror/language";
import type { Extension, TransactionSpec } from "@codemirror/state";
import {
  ChangeSet,
  EditorState,
  StateEffect,
  Transaction,
} from "@codemirror/state";
import { EditorView } from "@codemirror/view";

import { fromCell, withEdges, withEscapes } from "./cellEditor";
import { cellElement, hostOpenCell, tableOf } from "./gridWidget";
import {
  closeCell,
  gridWritable,
  isSearchFromCell,
  mountedCell,
  openCellField,
  openCellRange,
  setOpenCell,
} from "./openCell";
import type { GridCell } from "./structure";
import { cellAt, gridTableAt } from "./structure";

const HIT = "cm-grid-hit";

/** The position of the last search hit inside a cell, per main view. */
const hits = new WeakMap<EditorView, number>();
/** The cell element carrying the hit mark, per main view. */
const marked = new WeakMap<EditorView, HTMLElement>();

/** Whether the parse has reached the line of `pos`. */
function parsedAt(main: EditorView, pos: number): boolean {
  return syntaxTree(main.state).length >= main.state.doc.lineAt(pos).to;
}

/**
 * Closes the open cell for a selection outside every table, leaving the
 * focus where it is - unless the cell had it, which then goes to the main
 * view. Not `"main"`: that would move main's selection off the hit (R6).
 */
function leaveTables(main: EditorView): void {
  hits.delete(main);
  const mounted = mountedCell(main);
  if (!mounted && !main.state.field(openCellField, false)) return;
  const hadFocus = isSearchFromCell(main) || (mounted?.view.hasFocus ?? false);
  closeCell(main, "user", "none");
  if (hadFocus) main.focus();
}

/** A search hit at main's selection. */
function onHit(main: EditorView): void {
  const range = main.state.selection.main;
  const parsed = parsedAt(main, range.head);
  const table = parsed ? gridTableAt(main.state, range.head) : null;
  if (parsed && !table) {
    leaveTables(main);
    return;
  }
  if (table) {
    const cell = cellAt(table, range.head);
    // On a pipe or the delimiter row: no cell to show, the open one stays.
    if (!cell) {
      hits.delete(main);
      return;
    }
    hits.set(main, range.head);
  } else hits.set(main, range.head); // not parsed yet: pending
  if (!gridWritable(main.state)) return; // R17: marked, never opened
  const mounted = mountedCell(main);
  const active = document.activeElement;
  const focus =
    isSearchFromCell(main) ||
    active === main.contentDOM ||
    (mounted !== null && mounted.view.dom.contains(active))
      ? "now"
      : "none";
  main.dispatch({
    effects: setOpenCell.of({
      anchor: range.anchor,
      head: range.head,
      focused: focus === "now",
    }),
  });
  hostOpenCell(main, focus);
}

/** A selection the user moved in the main view. */
function onMove(main: EditorView): void {
  hits.delete(main);
  const head = main.state.selection.main.head;
  if (parsedAt(main, head) && !gridTableAt(main.state, head)) {
    leaveTables(main);
  }
}

/**
 * The main view took the focus with its selection inside a table: closing
 * the search panel does that, with main's selection still on a hit. A
 * focused main view publishes its caret, and a caret inside a table would
 * sit frozen in a peer's grid (R6). So the focus goes on to the cell under
 * main's selection, opened there when it is not open yet; where no cell can
 * open (a read-only page, a pipe, the delimiter row) main's selection leaves
 * the table instead, to the start of the line after it.
 */
function handBack(main: EditorView): void {
  const { anchor, head } = main.state.selection.main;
  if (!parsedAt(main, head)) return;
  const table = gridTableAt(main.state, head);
  if (!table) return;
  const cell = cellAt(table, head);
  if (cell && gridWritable(main.state)) {
    const open = openCellRange(main.state);
    if (open?.span.from !== cell.span.from) {
      main.dispatch({
        effects: setOpenCell.of({ anchor, head, focused: true }),
      });
    }
    hostOpenCell(main, "now");
    return;
  }
  const doc = main.state.doc;
  const last = doc.lineAt(table.to);
  const pos =
    last.number < doc.lines ? doc.line(last.number + 1).from : table.to;
  main.dispatch({ selection: { anchor: pos } });
}

/** The cell element the hit at `pos` is in, if its table is drawn. */
function hitElement(main: EditorView, pos: number): HTMLElement | null {
  const table = gridTableAt(main.state, pos);
  const cell = table ? cellAt(table, pos) : null;
  if (!table || !cell) return null;
  for (const root of main.contentDOM.querySelectorAll<HTMLElement>(
    ".cm-grid",
  )) {
    if (tableOf(main, root)?.from === table.from) {
      return cellElement(root, cell.row, cell.column);
    }
  }
  return null;
}

/** Puts the hit mark where the hit is now, and takes it off elsewhere. */
function mark(main: EditorView): void {
  let hit = hits.get(main);
  // A hit the parse has not reached yet stays pending: its cell is marked
  // once the table is parsed and drawn.
  const pending = hit !== undefined && !parsedAt(main, hit);
  if (hit !== undefined && !pending && gridWritable(main.state)) {
    // On a writable page the mark rings the open cell holding the hit only.
    const open = openCellRange(main.state);
    if (!open || hit < open.span.from || hit > open.span.to) {
      hits.delete(main);
      hit = undefined;
    }
  }
  const target = hit === undefined || pending ? null : hitElement(main, hit);
  const before = marked.get(main);
  if (before && before !== target) before.classList.remove(HIT);
  if (target) {
    target.classList.add(HIT);
    marked.set(main, target);
  } else marked.delete(main);
}

const searchListener = EditorView.updateListener.of((update) => {
  const main = update.view;
  let search = false;
  let moved = false;
  let typed = false;
  for (const tr of update.transactions) {
    if (tr.annotation(fromCell)) {
      if (tr.docChanged) typed = true;
      continue;
    }
    if (!tr.selection) continue;
    if (tr.isUserEvent("select.search") || tr.isUserEvent("input.replace")) {
      search = true;
    } else if (tr.isUserEvent("select")) moved = true;
  }
  const hit = hits.get(main);
  if (hit !== undefined && update.docChanged) {
    hits.set(main, update.changes.mapPos(hit));
  }
  // Typing in the cell ends the hit's mark: the user is editing now.
  if (typed) hits.delete(main);
  if (search) onHit(main);
  else if (moved) onMove(main);
  else if (update.focusChanged && main.hasFocus) handBack(main);
  mark(main);
});

/** The cell whose content holds all of `[from, to]` in `state`, or null. */
function cellHolding(
  state: EditorState,
  from: number,
  to: number,
): GridCell | null {
  const table = gridTableAt(state, from);
  const cell = table ? cellAt(table, from) : null;
  if (!cell || from < cell.content.from || to > cell.content.to) return null;
  return cell;
}

/**
 * R20: a replace from the search panel that lands inside a cell keeps the
 * table's shape, as typing in the cell does (R8). Its line breaks become
 * spaces, and every pipe in the cell's new content is escaped by the parity
 * rule, with an even backslash run before the closing pipe. A replace
 * outside every cell, or on a read-only page, is left alone.
 */
const replaceInCells = EditorState.transactionFilter.of(
  (tr: Transaction): TransactionSpec | readonly TransactionSpec[] => {
    if (!tr.docChanged || !tr.isUserEvent("input.replace")) return tr;
    const start = tr.startState;
    if (!gridWritable(start)) return tr;
    const lb = start.lineBreak;
    const kept: { from: number; to: number; insert: string }[] = [];
    const cells: GridCell[] = [];
    let altered = false;
    tr.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
      const text = inserted.sliceString(0, inserted.length, "\n");
      const cell = cellHolding(start, fromA, toA);
      if (!cell) {
        kept.push({ from: fromA, to: toA, insert: text });
        return;
      }
      const flat = text.replace(/\r\n|\r|\n/g, " ");
      if (flat !== text) altered = true;
      kept.push({ from: fromA, to: toA, insert: flat });
      if (!cells.some((c) => c.span.from === cell.span.from)) cells.push(cell);
    });
    if (cells.length === 0) return tr;
    const plain = ChangeSet.of(
      kept.map((c) => ({ ...c, insert: start.toText(c.insert) })),
      start.doc.length,
      lb,
    );
    let changes = plain;
    for (const cell of cells) {
      changes = withEscapes(start.doc, cell.content, changes, lb);
      // R24: a replace that empties a bare edge cell, or opens a bare first
      // cell with a block marker, keeps the cell with its edge pipe.
      changes = withEdges(start.doc, cell.content, changes, lb).changes;
    }
    if (!altered && changes === plain) return tr;
    // The selection and the effects are in the coordinates of the changes
    // given: take them back to the start document, then through these.
    const back = tr.changes.invertedDesc;
    const selection = tr.selection
      ? tr.selection.map(back, 1).map(changes, 1)
      : undefined;
    const effects = StateEffect.mapEffects(
      StateEffect.mapEffects(tr.effects, back),
      changes,
    );
    const userEvent = tr.annotation(Transaction.userEvent);
    return [
      {
        changes,
        ...(selection ? { selection } : {}),
        effects,
        scrollIntoView: tr.scrollIntoView,
        ...(userEvent === undefined ? {} : { userEvent }),
      },
    ];
  },
);

const searchTheme = EditorView.baseTheme({
  ".cm-grid .cm-grid-hit": {
    outline: "2px solid var(--color-amber-700)",
    outlineOffset: "-2px",
  },
});

/** Search and replace inside the grid, on the main view. */
export const gridSearch: Extension = [
  searchListener,
  replaceInCells,
  searchTheme,
];
