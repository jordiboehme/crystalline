/**
 * The table grid's keyboard: inside an open cell, in grid navigation (focus
 * on a `th`/`td`, one roving tab stop) and on the way in from the main view.
 *
 * - **In a cell** (`cellKeymap`, `Prec.high` in the cell view): Tab and
 *   Shift+Tab walk the cells row by row, Enter goes down a column, the arrows
 *   cross a cell's visible edge into the next cell, Up from the header and
 *   Down from the last row leave to the main line beside the table, Escape
 *   steps out to grid navigation. Tab in the last cell and Enter in the last
 *   row add a row first; that row is one undo step, and its undo opens the
 *   cell the key was pressed in again.
 * - **Grid navigation** (`gridNavKeys`, a listener on the table element):
 *   arrows, Home and End, Mod-Home and Mod-End move the focus; Enter or F2
 *   open the cell, a printable character opens it and replaces its content
 *   (as a spreadsheet does), Delete or Backspace clear it, Mod-z and
 *   Mod-Shift-z (or Mod-y) undo and redo, Escape leaves the table, Tab leaves
 *   the grid the browser's way.
 * - **From the main view** (`mainEntryKeys`): ArrowDown on the line above a
 *   drawn table and ArrowRight at that line's end open the first header
 *   cell; ArrowUp on the line below and ArrowLeft at its start open the last
 *   row's first cell.
 *
 * Every move goes through `setOpenCell` and `hostOpenCell`, the one way the
 * cell editor moves.
 */

import { isolateHistory } from "@codemirror/commands";
import type { Extension } from "@codemirror/state";
import { Prec } from "@codemirror/state";
import type { Command, EditorView } from "@codemirror/view";
import { keymap } from "@codemirror/view";

import { roomUndo } from "../roomUndo";
import { tableAddRowBelow } from "../tableVerbs";
import type { CellEditor } from "./cellEditor";
import {
  atCellEdge,
  cellMain,
  noteUndoLanding,
  runMainUndo,
} from "./cellEditor";
import {
  cellElement,
  hostOpenCell,
  placeOf,
  rove,
  tableOf,
} from "./gridWidget";
import {
  closeCell,
  gridWritable,
  mountedCell,
  openCellField,
  openCellRange,
  setOpenCell,
} from "./openCell";
import type { GridCell, GridTable } from "./structure";
import { gridTableAt, gridTables } from "./structure";

type Side = "start" | "end";

/**
 * Opens `cell` with the caret at its content's start or end, or with the
 * whole content selected, mounts the cell editor there and focuses it.
 */
function openAt(
  main: EditorView,
  cell: GridCell,
  side: Side | "all",
): CellEditor | null {
  const { from, to } = cell.content;
  const anchor = side === "end" ? to : from;
  const head = side === "start" ? from : to;
  main.dispatch({
    effects: setOpenCell.of({ anchor, head, focused: true }),
  });
  return hostOpenCell(main, "now");
}

/** The open cell and its table, read from the current state. */
function current(
  main: EditorView,
): { table: GridTable; cell: GridCell } | null {
  const open = main.state.field(openCellField, false);
  const cell = openCellRange(main.state);
  if (!open || !cell) return null;
  const table = gridTableAt(main.state, open.head);
  return table ? { table, cell } : null;
}

function cellIn(
  table: GridTable,
  row: number,
  column: number,
): GridCell | null {
  return table.rows[row]?.[column] ?? null;
}

/** The cell before (-1) or after (1) `cell`, row by row, or null. */
function neighbour(
  table: GridTable,
  cell: GridCell,
  dir: -1 | 1,
): GridCell | null {
  const flat = table.rows.flat();
  const index = flat.findIndex(
    (c) => c.row === cell.row && c.column === cell.column,
  );
  return flat[index + dir] ?? null;
}

/**
 * Closes the undo group on both stacks: CodeMirror's history solo, the
 * room's `Y.UndoManager` capture window in a room.
 */
export function closeUndoGroup(
  main: EditorView,
  side: "before" | "after",
): void {
  main.state.facet(roomUndo)?.stopCapturing();
  main.dispatch({ annotations: isolateHistory.of(side) });
}

