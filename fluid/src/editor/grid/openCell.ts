/**
 * The open-cell intent of a main view, and the one path that closes a cell.
 *
 * `openCellField` holds where the caret of the open cell sits, in main-doc
 * positions. Which cell that is, is never stored: `openCellRange` reads it
 * from the parsed table on every call, so a change elsewhere in the table (a
 * peer deleting the row above, an escaped pipe typed into the cell) moves the
 * cell without anyone having to track it. The field drops itself to `null`
 * when the parsed table has no cell at the mapped caret any more - its pipes
 * or its row were removed. That is the only "gone" test.
 *
 * `closeCell` is the one teardown: the gone check, a user leaving the cell,
 * Raw mode taking the grid away, a buffer swap and the main view's destroy
 * all go through it, so every one of them notifies and gives focus back the
 * same way.
 */

import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import type {
  ChangeDesc,
  EditorState,
  TransactionSpec,
} from "@codemirror/state";
import { Facet, MapMode, StateEffect, StateField } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { YSyncConfig } from "y-codemirror.next";
import { ySyncFacet } from "y-codemirror.next";

import { parseAdvanced } from "../parseProgress";
import { drawnGridTables } from "./draws";
import type { GridCell } from "./structure";
import { cellAt, gridTableAt } from "./structure";

/**
 * The main view a cell view edits a cell of. It lives here, below the cell
 * editor, so the cell view's presence layer can read it without importing
 * the cell editor back.
 */
export const cellMain = Facet.define<EditorView, EditorView | null>({
  combine: (values) => values[0] ?? null,
});

/** Main views running a search command sent from a focused cell. */
const searchingFromCell = new WeakSet<EditorView>();

/**
 * Runs `command` on `main` marked as a search from a focused cell. The hit
 * may scroll that cell's table out of the viewport in the same update, which
 * takes the focus with it, so the search listener asks this instead of the
 * focus to know where the focus belongs.
 */
export function searchFromCell<T>(main: EditorView, command: () => T): T {
  searchingFromCell.add(main);
  try {
    return command();
  } finally {
    searchingFromCell.delete(main);
  }
}

/** Whether `main` is running a search command sent from a focused cell. */
export function isSearchFromCell(main: EditorView): boolean {
  return searchingFromCell.has(main);
}

/** Main-doc positions inside one cell's content. */
export interface OpenCell {
  anchor: number;
  head: number;
  focused: boolean;
}

export type CloseReason = "gone" | "user" | "raw" | "swap" | "destroy";

/** Opens a cell (the one holding `head`), moves its caret, or closes it. */
export const setOpenCell = StateEffect.define<OpenCell | null>();

/**
 * The cell holding `head` in the parsed tables: a cell, `null` when there is
 * none, or `undefined` when the parse has not reached `head`'s line yet and
 * the answer is not known.
 */
function cellAtHead(
  state: EditorState,
  head: number,
): GridCell | null | undefined {
  if (head < 0 || head > state.doc.length) return null;
  // The parse must reach the end of the table's block, not only the head's
  // line: a tree that stops inside the table holds no whole table (D54).
  let line = state.doc.lineAt(head);
  while (
    line.number < state.doc.lines &&
    state.doc.line(line.number + 1).text.trim() !== ""
  ) {
    line = state.doc.line(line.number + 1);
  }
  // The state's own tree, as the grid reads it in the same update. Short
  // of the table, the cell is read from the tables the grid draws (D58),
  // never from a parse step of its own: that could read text the grid
  // still draws as a table as code, and close a cell the grid keeps (R4-1).
  const tree = syntaxTree(state);
  if (tree.length < line.to) return drawnCellAt(state, head);
  const table = gridTableAt(state, head, tree);
  return table ? cellAt(table, head) : null;
}

/**
 * The cell holding `head` in a table the grid still draws from its own
 * lines while the parse has not reached it (D58). That table is read again
 * from the current text on every change, so a head in it with no cell (a
 * peer emptied a last cell without a pipe after it, R10) is gone, as the
 * tree will say. `undefined` when no drawn table holds `head`: not known.
 */
