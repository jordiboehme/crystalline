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

import { syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
import { StateEffect, StateField } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { YSyncConfig } from "y-codemirror.next";
import { ySyncFacet } from "y-codemirror.next";

import type { GridCell } from "./structure";
import { cellAt, gridTableAt } from "./structure";

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
  if (syntaxTree(state).length < state.doc.lineAt(head).to) return undefined;
  const table = gridTableAt(state, head);
  return table ? cellAt(table, head) : null;
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

export const openCellField = StateField.define<OpenCell | null>({
  create: () => null,
  update(value, tr) {
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
      if (!tr.docChanged) return value;
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
const listenersOn = new WeakMap<EditorView, Set<(r: CloseReason) => void>>();

/** The cell editor mounted on `main`, if any. */
export function mountedCell(main: EditorView): MountedCell | null {
  return mountedOn.get(main) ?? null;
}

/** Registers `cell` as `main`'s one cell editor, tearing down the last one. */
export function registerCell(main: EditorView, cell: MountedCell): void {
  const previous = mountedOn.get(main);
  mountedOn.set(main, cell);
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
 * Closes `main`'s open cell: destroys the cell view, clears the published
 * cursor in a room, sets `openCellField` to null, notifies the listeners and
 * moves focus. A no-op when nothing is open. Inside an update of `main` the
 * dispatch, the notifying and the focus move wait for a microtask; on the
 * main view's destroy there is no dispatch and no focus move at all.
 * `"grid"` focuses the nearest ancestor of the cell's element with a
 * `tabindex`, or the main view when there is none.
 */
export function closeCell(
  main: EditorView,
  reason: CloseReason,
  focusTo: "main" | "grid" | "none",
): void {
  const mounted = mountedOn.get(main) ?? null;
  const open = main.state.field(openCellField, false) ?? null;
  if (!mounted && !open) return;
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
    if (isDestroyed(main)) return;
    if (main.state.field(openCellField, false)) {
      main.dispatch({ effects: setOpenCell.of(null) });
    }
    notify(main, reason);
    if (focusTo === "main") main.focus();
    else if (focusTo === "grid") {
      const grid = parent?.parentElement?.closest<HTMLElement>("[tabindex]");
      if (grid?.isConnected) grid.focus();
      else main.focus();
    }
  };
  if (inUpdate(main)) queueMicrotask(finish);
  else finish();
}

function notify(main: EditorView, reason: CloseReason): void {
  for (const listener of [...(listenersOn.get(main) ?? [])]) listener(reason);
}