/**
 * Adds a row below the last one and opens its cell in `column`, as one undo
 * step: the undo group is closed on both sides of the row, and the undo that
 * takes it away reopens the cell the key was pressed in. The main view never
 * takes the focus (R15): its caret sits in the new row, and a focused main
 * view would publish it to the room.
 */
function addRowAndOpen(
  main: EditorView,
  table: GridTable,
  from: GridCell,
  column: number,
): boolean {
  const open = main.state.field(openCellField, false);
  if (!open) return false;
  const before = main.state.doc;
  closeUndoGroup(main, "before");
  if (!tableAddRowBelow(main, from.content.from, { focus: false })) return true;
  closeUndoGroup(main, "after");
  const after = main.state.doc;
  const left = { anchor: open.anchor, head: open.head, focused: true };
  noteUndoLanding(main, {
    before,
    after,
    restore: () => {
      main.dispatch({ effects: setOpenCell.of(left) });
      hostOpenCell(main, "now");
    },
  });
  const grown = gridTableAt(main.state, table.from);
  const row = grown?.rows.at(-1);
  const target = row?.[column] ?? row?.[0];
  if (target) openAt(main, target, "end");
  return true;
}

/**
 * Closes the cell and puts main's caret beside the table: the end of the
 * line above (`dir` -1) or the start of the line below (1). With no such
 * line the cell stays open.
 */
function leave(main: EditorView, table: GridTable, dir: -1 | 1): boolean {
  const doc = main.state.doc;
  const first = doc.lineAt(table.from);
  const last = doc.lineAt(table.to);
  if (dir < 0 && first.number === 1) return true;
  if (dir > 0 && last.number === doc.lines) return true;
  const pos =
    dir < 0 ? doc.line(first.number - 1).to : doc.line(last.number + 1).from;
  closeCell(main, "user", "none");
  main.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
  main.focus();
  return true;
}

/** Closes the cell and gives the focus to its element: grid navigation. */
function toGrid(main: EditorView): boolean {
  const el = mountedCell(main)?.parent ?? null;
  closeCell(main, "user", "none");
  if (el?.isConnected) {
    rove(el);
    el.focus();
  } else {
    // The cell's element is gone, so main is the one place left: a narrow
    // exception to R15, and the cell is already closed.
    main.focus();
  }
  return true;
}

/** A cell-view command that runs on its main view and the open cell. */
function onCell(
  run: (
    main: EditorView,
    view: EditorView,
    at: { table: GridTable; cell: GridCell },
  ) => boolean,
): Command {
  return (view) => {
    const main = view.state.facet(cellMain);
    if (!main || !gridWritable(main.state)) return false;
    const at = current(main);
    return at ? run(main, view, at) : false;
  };
}

/** Vertical moves: inside a cell of several lines first, then row to row. */
function vertical(dir: -1 | 1): Command {
  return onCell((main, view, { table, cell }) => {
    if (insideVertically(main, view, dir)) return false;
    const target = cellIn(table, cell.row + dir, cell.column);
    if (target) {
      openAt(main, target, "end");
      return true;
    }
    return leave(main, table, dir);
  });
}

/**
 * Whether ArrowUp or ArrowDown should stay in the cell: its content wraps
 * or holds a `<br>` and the caret is not on its first or last visual line.
 */
function insideVertically(
  main: EditorView,
  view: EditorView,
  dir: -1 | 1,
): boolean {
  const cell = openCellRange(main.state);
  if (!cell) return false;
  const range = view.state.selection.main;
  const next = view.moveVertically(range, dir > 0);
  return (
    next.head !== range.head &&
    next.head >= cell.content.from &&
    next.head <= cell.content.to
  );
}

/**
 * The cell view's navigation keys. `Prec.high`, so they come before the
 * cell's own Enter and the default keymap.
 */
