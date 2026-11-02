/**
 * One table cell edited in a second EditorView.
 *
 * The cell view holds the WHOLE main document (the same `Text`, the same line
 * separator), so a position in one view is the same position in the other
 * and a change set from one applies to the other as it is. Everything outside
 * the cell is hidden by replace decorations, a transaction filter keeps local
 * edits and the selection inside the cell, and an update listener forwards
 * every local change to the main view tagged `fromCell`, together with the
 * cell's caret as the main view's `openCellField`.
 *
 * The way back is `cellForwarding` on the main view: every main change that
 * did not come from the cell (local typing in the main view, a remote change,
 * a table verb, an undo) is applied to the cell view tagged `fromMain`, and
 * the cell's range is re-read from the parsed table each time. When the views
 * disagree about the document (a main filter refused or rebuilt a forwarded
 * change) the cell is resynced from the main document as a whole.
 *
 * Undo: the cell view has no history of its own. Its Mod-z, Mod-Shift-z and
 * Mod-y run on the main view's stack, so CodeMirror's history or
 * Y.UndoManager stays the only one.
 */

import {
  defaultKeymap,
  historyField,
  redo as historyRedo,
  undo as historyUndo,
} from "@codemirror/commands";
import type { Extension, Text, TransactionSpec } from "@codemirror/state";
import {
  Annotation,
  ChangeSet,
  EditorSelection,
  EditorState,
  Facet,
  Prec,
  StateEffect,
  StateField,
  Transaction,
} from "@codemirror/state";
import type { DecorationSet, ViewUpdate } from "@codemirror/view";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
  keymap,
} from "@codemirror/view";
import { ySyncFacet, yUndoManagerKeymap } from "y-codemirror.next";

import { documentLanguage } from "../setup";
import { formattingKeymap } from "../toolbar";
import { cellPreviewLayers } from "./draws";
import type { MountedCell, OpenCell } from "./openCell";
import {
  bumpGeneration,
  closeCell,
  closeCellAt,
  inUpdate,
  isDestroyed,
  mountedCell,
  openCellField,
  openCellRange,
  registerCell,
  setOpenCell,
  unregisterCell,
} from "./openCell";

export interface CellEditor {
  view: EditorView;
  /**
   * Closes the cell through `closeCell` (reason "user", focus left alone)
   * while this is the main view's active cell editor; afterwards only makes
   * sure its view is gone. Safe to call twice.
   */
  destroy(): void;
}

interface CellRange {
  from: number;
  to: number;
}

/** Marks a main-view transaction that a cell editor forwarded. */
export const fromCell = Annotation.define<boolean>();
/** Marks a cell-view transaction that came from the main view. */
const fromMain = Annotation.define<boolean>();
/** Sets the cell's content range outright. */
const setRange = StateEffect.define<CellRange>();

/**
 * The content range inside the cell view's own state. A main change sets it
 * from the parsed table; a local edit maps it inclusively, so typing at either
 * edge grows the cell.
 */
const rangeField = StateField.define<CellRange>({
  create: () => ({ from: 0, to: 0 }),
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setRange)) return effect.value;
    }
    if (!tr.docChanged) return value;
    const from = tr.changes.mapPos(value.from, -1);
    return { from, to: Math.max(from, tr.changes.mapPos(value.to, 1)) };
  },
});

const hidden = Decoration.replace({});

function rangeMoved(tr: Transaction): boolean {
  return tr.docChanged || tr.effects.some((effect) => effect.is(setRange));
}

/** Atomic ranges from a decoration field, so the caret steps over them. */
function atomicField(
  build: (state: EditorState) => DecorationSet,
): StateField<DecorationSet> {
  return StateField.define<DecorationSet>({
    create: build,
    update: (value, tr) => (rangeMoved(tr) ? build(tr.state) : value),
    provide: (field) => [
      EditorView.decorations.from(field),
      EditorView.atomicRanges.of((view) => view.state.field(field)),
    ],
  });
}

/**
 * Everything outside the cell's content, hidden. A StateField because the
 * replace decorations cross line breaks.
 */
const hideOutside = atomicField((state) => {
  const { from, to } = state.field(rangeField);
  const end = state.doc.length;
  const ranges = [];
  if (from > 0) ranges.push(hidden.range(0, from));
  if (to < end) ranges.push(hidden.range(to, end));
  return Decoration.set(ranges);
});