function drawnCellAt(
  state: EditorState,
  head: number,
): GridCell | null | undefined {
  for (const read of state.facet(drawnGridTables)) {
    const table = read(state).find((t) => t.from <= head && head <= t.to);
    if (table) return cellAt(table, head);
  }
  return undefined;
}

function clampTo(pos: number, cell: GridCell): number {
  return Math.min(cell.content.to, Math.max(cell.content.from, pos));
}

/** `value` checked against the table: kept inside its cell, or null. */
function settle(state: EditorState, value: OpenCell): OpenCell | null {
  const cell = cellAtHead(state, value.head);
  if (cell === undefined) return value; // not parsed yet: keep, never guess
  if (cell === null) return null;
  const anchor = clampTo(value.anchor, cell);
  const head = clampTo(value.head, cell);
  if (anchor === value.anchor && head === value.head) return value;
  return { ...value, anchor, head };
}

/**
 * R7: whether `changes` deleted the cell's closing delimiter. With a closing
 * pipe that is the pipe. A last cell without one is gone when one deletion
 * takes the line break on either side of its row together with its content,
 * measured to the line's end (trailing spaces included). Clearing the cell's
 * content never counts.
 */
function closingDeleted(
  state: EditorState,
  cell: GridCell,
  changes: ChangeDesc,
): boolean {
  const end = cell.span.to;
  const doc = state.doc;
  if (doc.sliceString(end, end + 1) === "|") {
    return changes.mapPos(end, 1, MapMode.TrackAfter) === null;
  }
  const line = doc.lineAt(end);
  const before = line.from > 0 ? line.from - 1 : null;
  const after = line.to < doc.length ? line.to + 1 : null;
  let gone = false;
  changes.iterChangedRanges((fromA, toA) => {
    if (fromA === toA) return;
    if (before !== null && fromA <= before && toA >= line.to) gone = true;
    if (after !== null && fromA <= cell.content.from && toA >= after)
      gone = true;
  });
  return gone;
}

/**
 * Whether the grid may change `state`'s document: the view is editable and
 * the state not read-only (an ended Reflection is neither). `dispatch` does
 * not enforce either, so every grid path that opens a cell or edits asks.
 */
export function gridWritable(state: EditorState): boolean {
  return state.facet(EditorView.editable) && !state.readOnly;
}

/**
 * The wall clock a landing gives the parse to reach its table. A watchdog,
 * not a tolerance: an incremental reparse of a changed table takes well
 * under a millisecond.
 */
const LANDING_PARSE_MS = 1000;

/**
 * Makes `main`'s syntax tree reach `pos` and the line after it (the line
 * that ends a table), and publishes it with an empty transaction, as the
 * parse worker does. A document change reparses on a budget of 20 ms of
 * `Date.now()`, so under load the tree can stop short of the table a gesture
 * just changed; a landing that read it then found no cell to open. Call it
 * after the gesture's dispatch and before the landing reads the table, never
 * inside an update.
 */
export function parseThrough(main: EditorView, pos: number): void {
  const doc = main.state.doc;
  const line = doc.lineAt(Math.max(0, Math.min(pos, doc.length)));
  const upto =
    line.number < doc.lines ? doc.line(line.number + 1).to : doc.length;
  if (syntaxTree(main.state).length >= upto) return;
  ensureSyntaxTree(main.state, upto, LANDING_PARSE_MS);
  main.dispatch({});
}