export const cellKeymap: Extension = Prec.high(
  keymap.of([
    {
      key: "Tab",
      run: onCell((main, _view, { table, cell }) => {
        const next = neighbour(table, cell, 1);
        if (next) openAt(main, next, "end");
        else addRowAndOpen(main, table, cell, 0);
        return true;
      }),
    },
    {
      key: "Shift-Tab",
      run: onCell((main, _view, { table, cell }) => {
        const previous = neighbour(table, cell, -1);
        if (previous) openAt(main, previous, "end");
        return true;
      }),
    },
    {
      key: "Enter",
      run: onCell((main, _view, { table, cell }) => {
        const below = cellIn(table, cell.row + 1, cell.column);
        if (below) openAt(main, below, "end");
        else addRowAndOpen(main, table, cell, cell.column);
        return true;
      }),
    },
    {
      key: "ArrowLeft",
      run: onCell((main, view, { table, cell }) => {
        if (!atCellEdge(view, -1)) return false;
        const previous = neighbour(table, cell, -1);
        if (previous) openAt(main, previous, "end");
        return true;
      }),
    },
    {
      key: "ArrowRight",
      run: onCell((main, view, { table, cell }) => {
        if (!atCellEdge(view, 1)) return false;
        const next = neighbour(table, cell, 1);
        if (next) openAt(main, next, "start");
        return true;
      }),
    },
    { key: "ArrowUp", run: vertical(-1) },
    { key: "ArrowDown", run: vertical(1) },
    {
      key: "Escape",
      run: (view) => {
        const main = view.state.facet(cellMain);
        return main ? toGrid(main) : false;
      },
    },
  ]),
);

const mac =
  typeof navigator !== "undefined" &&
  /Mac|iPhone|iPad|iPod/.test(navigator.platform);

/** A key that types one character: no Ctrl, Cmd or Alt-Ctrl chord. */
function printable(event: KeyboardEvent): boolean {
  if (event.metaKey) return false;
  // AltGr on Windows and Linux reports Ctrl and Alt together: on a German
  // layout that is how `|`, `@`, `\`, `[`, `{` and `~` are typed.
  if (event.ctrlKey && !event.altKey) return false;
  return [...event.key].length === 1;
}

/**
 * Moves the grid's focus to the cell element at `row` and `column`, clamped
 * to the table.
 */
function focusAt(
  root: HTMLElement,
  table: GridTable,
  row: number,
  column: number,
): void {
  const r = Math.max(0, Math.min(table.rows.length - 1, row));
  const width = table.rows[r]?.length ?? 1;
  const c = Math.max(0, Math.min(width - 1, column));
  const el = cellElement(root, r, c);
  if (!el) return;
  rove(el);
  el.focus();
}

/** Main's caret on the line after the table (its end when it is the last). */
function leaveGrid(main: EditorView, table: GridTable): void {
  const doc = main.state.doc;
  const last = doc.lineAt(table.to);
  const pos =
    last.number < doc.lines ? doc.line(last.number + 1).from : table.to;
  main.dispatch({ selection: { anchor: pos }, scrollIntoView: true });
  main.focus();
}