class LineBreakWidget extends WidgetType {
  override eq(): boolean {
    return true;
  }

  override toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = "cm-cell-break";
    span.appendChild(document.createElement("br"));
    return span;
  }
}

const lineBreak = Decoration.replace({ widget: new LineBreakWidget() });

/** A `\|` or a `<br>` tag inside the cell's text. */
const MARKS = /\\\||<br\s*\/?>/gi;

/**
 * Inside the cell: the backslash of each `\|` hidden, each `<br>` drawn as a
 * line break. Both atomic.
 */
const cellMarks = atomicField((state) => {
  const { from, to } = state.field(rangeField);
  const text = state.doc.sliceString(from, to);
  const ranges = [];
  for (const match of text.matchAll(MARKS)) {
    const at = from + match.index;
    ranges.push(
      match[0] === "\\|"
        ? hidden.range(at, at + 1)
        : lineBreak.range(at, at + match[0].length),
    );
  }
  // The trailing backslash pairs before the closing pipe: the escaping half
  // of each is hidden, so `\\` reads as one `\`.
  const run = trailingRun(state.doc, { from, to });
  for (let at = run; at < to; at += 2) ranges.push(hidden.range(at, at + 1));
  return Decoration.set(ranges, true);
});

/**
 * Where the run of backslashes ending at the cell's end starts, when the
 * cell's closing pipe follows right after it; `range.to` otherwise (no run).
 */
function trailingRun(doc: EditorState["doc"], range: CellRange): number {
  if (doc.sliceString(range.to, range.to + 1) !== "|") return range.to;
  let start = range.to;
  while (start > range.from && doc.sliceString(start - 1, start) === "\\")
    start -= 1;
  return start;
}

function clampSelection(
  selection: EditorSelection,
  range: CellRange,
): EditorSelection {
  const clamp = (pos: number) => Math.min(range.to, Math.max(range.from, pos));
  return EditorSelection.create(
    selection.ranges.map((r) =>
      EditorSelection.range(clamp(r.anchor), clamp(r.head)),
    ),
    selection.mainIndex,
  );
}

/** Carries a transaction's user event over to a rebuilt spec. */
function userEventOf(tr: Transaction): { userEvent?: string } {
  const event = tr.annotation(Transaction.userEvent);
  return event === undefined ? {} : { userEvent: event };
}

/**
 * One local change, made safe for the cell: line breaks out, every pipe
 * escaped, and an escaped pair never split - a deletion that takes one half
 * of a `\|` takes the other half too, and an insert between the two halves
 * moves in front of the backslash. The same holds for the backslash pairs
 * right before the closing pipe (see `closingParity`).
 */
function cellChange(
  doc: EditorState["doc"],
  range: CellRange,
  fromA: number,
  toA: number,
  text: string,
  floor: number,
): { from: number; to: number; insert: string } {
  const insert = text.replace(/\r\n|\r|\n/g, "").replaceAll("|", "\\|");
  let from = fromA;
  let to = toA;
  const at = (pos: number) => doc.sliceString(pos, pos + 1);
  if (from < to) {
    if (at(from) === "|" && from - 1 >= Math.max(range.from, floor)) {
      if (at(from - 1) === "\\") from -= 1;
    }
    if (at(to - 1) === "\\" && to + 1 <= range.to && at(to) === "|") to += 1;
  } else if (from - 1 >= Math.max(range.from, floor)) {
    if (at(from - 1) === "\\" && at(from) === "|") {
      from -= 1;
      to = from;
    }
  }
  const run = trailingRun(doc, range);
  if (run < range.to && (range.to - run) % 2 === 0) {
    const pairStart = (pos: number) => run + Math.floor((pos - run) / 2) * 2;
    if (from < to && to > run && from < range.to) {
      if (from > run) from = Math.max(pairStart(from), floor);
      to = Math.min(range.to, run + Math.ceil((to - run) / 2) * 2);
    } else if (from === to && from > run && from < range.to) {
      from = Math.max(pairStart(from), floor);
      to = from;
    }
  }
  return { from, to, insert };
}

/**
 * R8: a local edit must never leave an odd run of backslashes right before
 * the cell's closing pipe, where the last one would escape the pipe and the
 * row would lose a column. When it would, one more `\` goes in at the cell's
 * end in the same change: a typed `\` at the end is stored `\\`, and a
 * deletion that exposes a `\` there doubles it.
 */