export const openCellField = StateField.define<OpenCell | null>({
  create: () => null,
  update(value, tr) {
    // A read-only buffer opens no cell, and one turning read-only closes it.
    if (!gridWritable(tr.state)) return null;
    let next = value;
    let set = false;
    for (const effect of tr.effects) {
      if (effect.is(setOpenCell)) {
        next = effect.value;
        set = true;
      }
    }
    if (next === null) return null;
    if (!set) {
      // A parse that moved on can read the cell's table as gone (R4-1).
      if (!tr.docChanged)
        return parseAdvanced(tr) ? settle(tr.state, next) : value;
      const before = cellAtHead(tr.startState, next.head);
      if (before && closingDeleted(tr.startState, before, tr.changes)) {
        return null;
      }
      next = {
        anchor: tr.changes.mapPos(next.anchor),
        head: tr.changes.mapPos(next.head),
        focused: next.focused,
      };
    }
    return settle(tr.state, next);
  },
});

/** The open cell, re-derived from the table structure on every read. */
export function openCellRange(state: EditorState): GridCell | null {
  const open = state.field(openCellField, false);
  if (!open) return null;
  return cellAtHead(state, open.head) ?? null;
}

/** What `cellEditor.ts` registers for the cell editor mounted on a view. */
export interface MountedCell {
  view: EditorView;
  /** The element the cell view sits in. */
  parent: HTMLElement;
  /** Destroys the cell view; no notifying, no focus. Safe to call twice. */
  teardown(): void;
}

const mountedOn = new WeakMap<EditorView, MountedCell>();
/**
 * Bumped on every mount and every outside `setOpenCell` that opens a cell, so
 * a close that waits for a microtask can tell whether a newer cell took over
 * in the meantime.
 */
const generations = new WeakMap<EditorView, number>();
/** The generation a close still waiting for its microtask belongs to. */
const pendingOn = new WeakMap<EditorView, number>();

function generation(main: EditorView): number {
  return generations.get(main) ?? 0;
}

/** Marks that a newer cell was opened on `main`. */
export function bumpGeneration(main: EditorView): void {
  generations.set(main, generation(main) + 1);
}
const listenersOn = new WeakMap<EditorView, Set<(r: CloseReason) => void>>();

/** The cell editor mounted on `main`, if any. */
export function mountedCell(main: EditorView): MountedCell | null {
  return mountedOn.get(main) ?? null;
}

/** Registers `cell` as `main`'s one cell editor, tearing down the last one. */
export function registerCell(main: EditorView, cell: MountedCell): void {
  const previous = mountedOn.get(main);
  mountedOn.set(main, cell);
  bumpGeneration(main);
  if (previous && previous !== cell) previous.teardown();
}

/** Forgets `cell` if it is still `main`'s cell editor. */
export function unregisterCell(main: EditorView, cell: MountedCell): void {
  if (mountedOn.get(main) === cell) mountedOn.delete(main);
}

/**
 * Whether `view` is inside an update, a `setState` or a reconfigure, where a
 * dispatch into it throws. CodeMirror offers no public test; its private
 * `updateState` is 0 exactly when the view is idle. A view without the field
 * reads as idle.
 */
export function inUpdate(view: EditorView): boolean {
  const state = (view as unknown as { updateState?: unknown }).updateState;
  return typeof state === "number" && state !== 0;
}

/** `view.destroyed`, which CodeMirror keeps private in its types. */
export function isDestroyed(view: EditorView): boolean {
  return (view as unknown as { destroyed?: unknown }).destroyed === true;
}

/** Clears the local awareness `cursor` when the state is in a room. */
function clearCursor(state: EditorState): void {
  const sync = state.facet(ySyncFacet) as YSyncConfig | undefined;
  const awareness = sync?.awareness as
    | {
        getLocalState(): Record<string, unknown> | null;
        setLocalStateField(field: string, value: unknown): void;
      }
    | undefined;
  if (awareness?.getLocalState()?.cursor != null) {
    awareness.setLocalStateField("cursor", null);
  }
}

/** Tells `listener` every time a cell of `main` closes, with why. */
export function onCellClosed(
  main: EditorView,
  listener: (reason: CloseReason) => void,
): () => void {
  let set = listenersOn.get(main);
  if (!set) {
    set = new Set();
    listenersOn.set(main, set);
  }
  set.add(listener);
  return () => {
    listenersOn.get(main)?.delete(listener);
  };
}