/** One grid-navigation key on the focused cell element `el`. */
function navigate(
  main: EditorView,
  root: HTMLElement,
  el: HTMLElement,
  event: KeyboardEvent,
): boolean {
  const table = tableOf(main, root);
  if (!table) return false;
  const [row, column] = placeOf(el);
  const cell = cellIn(table, row, column);
  if (!cell) return false;
  const mod = mac ? event.metaKey : event.ctrlKey;
  const last = table.rows.length - 1;
  // The main view never sees a key inside the grid (the widget ignores
  // events), so its undo keys are bound here too.
  // Reading is allowed on a read-only buffer (moving between cells, leaving
  // the table); nothing below that changes the document or opens a cell is.
  const writable = gridWritable(main.state);
  const letter = event.key.toLowerCase();
  if (mod && !event.altKey && (letter === "z" || letter === "y")) {
    if (!writable) return false;
    runMainUndo(main, letter === "z" && !event.shiftKey ? "undo" : "redo");
    return true;
  }
  switch (event.key) {
    case "ArrowLeft":
      focusAt(root, table, row, column - 1);
      return true;
    case "ArrowRight":
      focusAt(root, table, row, column + 1);
      return true;
    case "ArrowUp":
      focusAt(root, table, row - 1, column);
      return true;
    case "ArrowDown":
      focusAt(root, table, row + 1, column);
      return true;
    case "Home":
      if (mod) focusAt(root, table, 0, 0);
      else focusAt(root, table, row, 0);
      return true;
    case "End":
      if (mod) focusAt(root, table, last, Infinity);
      else focusAt(root, table, row, Infinity);
      return true;
    case "Enter":
    case "F2":
      if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey)
        return false;
      if (!writable) return false;
      openAt(main, cell, "end");
      return true;
    case "Delete":
    case "Backspace": {
      if (mod || event.altKey || !writable) return false;
      const { from, to } = cell.content;
      if (from < to) {
        // R23: an edge cell without its edge pipe exists only while it has
        // text (` | 2` is a leading pipe and one cell, `1 | ` a trailing
        // pipe), so clearing it leaves the one pipe it needs, as a paste of
        // an empty value does.
        const line = table.model.lines[row === 0 ? 0 : row + 1];
        const bare =
          line !== undefined &&
          ((column === 0 && !line.leadingPipe) ||
            (column === line.cells.length - 1 && !line.trailingPipe));
        // Its own undo step, never joined to typing just before or after.
        closeUndoGroup(main, "before");
        main.dispatch({
          changes: bare ? { from, to, insert: "|" } : { from, to },
          userEvent:
            event.key === "Delete" ? "delete.forward" : "delete.backward",
        });
        closeUndoGroup(main, "after");
      }
      return true;
    }
    case "Escape":
      leaveGrid(main, table);
      return true;
    default:
      break;
  }
  if (!printable(event) || !writable) return false;
  // The character goes through the cell view, so its filter escapes a `|`
  // and keeps a `\` before the closing pipe even.
  const editor = openAt(main, cell, "all");
  if (!editor) return true;
  // The replacement starts its own undo step; what is typed after it joins.
  closeUndoGroup(main, "before");
  const { from, to } = editor.view.state.selection.main;
  editor.view.dispatch({
    changes: { from, to, insert: event.key },
    selection: { anchor: from + event.key.length },
    userEvent: "input.type",
  });
  return true;
}

/**
 * Grid navigation on `table`, a grid's table element: keys pressed while a
 * `th` or `td` itself has the focus. Keys from the hosted cell view bubble
 * through here too and are left alone.
 */
export function gridNavKeys(main: EditorView, table: HTMLElement): void {
  table.addEventListener("keydown", (event) => {
    if (event.defaultPrevented || event.isComposing) return;
    const el = event.target;
    if (!(el instanceof HTMLElement) || !table.contains(el)) return;
    if (!el.matches("[aria-rowindex][aria-colindex]")) return;
    if (mountedCell(main)?.parent === el) return;
    const root = table.closest<HTMLElement>(".cm-grid");
    if (!root) return;
    if (navigate(main, root, el, event)) event.preventDefault();
  });
}

/**
 * The drawn table whose first line comes right after line `number` (`dir`
 * 1), or whose last line comes right before it (-1).
 */
function tableBeside(
  view: EditorView,
  number: number,
  dir: -1 | 1,
): GridTable | null {
  const doc = view.state.doc;
  return (
    gridTables(view.state).find((table) =>
      dir > 0
        ? doc.lineAt(table.from).number === number + 1
        : doc.lineAt(table.to).number === number - 1,
    ) ?? null
  );
}

/** An entry key: `dir` 1 enters from the line above, -1 from below. */
function enter(dir: -1 | 1, atEdge: boolean): Command {
  return (view) => {
    const { selection } = view.state;
    if (!gridWritable(view.state)) return false;
    if (selection.ranges.length > 1 || !selection.main.empty) return false;
    const head = selection.main.head;
    const line = view.state.doc.lineAt(head);
    if (atEdge && head !== (dir > 0 ? line.to : line.from)) return false;
    const table = tableBeside(view, line.number, dir);
    if (!table) return false;
    const cell =
      dir > 0 ? cellIn(table, 0, 0) : cellIn(table, table.rows.length - 1, 0);
    if (!cell) return false;
    openAt(view, cell, dir > 0 ? "start" : "end");
    return true;
  };
}

/**
 * The main view's way into a drawn table. `Prec.high`, so it comes before
 * the default keymap's arrows.
 */
export const mainEntryKeys: Extension = Prec.high(
  keymap.of([
    { key: "ArrowDown", run: enter(1, false) },
    { key: "ArrowRight", run: enter(1, true) },
    { key: "ArrowUp", run: enter(-1, false) },
    { key: "ArrowLeft", run: enter(-1, true) },
  ]),
);