function closingParity(
  doc: EditorState["doc"],
  range: CellRange,
  kept: { from: number; to: number; insert: string }[],
  lineBreak: string,
): boolean {
  const changes = ChangeSet.of(kept, doc.length, lineBreak);
  const next = changes.apply(doc);
  const from = changes.mapPos(range.from, -1);
  const to = Math.max(from, changes.mapPos(range.to, 1));
  const run = trailingRun(next, { from, to });
  if ((to - run) % 2 === 0) return false;
  const last = kept.at(-1);
  if (last && last.to === range.to) last.insert += "\\";
  else kept.push({ from: range.to, to: range.to, insert: "\\" });
  return true;
}

/**
 * Keeps local cell edits inside the cell: a change not fully inside
 * `[from, to]` is dropped, inserts lose their line breaks and get their pipes
 * escaped, an insert left empty is dropped, and the selection - an explicit
 * one too - is clamped to the mapped cell range.
 */
const clampToCell = EditorState.transactionFilter.of(
  (tr: Transaction): TransactionSpec | readonly TransactionSpec[] => {
    if (tr.annotation(fromMain)) return tr;
    const range = tr.startState.field(rangeField);
    if (!tr.docChanged) {
      if (!tr.selection) return tr;
      const clamped = clampSelection(tr.selection, range);
      if (clamped.eq(tr.selection)) return tr;
      return [
        {
          selection: clamped,
          effects: tr.effects,
          scrollIntoView: tr.scrollIntoView,
        },
      ];
    }
    const doc = tr.startState.doc;
    const kept: { from: number; to: number; insert: string }[] = [];
    let altered = false;
    tr.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
      if (fromA < range.from || toA > range.to) {
        altered = true;
        return;
      }
      const text = inserted.sliceString(0, inserted.length, "\n");
      const floor = kept.at(-1)?.to ?? 0;
      const change = cellChange(doc, range, fromA, toA, text, floor);
      if (change.insert !== text || change.from !== fromA || change.to !== toA)
        altered = true;
      if (change.insert.length === 0 && change.from === change.to) return;
      kept.push(change);
    });
    if (closingParity(doc, range, kept, tr.startState.lineBreak))
      altered = true;
    if (!altered) {
      if (!tr.selection) return tr;
      const clamped = clampSelection(tr.selection, tr.state.field(rangeField));
      if (clamped.eq(tr.selection)) return tr;
      return [
        {
          changes: tr.changes,
          selection: clamped,
          effects: tr.effects,
          scrollIntoView: tr.scrollIntoView,
          ...userEventOf(tr),
        },
      ];
    }
    const changes = ChangeSet.of(kept, doc.length, tr.startState.lineBreak);
    const from = changes.mapPos(range.from, -1);
    const mapped = { from, to: Math.max(from, changes.mapPos(range.to, 1)) };
    // An explicit selection is in the coordinates of the refused changes:
    // take it back to the start document, then through the kept ones.
    const base = tr.selection
      ? tr.selection.map(tr.changes.invertedDesc, 1)
      : tr.startState.selection;
    return [
      {
        ...(kept.length > 0 ? { changes } : {}),
        selection: clampSelection(base.map(changes, 1), mapped),
        effects: tr.effects,
        scrollIntoView: tr.scrollIntoView,
        ...(kept.length > 0 ? userEventOf(tr) : {}),
      },
    ];
  },
);

/** The undo stack of `main`: CodeMirror's history solo, Y.UndoManager in a room. */
export function runMainUndo(main: EditorView, which: "undo" | "redo"): boolean {
  if (main.state.field(historyField, false) !== undefined) {
    return (which === "undo" ? historyUndo : historyRedo)(main);
  }
  if (main.state.facet(ySyncFacet) === undefined) return false;
  const key = which === "undo" ? "Mod-z" : "Mod-Shift-z";
  const binding = yUndoManagerKeymap.find((b) => b.key === key);
  try {
    return binding?.run?.(main) ?? false;
  } catch {
    // A room built without an undo manager has no stack to run.
    return false;
  }
}

function insertLineBreakTag(view: EditorView): boolean {
  const range = view.state.selection.main;
  view.dispatch({
    changes: { from: range.from, to: range.to, insert: "<br>" },
    selection: { anchor: range.from + 4 },
    userEvent: "input",
  });
  return true;
}