/**
 * Where main's caret goes when a cell closes with focus to main (R6): out of
 * the table, to the start of the line after it (the table's end when it is
 * the last line), or the position itself when no table is there any more. In
 * Raw the table is text, so the caret stays at the cell's head.
 */
function caretAfterClose(
  state: EditorState,
  reason: CloseReason,
  head: number,
): number {
  const pos = Math.max(0, Math.min(head, state.doc.length));
  if (reason === "raw") return pos;
  const table = gridTableAt(state, pos);
  if (!table) return pos;
  const last = state.doc.lineAt(table.to);
  return last.number < state.doc.lines
    ? state.doc.line(last.number + 1).from
    : table.to;
}

/**
 * Closes `main`'s open cell: destroys the cell view, clears the published
 * cursor in a room, sets `openCellField` to null, notifies the listeners and
 * moves focus. With focus to main, main's caret first leaves the table (see
 * `caretAfterClose`), so main never publishes a caret inside one; a swap
 * keeps the new state's own selection. A no-op
 * when nothing is open, and while a close of the same cell still waits for
 * its microtask. Inside an update of `main` the dispatch, the notifying and
 * the focus move wait for a microtask; if a newer cell was opened by then,
 * the close only notifies and leaves the field and the focus to the new one.
 * On the main view's destroy there is no dispatch and no focus move at all.
 * `"grid"` focuses the nearest ancestor of the cell's element with a
 * `tabindex`, or the main view when there is none.
 */
export function closeCell(
  main: EditorView,
  reason: CloseReason,
  focusTo: "main" | "grid" | "none",
): void {
  const open = main.state.field(openCellField, false) ?? null;
  closeCellAt(main, reason, focusTo, open?.head ?? null);
}

/**
 * `closeCell` with the caret position known to the caller: the gone check
 * passes the mapped head of a cell the field already dropped, and Raw the
 * head from before the reconfigure.
 */
export function closeCellAt(
  main: EditorView,
  reason: CloseReason,
  focusTo: "main" | "grid" | "none",
  head: number | null,
): void {
  const mounted = mountedOn.get(main) ?? null;
  const open = main.state.field(openCellField, false) ?? null;
  const gen = generation(main);
  if (!mounted && (!open || pendingOn.get(main) === gen)) return;
  const parent = mounted?.parent ?? null;
  if (mounted) {
    mountedOn.delete(main);
    mounted.teardown();
  }
  clearCursor(main.state);
  if (reason === "destroy") {
    notify(main, reason);
    return;
  }
  const finish = () => {
    if (pendingOn.get(main) === gen) pendingOn.delete(main);
    if (isDestroyed(main)) return;
    const current = generation(main) === gen;
    if (current) {
      const spec: TransactionSpec = {
        ...(main.state.field(openCellField, false)
          ? { effects: setOpenCell.of(null) }
          : {}),
        // A swap leaves the selection to the new state: the head belongs
        // to the old document (R9).
        ...(focusTo === "main" && head !== null && reason !== "swap"
          ? { selection: { anchor: caretAfterClose(main.state, reason, head) } }
          : {}),
      };
      if (spec.effects !== undefined || spec.selection !== undefined) {
        main.dispatch(spec);
      }
    }
    notify(main, reason);
    if (!current) return; // the newer cell owns the focus
    if (focusTo === "main") main.focus();
    else if (focusTo === "grid") {
      const grid = parent?.parentElement?.closest<HTMLElement>("[tabindex]");
      if (grid?.isConnected) grid.focus();
      else main.focus();
    }
  };
  if (inUpdate(main)) {
    pendingOn.set(main, gen);
    queueMicrotask(finish);
  } else finish();
}

function notify(main: EditorView, reason: CloseReason): void {
  for (const listener of [...(listenersOn.get(main) ?? [])]) listener(reason);
}
