/**
 * What the floating action bar stands by: a text selection outside any
 * table, the caret or a selection in the open table cell, or nothing.
 *
 * Eager and small on purpose. It imports nothing from `grid/`, which is the
 * lazy chunk: the cell editor reaches in instead, through `floatingCell(main)`,
 * which the grid adds to every cell view it mounts. Per main view one hub
 * combines the two sources: while a cell is reported, main's own selection is
 * stale (the cell sends only its caret) and is not looked at; when the cell
 * goes, main's state is read again rather than `null` being assumed.
 *
 * The kind is decided from the state alone, never from layout. The rectangle
 * is read in a measure pass (`requestMeasure`), never inside an update, and
 * falls back to an empty one where there is no layout (jsdom). A page that is
 * not writable (R17) has no context at all, so the bar never shows on it.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension } from "@codemirror/state";
import { Prec } from "@codemirror/state";
import type { ViewUpdate } from "@codemirror/view";
import { EditorView, ViewPlugin, keymap } from "@codemirror/view";

export type FloatingContext =
  /** A non-empty main selection outside tables. */
  | { kind: "text"; rect: DOMRect }
  /**
   * The caret, or a selection, in the open cell. `rect` is the table's box,
   * so the bar stands above or below the whole table and never over a row
   * (finding 3); for a table taller than the viewport it is the caret's.
   * `caret` is the caret's box, where the bar goes when the table leaves no
   * room on either side.
   */
  | { kind: "cell"; rect: DOMRect; caret: DOMRect; selection: boolean }
  | null;

interface Hub {
  main: EditorView;
  onChange: (context: FloatingContext) => void;
  /** The mounted cell view reporting to this hub, if any. */
  cell: EditorView | null;
  /** What was last reported; undefined before the first report. */
  last: FloatingContext | undefined;
  /** The key of this hub's measure requests. */
  key: object;
  /** How often the person moved a selection here (main or cell). */
  moves: number;
}

const hubs = new WeakMap<EditorView, Hub>();

/** What the bar registers to be focused by Alt+F10, per main view. */
const focusers = new WeakMap<EditorView, (origin: EditorView) => boolean>();

/**
 * Registers what Alt+F10 calls on `main` (with the view the key was pressed
 * in), or removes it. Returns the unregister.
 */
export function setFloatingFocus(
  main: EditorView,
  focus: (origin: EditorView) => boolean,
): () => void {
  focusers.set(main, focus);
  return () => {
    if (focusers.get(main) === focus) focusers.delete(main);
  };
}

function focusBar(main: EditorView, origin: EditorView): boolean {
  return focusers.get(main)?.(origin) ?? false;
}

/** The R17 test, spelled here so this file never imports the grid. */
function writable(state: EditorState): boolean {
  return state.facet(EditorView.editable) && !state.readOnly;
}

function inTable(state: EditorState, pos: number): boolean {
  const tree = syntaxTree(state);
  for (const side of [-1, 1] as const) {
    for (
      let node: ReturnType<typeof tree.resolveInner> | null = tree.resolveInner(
        pos,
        side,
      );
      node;
      node = node.parent
    ) {
      if (node.name === "Table") return true;
    }
  }
  return false;
}

/**
 * Whether `from`..`to` touches a table: an end inside one, or a table lying
 * between the ends. The sticky bar's `toggleInline` would wrap such a span
 * as it stands (it does not refuse it), and a mark across a table's rows
 * breaks them, so the floating bar offers nothing there.
 */
function touchesTable(state: EditorState, from: number, to: number): boolean {
  if (inTable(state, from) || inTable(state, to)) return true;
  let found = false;
  syntaxTree(state).iterate({
    from,
    to,
    enter: (node) => {
      if (found) return false;
      if (node.name === "Table" && node.from < to && node.to > from) {
        found = true;
        return false;
      }
      return true;
    },
  });
  return found;
}