/** The open cell's content range in `main`, or null. */
function contentOf(state: EditorState): CellRange | null {
  return openCellRange(state)?.content ?? null;
}

function selectionFor(open: OpenCell | null, range: CellRange) {
  const anchor = open?.anchor ?? range.to;
  const head = open?.head ?? range.to;
  return clampSelection(
    EditorSelection.create([EditorSelection.range(anchor, head)]),
    range,
  );
}

/** Replaces the cell view's document with the main one, as a whole. */
function resync(view: EditorView, main: EditorView): void {
  if (isDestroyed(main) || mountedCell(main)?.view !== view) return;
  if (view.state.doc.eq(main.state.doc)) return;
  const range = contentOf(main.state) ?? view.state.field(rangeField);
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: main.state.doc },
    selection: selectionFor(
      main.state.field(openCellField, false) ?? null,
      range,
    ),
    effects: setRange.of(range),
    annotations: [fromMain.of(true), Transaction.addToHistory.of(false)],
  });
}

/**
 * Marks a state that carries `cellForwarding`, so its teardown can tell a
 * reconfigure that took it away (Raw) from a `setState` that may bring it
 * back.
 */
const forwardingOn = Facet.define<boolean, boolean>({
  combine: (values) => values.some(Boolean),
});

/** Feeds main changes into the mounted cell and runs the gone check. */
class Forwarding {
  readonly main: EditorView;
  /** The open cell's head as of the last update, for a close after it. */
  head: number | null;

  constructor(main: EditorView) {
    this.main = main;
    this.head = main.state.field(openCellField, false)?.head ?? null;
  }

  update(update: ViewUpdate): void {
    const main = update.view;
    const was = update.startState.field(openCellField, false) ?? null;
    const now = update.state.field(openCellField, false) ?? null;
    this.head = now?.head ?? null;
    const outside = update.transactions.filter(
      (tr) => !tr.annotation(fromCell),
    );
    const opened = outside.some((tr) =>
      tr.effects.some((e) => e.is(setOpenCell) && e.value !== null),
    );
    if (opened && now) bumpGeneration(main);
    if (was && !now) {
      const closedOnPurpose = update.transactions.some((tr) =>
        tr.effects.some((e) => e.is(setOpenCell) && e.value === null),
      );
      if (closedOnPurpose) closeCell(main, "user", "none");
      else closeCellAt(main, "gone", "main", update.changes.mapPos(was.head));
      return;
    }
    const cell = mountedCell(main);
    if (!cell || !now) return;
    const view = cell.view;
    let touched = false;
    for (const tr of update.transactions) {
      if (tr.annotation(fromCell)) continue;
      if (tr.docChanged || tr.effects.some((e) => e.is(setOpenCell)))
        touched = true;
      if (!tr.docChanged || !view.state.doc.eq(tr.startState.doc)) continue;
      view.dispatch({
        changes: tr.changes,
        annotations: [fromMain.of(true), Transaction.addToHistory.of(false)],
      });
    }
    if (!view.state.doc.eq(update.state.doc)) {
      resync(view, main);
      return;
    }
    if (!touched) return;
    const range = contentOf(update.state) ?? view.state.field(rangeField);
    const selection = selectionFor(now, range);
    const current = view.state.field(rangeField);
    const same =
      current.from === range.from &&
      current.to === range.to &&
      selection.eq(view.state.selection);
    if (same) return;
    view.dispatch({
      selection,
      effects: setRange.of(range),
      annotations: [fromMain.of(true)],
    });
  }

  destroy(): void {
    const main = this.main;
    if (!mountedCell(main) && !main.state.field(openCellField, false)) return;
    // Idle means `main.destroy()`; inside an update the state still carrying
    // this extension means `setState`, and one without it a reconfigure.
    const reason = !inUpdate(main)
      ? "destroy"
      : main.state.facet(forwardingOn)
        ? "swap"
        : "raw";
    closeCellAt(
      main,
      reason,
      reason === "destroy" ? "none" : "main",
      main.state.field(openCellField, false)?.head ?? this.head,
    );
  }
}

/**
 * The main-view side: the open-cell field, the forwarding of every main
 * change that did not come from the cell into it, and the gone check, which
 * closes through `closeCell`. Its teardown closes the cell too: Raw taking it
 * away ("raw"), `setState` ("swap") and the main view's destroy ("destroy").
 */
