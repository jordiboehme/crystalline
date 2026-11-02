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
  /** The caret, or a selection, in the open cell. */
  | { kind: "cell"; rect: DOMRect; selection: boolean }
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
  const cell = hub.cell;
  if (cell && !isDestroyed(cell)) {
    const range = cell.state.selection.main;
    const box =
      rangeBox(cell, range.from, range.to) ??
      cell.dom.parentElement?.getBoundingClientRect() ??
      EMPTY;
    return { kind: "cell", rect: toRect(box), selection: !range.empty };
  }
  const range = main.state.selection.main;
  if (range.empty) return null;
  if (inTable(main.state, range.from) || inTable(main.state, range.to)) {
    return null;
  }
  return {
    kind: "text",
    rect: toRect(rangeBox(main, range.from, range.to) ?? EMPTY),
  };
}

function same(a: FloatingContext, b: FloatingContext): boolean {
  if (a === null || b === null) return a === b;
  if (a.kind !== b.kind) return false;
  if (a.kind === "cell" && b.kind === "cell" && a.selection !== b.selection) {
    return false;
  }
  return (
    a.rect.left === b.rect.left &&
    a.rect.top === b.rect.top &&
    a.rect.right === b.rect.right &&
    a.rect.bottom === b.rect.bottom
  );
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
        };
        hubs.set(view, this.hub);
        schedule(this.hub);
      }

      update(update: ViewUpdate): void {
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
        if (!changed(update)) return;
        const hub = hubs.get(main);
        if (hub?.cell === this.view) schedule(hub);
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