const EMPTY = { left: 0, top: 0, right: 0, bottom: 0 };

/** The box around `from`..`to` in `view`, or null where nothing is laid out. */
function rangeBox(
  view: EditorView,
  from: number,
  to: number,
): { left: number; top: number; right: number; bottom: number } | null {
  let start: ReturnType<EditorView["coordsAtPos"]>;
  let end: ReturnType<EditorView["coordsAtPos"]>;
  try {
    start = view.coordsAtPos(from, 1);
    end = from === to ? start : view.coordsAtPos(to, -1);
  } catch {
    return null; // no layout to ask (jsdom)
  }
  if (!start || !end) return null;
  const oneLine = Math.abs(start.top - end.top) < 1;
  return {
    left: oneLine ? Math.min(start.left, end.left) : start.left,
    top: Math.min(start.top, end.top),
    right: oneLine ? Math.max(start.right, end.right) : start.right,
    bottom: Math.max(start.bottom, end.bottom),
  };
}

function toRect(box: {
  left: number;
  top: number;
  right: number;
  bottom: number;
}): DOMRect {
  return new DOMRect(
    box.left,
    box.top,
    Math.max(0, box.right - box.left),
    Math.max(0, box.bottom - box.top),
  );
}

function isDestroyed(view: EditorView): boolean {
  return (view as unknown as { destroyed?: unknown }).destroyed === true;
}

/** The context as of now, read from the states and (for the rect) layout. */
function derive(hub: Hub): FloatingContext {
  const { main } = hub;
  if (!writable(main.state)) return null;
  const cell = openCellView(main);
  if (cell) {
    const range = cell.state.selection.main;
    const caret = toRect(
      rangeBox(cell, range.from, range.to) ??
        cell.dom.parentElement?.getBoundingClientRect() ??
        EMPTY,
    );
    // The table element, found from the cell's DOM: no grid code here.
    const table = cell.dom.closest("table")?.getBoundingClientRect();
    const laidOut =
      table !== undefined && (table.width > 0 || table.height > 0);
    const fits = laidOut && table.height <= window.innerHeight;
    return {
      kind: "cell",
      rect: fits ? toRect(table) : caret,
      caret,
      selection: !range.empty,
    };
  }
  const range = main.state.selection.main;
  if (range.empty) return null;
  if (touchesTable(main.state, range.from, range.to)) return null;
  return {
    kind: "text",
    rect: toRect(rangeBox(main, range.from, range.to) ?? EMPTY),
  };
}

function sameRect(a: DOMRect, b: DOMRect): boolean {
  return (
    a.left === b.left &&
    a.top === b.top &&
    a.right === b.right &&
    a.bottom === b.bottom
  );
}

function same(a: FloatingContext, b: FloatingContext): boolean {
  if (a === null || b === null) return a === b;
  if (a.kind !== b.kind) return false;
  if (a.kind === "cell" && b.kind === "cell") {
    if (a.selection !== b.selection || !sameRect(a.caret, b.caret)) {
      return false;
    }
  }
  return sameRect(a.rect, b.rect);
}

function emit(hub: Hub, context: FloatingContext): void {
  if (hubs.get(hub.main) !== hub) return; // replaced or destroyed
  if (hub.last !== undefined && same(hub.last, context)) return;
  hub.last = context;
  hub.onChange(context);
}

/** Re-derives the hub's context in the main view's next measure pass. */
function schedule(hub: Hub): void {
  if (hubs.get(hub.main) !== hub || isDestroyed(hub.main)) return;
  hub.main.requestMeasure({
    key: hub.key,
    read: () => derive(hub),
    write: (context) => {
      emit(hub, context);
    },
  });
}

/**
 * Whether the person moved the selection in this update: a `select` user
 * event (arrows, Shift+arrows, a click, select all). Typing, a peer's edit
 * and the cell editor's own forwarding carry none, so a rect that moved for
 * those reasons is not a new place.
 */