export const cellForwarding: Extension = [
  openCellField,
  forwardingOn.of(true),
  ViewPlugin.fromClass(Forwarding),
];

/** The cell editor mounted on `main`, if any. */
export function activeCell(main: EditorView): CellEditor | null {
  const mounted = mountedCell(main);
  return mounted ? (editors.get(mounted.view) ?? null) : null;
}

const editors = new WeakMap<EditorView, CellEditor>();

/**
 * Mounts the cell editor for `main`'s open cell (`openCellField`, which must
 * be set) into `parent`, with the cell's caret. One per main view: a second
 * mount destroys the first without closing the cell. `extensions` come
 * before the cell's own default keymap, so their bindings win over it, and
 * below its undo bindings.
 */
export function mountCellEditor(
  main: EditorView,
  parent: HTMLElement,
  extensions: Extension[] = [],
): CellEditor {
  const open = main.state.field(openCellField, false) ?? null;
  const range = contentOf(main.state);
  if (!open || !range) throw new Error("mountCellEditor: no open cell");
  const separator = main.state.facet(EditorState.lineSeparator);
  let closing = false;

  /**
   * Dispatches into main, after a microtask when main is mid-update. `start`
   * is the document a forwarded change applies to: when main has moved on
   * by the time a deferred one runs, the change is dropped and the cell is
   * resynced from main instead.
   */
  const tellMain = (spec: TransactionSpec, start?: Text) => {
    if (closing || isDestroyed(main)) return;
    if (!inUpdate(main)) {
      if (start && !main.state.doc.eq(start)) resync(view, main);
      else main.dispatch(spec);
      return;
    }
    queueMicrotask(() => {
      tellMain(spec, start);
    });
  };

  const forward = EditorView.updateListener.of((update) => {
    if (closing || isDestroyed(main)) return;
    const local = update.transactions.filter((tr) => !tr.annotation(fromMain));
    const own = (state: EditorState): OpenCell => ({
      anchor: state.selection.main.anchor,
      head: state.selection.main.head,
      focused: update.view.hasFocus,
    });
    let sent = false;
    for (const tr of local) {
      if (!tr.docChanged) continue;
      sent = true;
      tellMain(
        {
          changes: tr.changes,
          effects: setOpenCell.of(own(tr.state)),
          annotations: [fromCell.of(true)],
          ...userEventOf(tr),
        },
        tr.startState.doc,
      );
    }
    if (sent) {
      if (!main.state.doc.eq(update.state.doc))
        queueMicrotask(() => resync(update.view, main));
      return;
    }
    const moved = local.some((tr) => tr.selection !== undefined);
    if (moved || update.focusChanged) {
      tellMain({
        effects: setOpenCell.of(own(update.state)),
        annotations: [fromCell.of(true)],
      });
    }
  });

  const view = new EditorView({
    state: EditorState.create({
      doc: main.state.doc,
      selection: selectionFor(open, range),
      extensions: [
        ...(separator ? [EditorState.lineSeparator.of(separator)] : []),
        rangeField.init(() => range),
        Prec.highest([hideOutside, cellMarks]),
        clampToCell,
        forward,
        Prec.high(
          keymap.of([
            {
              key: "Mod-z",
              run: () => runMainUndo(main, "undo"),
              preventDefault: true,
            },
            {
              key: "Mod-Shift-z",
              run: () => runMainUndo(main, "redo"),
              preventDefault: true,
            },
            {
              key: "Mod-y",
              run: () => runMainUndo(main, "redo"),
              preventDefault: true,
            },
          ]),
        ),
        documentLanguage,
        ...main.state.facet(cellPreviewLayers),
        formattingKeymap,
        ...extensions,
        keymap.of([
          { key: "Shift-Enter", run: insertLineBreakTag },
          // Never text; a caller's Enter (navigation) comes first.
          { key: "Enter", run: () => true },
        ]),
        keymap.of(defaultKeymap),
      ],
    }),
    parent,
  });

  const entry: MountedCell = {
    view,
    parent,
    teardown() {
      if (closing) return;
      closing = true;
      unregisterCell(main, entry);
      view.destroy();
    },
  };
  const editor: CellEditor = {
    view,
    destroy() {
      if (mountedCell(main) === entry) closeCell(main, "user", "none");
      else entry.teardown();
    },
  };
  editors.set(view, editor);
  registerCell(main, entry);
  return editor;
}