function selectedByHand(update: ViewUpdate): boolean {
  return update.transactions.some((tr) => tr.isUserEvent("select"));
}

/**
 * How often the person moved a selection on `main` or in its open cell. The
 * bar compares it to tell a new place from a rect that only moved.
 */
export function selectionMoves(main: EditorView): number {
  return hubs.get(main)?.moves ?? 0;
}

/**
 * The cell editor open on `main` right now, or null. Eager, so the sticky
 * Formatting bar can run its inline commands in the cell the person is
 * typing in without importing the grid: while a cell is open, main's own
 * selection is stale (the cell sends only its caret), and a command run
 * there would change bytes the person is not looking at.
 */
export function openCellView(main: EditorView): EditorView | null {
  if (mainFocused(main)) return null;
  const cell = hubs.get(main)?.cell ?? null;
  return cell && !isDestroyed(cell) ? cell : null;
}

/**
 * Whether main itself holds the focus: the person is typing in main even
 * though a cell is still mounted (Shift+Tab back in, a `main.focus()`), so
 * main's selection is the live one. Read from the DOM, because
 * `EditorView.hasFocus` also asks the document, which a test may not have.
 */
function mainFocused(main: EditorView): boolean {
  return main.dom.ownerDocument.activeElement === main.contentDOM;
}

function changed(update: ViewUpdate): boolean {
  return (
    update.selectionSet ||
    update.docChanged ||
    update.geometryChanged ||
    update.viewportChanged ||
    update.focusChanged ||
    update.transactions.some((tr) => tr.reconfigured) ||
    syntaxTree(update.startState) !== syntaxTree(update.state)
  );
}

/**
 * Tells `onChange` the floating context of the main view this sits in, and
 * of the cell editor open in it. Only changes are reported, and the first
 * look after a buffer is (re)built always is: the React state it feeds
 * outlives the buffer.
 */
export function floatingContextListener(
  onChange: (context: FloatingContext) => void,
): Extension {
  const plugin = ViewPlugin.fromClass(
    class {
      readonly hub: Hub;

      constructor(view: EditorView) {
        this.hub = {
          main: view,
          onChange,
          cell: null,
          last: undefined,
          key: {},
          moves: 0,
        };
        hubs.set(view, this.hub);
        schedule(this.hub);
      }

      update(update: ViewUpdate): void {
        if (selectedByHand(update)) this.hub.moves += 1;
        if (changed(update)) schedule(this.hub);
      }

      destroy(): void {
        if (hubs.get(this.hub.main) === this.hub) hubs.delete(this.hub.main);
      }
    },
  );
  return [
    plugin,
    // Alt+F10 to the bar; nothing happens (and the key goes on) with no bar.
    Prec.high(
      keymap.of([{ key: "Alt-F10", run: (view) => focusBar(view, view) }]),
    ),
  ];
}

/**
 * The cell view's side, added by the grid to every cell editor it mounts on
 * `main`: reports the cell's caret and selection to `main`'s hub while the
 * view lives, and takes Alt+F10 to the bar from inside the cell.
 */
export function floatingCell(main: EditorView): Extension {
  const plugin = ViewPlugin.fromClass(
    class {
      readonly view: EditorView;

      constructor(view: EditorView) {
        this.view = view;
        const hub = hubs.get(main);
        if (!hub) return;
        hub.cell = view;
        schedule(hub);
      }

      update(update: ViewUpdate): void {
        const hub = hubs.get(main);
        if (hub?.cell !== this.view) return;
        if (selectedByHand(update)) hub.moves += 1;
        if (changed(update)) schedule(hub);
      }

      destroy(): void {
        const hub = hubs.get(main);
        if (hub?.cell !== this.view) return;
        hub.cell = null;
        schedule(hub);
      }
    },
  );
  return [
    plugin,
    Prec.high(
      keymap.of([{ key: "Alt-F10", run: (view) => focusBar(main, view) }]),
    ),
  ];
}
