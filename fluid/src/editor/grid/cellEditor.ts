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
 *
 * Search: Mod-f, F3 and Mod-g (Shift for the previous hit) run the main
 * view's search from the cell's caret. Presence: `cellPresence` draws the
 * peers' carets in the cell and publishes the local one while it has focus.
 */

import {
  defaultKeymap,
  historyField,
  redo as historyRedo,
  isolateHistory,
  undo as historyUndo,
} from "@codemirror/commands";
import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import { findNext, findPrevious, openSearchPanel } from "@codemirror/search";
import type { Extension, Text, TransactionSpec } from "@codemirror/state";
import {
  Annotation,
  ChangeSet,
  Compartment,
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

import { copyAsHtml } from "../copyHtml";
import { hidesIdLines, pageHidesComments } from "../hiddenIdLines";
import { canonical, markSegment, stepVisible, viewAtoms } from "../marks/caret";
import { judgedElsewhere, keptWhole } from "../marks/deleting";
import { documentLanguage } from "../setup";
import { formatKeymap, pendingFormat } from "../marks/formatting";
import { marksOnEditLine } from "../marks/mode";
import { roomUndo } from "../roomUndo";
import { cellPreviewLayers } from "./draws";
import { cellPresence } from "./presence";
import type { MountedCell, OpenCell } from "./openCell";
import {
  bumpGeneration,
  cellMain,
  closeCell,
  closeCellAt,
  gridWritable,
  inUpdate,
  isDestroyed,
  mountedCell,
  openCellField,
  openCellRange,
  registerCell,
  searchFromCell,
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
/**
 * Marks a local insert whose text is already cell Markdown (a paste of
 * Fluid's own copy): its escapes stay as they are, and only a bare pipe gets
 * its backslash (`withEscapes`), so the paste is byte for byte.
 */
export const cellMarkdown = Annotation.define<boolean>();
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

/** A `<br>` tag inside the cell's text. */
const LINE_BREAK_TAG = /<br\s*\/?>/gi;

/**
 * One unit of the cell's text the caret and a deletion never split, in
 * document positions. Every backslash run is read in pairs, `\\` being one
 * escaped backslash (R11), and an odd last backslash belongs to the `|` it
 * escapes, or stands alone before any other character. `hide` is the
 * escaping backslash the cell view hides, if any.
 */
interface Unit {
  from: number;
  to: number;
  hide: number | null;
}

function cellUnits(doc: EditorState["doc"], range: CellRange): Unit[] {
  const text = doc.sliceString(range.from, range.to);
  const base = range.from;
  const units: Unit[] = [];
  const single = (at: number) =>
    units.push({ from: base + at, to: base + at + 1, hide: null });
  let i = 0;
  while (i < text.length) {
    if (text[i] !== "\\") {
      single(i);
      i += 1;
      continue;
    }
    let j = i;
    while (j < text.length && text[j] === "\\") j += 1;
    // Read in pairs anywhere: Markdown shows `\\` as one `\`.
    for (let k = i; k + 1 < j; k += 2) {
      units.push({ from: base + k, to: base + k + 2, hide: base + k });
    }
    if ((j - i) % 2 === 1) {
      if (j < text.length && text[j] === "|") {
        // The last backslash escapes the inner pipe after it.
        units.push({
          from: base + j - 1,
          to: base + j + 1,
          hide: base + j - 1,
        });
        i = j + 1;
        continue;
      }
      // A literal backslash before anything else, shown as it is. Before the
      // closing pipe only an edit in flight is odd here.
      single(j - 1);
    }
    i = j;
  }
  return units;
}

/** Whether `[from, to)` touches an escape unit whose backslash is hidden. */
function keepsEscape(state: EditorState, from: number, to: number): boolean {
  const range = state.field(rangeField, false);
  if (!range) return false;
  return cellUnits(state.doc, range).some(
    (unit) => unit.hide !== null && unit.from < to && from < unit.to,
  );
}

/**
 * Inside the cell: every escaping backslash hidden (an escaped `\|` shows
 * `|`, any `\\` shows `\`), each `<br>` drawn as a line break. The bytes are
 * never rewritten for it.
 * Both atomic.
 */
const cellMarks = atomicField((state) => {
  const range = state.field(rangeField);
  const text = state.doc.sliceString(range.from, range.to);
  const ranges = [];
  for (const unit of cellUnits(state.doc, range)) {
    if (unit.hide !== null) ranges.push(hidden.range(unit.hide, unit.hide + 1));
  }
  for (const match of text.matchAll(LINE_BREAK_TAG)) {
    const at = range.from + match.index;
    ranges.push(lineBreak.range(at, at + match[0].length));
  }
  return Decoration.set(ranges, true);
});

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

/**
 * Carries a transaction's user event over to a rebuilt spec, and its history
 * isolation: a gesture (a format, a popover commit, a mark-aware delete) is
 * one undo step of its own on main's stack too.
 */
function userEventOf(tr: Transaction): {
  userEvent?: string;
  annotations?: Annotation<unknown>[];
} {
  const event = tr.annotation(Transaction.userEvent);
  const isolate = tr.annotation(isolateHistory);
  return {
    ...(event === undefined ? {} : { userEvent: event }),
    ...(isolate === undefined
      ? {}
      : { annotations: [isolateHistory.of(isolate)] }),
  };
}

/**
 * One local change, made safe for the cell: line breaks out, every pipe
 * escaped, and no unit split (see `cellUnits`) - a deletion that takes part
 * of one takes all of it, and an insert inside one moves to its start.
 */
function cellChange(
  units: Unit[],
  fromA: number,
  toA: number,
  text: string,
  floor: number,
  markdown = false,
): { from: number; to: number; insert: string } {
  const flat = text.replace(/\r\n|\r|\n/g, "");
  // Markdown keeps its escapes; `withEscapes` adds the one a bare pipe needs.
  const insert = markdown ? flat : flat.replaceAll("|", "\\|");
  const inside = (pos: number) =>
    units.find((unit) => unit.from < pos && pos < unit.to);
  let from = inside(fromA)?.from ?? fromA;
  let to = fromA === toA ? from : (inside(toA)?.to ?? toA);
  from = Math.max(from, floor);
  if (fromA === toA) to = from;
  return { from, to: Math.max(from, to), insert };
}

/**
 * R8: after a local edit the cell's content holds no unescaped pipe (each
 * `|` has an odd backslash run before it) and an even run right before the
 * closing pipe. The one `\` each place needs goes into the same change: a
 * typed `\` at the end is stored `\\`, a `|` typed after a literal `\` is
 * stored `\\\|`. Returns `changes` itself when nothing was needed.
 */
export function withEscapes(
  doc: EditorState["doc"],
  range: CellRange,
  changes: ChangeSet,
  lineBreak: string,
): ChangeSet {
  const next = changes.apply(doc);
  const from = changes.mapPos(range.from, -1);
  const to = Math.max(from, changes.mapPos(range.to, 1));
  const text = next.sliceString(from, to);
  const fixes: { from: number; insert: string }[] = [];
  let run = 0;
  for (let i = 0; i < text.length; i += 1) {
    if (text[i] === "|" && run % 2 === 0)
      fixes.push({ from: from + i, insert: "\\" });
    run = text[i] === "\\" ? run + 1 : 0;
  }
  if (next.sliceString(to, to + 1) === "|" && run % 2 === 1) {
    fixes.push({ from: to, insert: "\\" });
  }
  if (fixes.length === 0) return changes;
  return changes.compose(ChangeSet.of(fixes, next.length, lineBreak));
}

/**
 * Whether a bare first cell's text would start another block (or indented
 * code) instead of a table row: a list marker, an ATX heading, a quote, an
 * HTML block, a fence, or a leading blank or tab that would grow the line's
 * indent (only those are Markdown indent: U+3000 or a no-break space is text).
 * These are the line starts the server's merge reads as a new block too.
 */
function opensBlock(text: string): boolean {
  return (
    /^[ \t]/.test(text) ||
    /^[-*+](\s|$)/.test(text) ||
    /^\d{1,9}[.)](\s|$)/.test(text) ||
    /^#{1,6}(\s|$)/.test(text) ||
    /^[><]/.test(text) ||
    /^(`{3,}|~{3,})/.test(text)
  );
}

/**
 * R24: the table's shape wins over byte fidelity outside the cell, by the
 * minimum. After a local edit (or a replace) of the cell whose content is
 * `range`, a cell on a bare edge (no edge pipe) that would stop being a cell
 * gets that edge's pipe in the same change:
 *
 * - a bare first cell left empty, or opening with a block marker, gets a
 *   leading `| ` (` | 2` would be a leading pipe and one cell, `- x | 2` a
 *   list item);
 * - a bare last cell left empty gets a closing ` |` (`1 | ` reads as a
 *   trailing pipe, the R10 shape).
 *
 * Returns the changes (itself when nothing was needed) and the cell's content
 * range in the document they make, which the added pipe stays outside of.
 */
export function withEdges(
  doc: EditorState["doc"],
  range: CellRange,
  changes: ChangeSet,
  lineBreak: string,
): { changes: ChangeSet; range: CellRange } {
  const from = changes.mapPos(range.from, -1);
  const to = Math.max(from, changes.mapPos(range.to, 1));
  const unchanged = { changes, range: { from, to } };
  // Only an edit that reaches the cell's opening or closing edge can empty
  // it or put a marker at its start; any other edit leaves the row as it is.
  let opening = false;
  let closing = false;
  changes.iterChangedRanges((fromA, toA) => {
    if (fromA <= range.from && toA >= range.from) opening = true;
    if (fromA <= range.to && toA >= range.to) closing = true;
  });
  if (changes.empty || (!opening && !closing)) return unchanged;
  const next = changes.apply(doc);
  const fix = edgeFix(doc, range, next, { from, to });
  if (!fix) return unchanged;
  if (fix.leading && !opening) return unchanged;
  if (!fix.leading && !closing) return unchanged;
  return {
    changes: changes.compose(
      ChangeSet.of([fix.change], next.length, lineBreak),
    ),
    range: fix.range,
  };
}

/** Blank as Markdown reads it: spaces and tabs, nothing else. */
function blank(text: string): boolean {
  return /^[ \t]*$/.test(text);
}

/**
 * The one edge pipe R24 adds to the cell whose content was `range` in `doc`
 * and is `now` in `next`, or null when it needs none. Whether an edge is
 * bare is read off `doc`, the line as it was.
 */
function edgeFix(
  doc: EditorState["doc"],
  range: CellRange,
  next: EditorState["doc"],
  now: CellRange,
): {
  change: { from: number; insert: string };
  range: CellRange;
  leading: boolean;
} | null {
  const line = doc.lineAt(range.from);
  if (range.to > line.to) return null;
  const leadBare = blank(doc.sliceString(line.from, range.from));
  const trailBare = /^[ \t\r]*$/.test(doc.sliceString(range.to, line.to));
  const text = next.sliceString(now.from, now.to);
  if (leadBare && (blank(text) || opensBlock(text))) {
    return {
      change: { from: now.from, insert: "| " },
      range: { from: now.from + 2, to: now.to + 2 },
      leading: true,
    };
  }
  if (trailBare && blank(text)) {
    return {
      change: { from: now.to, insert: " |" },
      range: { ...now },
      leading: false,
    };
  }
  return null;
}

/** Marks the edge fix a composition gets once it has ended. */
const edgeSettle = Annotation.define<boolean>();

/**
 * R24 for a composition: an IME (or the emoji picker) writes its text in
 * many transactions while `view.composing`, and an edge pipe added in the
 * middle would land inside the text being composed. So the cell filter skips
 * the edge rule for composing transactions, and once the composition has
 * ended this applies it once, to the cell as it now reads, with the pipe
 * outside the cell's range. It joins the composition's own undo step.
 *
 * The limit: a composition whose own text already ends the cell (it empties
 * a bare cell, or opens a bare first cell with `- `, `# `, `> `, ...) loses
 * the cell before it ends, as before R24, so there is nothing left to settle.
 */
function settleEdges(view: EditorView): void {
  if (view.composing || !view.dom.isConnected) return;
  const { state } = view;
  const range = state.field(rangeField);
  const fix = edgeFix(state.doc, range, state.doc, range);
  if (!fix) return;
  const head = state.selection.main.head + (fix.leading ? 2 : 0);
  view.dispatch({
    changes: fix.change,
    selection: {
      anchor: Math.min(fix.range.to, Math.max(fix.range.from, head)),
    },
    effects: setRange.of(fix.range),
    annotations: [edgeSettle.of(true)],
    userEvent: "input.type.compose",
  });
}

/**
 * Watches a cell view's compositions and settles the edges once one that
 * changed the document has ended. CodeMirror may flush the composition's
 * last DOM change a microtask after `compositionend` (Android later still),
 * so the settle waits for a task, and an update that lands after the end
 * settles too; whichever comes first does it, once.
 */
const composeEdges = ViewPlugin.fromClass(
  class {
    composed = false;
    ended = false;
    readonly view: EditorView;
    constructor(view: EditorView) {
      this.view = view;
    }
    update(update: ViewUpdate): void {
      if (update.docChanged && update.view.composing) this.composed = true;
      if (this.ended && update.docChanged && !update.view.composing) {
        queueMicrotask(() => {
          this.settle();
        });
      }
    }
    end(): void {
      if (!this.composed) return;
      this.ended = true;
      setTimeout(() => {
        this.settle();
      }, 0);
    }
    settle(): void {
      if (!this.ended || this.view.composing) return;
      this.ended = false;
      this.composed = false;
      settleEdges(this.view);
    }
  },
  {
    eventHandlers: {
      compositionend() {
        this.end();
      },
    },
  },
);

/**
 * Keeps local cell edits inside the cell: a change not fully inside
 * `[from, to]` is dropped, inserts lose their line breaks and get their pipes
 * escaped, an insert left empty is dropped, and the selection - an explicit
 * one too - is clamped to the mapped cell range.
 */
const clampToCell = EditorState.transactionFilter.of(
  (tr: Transaction): TransactionSpec | readonly TransactionSpec[] => {
    if (tr.annotation(fromMain) || tr.annotation(edgeSettle)) return tr;
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
    const units = cellUnits(doc, range);
    const kept: { from: number; to: number; insert: string }[] = [];
    let altered = false;
    tr.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
      if (fromA < range.from || toA > range.to) {
        altered = true;
        return;
      }
      const text = inserted.sliceString(0, inserted.length, "\n");
      const floor = kept.at(-1)?.to ?? 0;
      const change = cellChange(
        units,
        fromA,
        toA,
        text,
        floor,
        tr.annotation(cellMarkdown) === true,
      );
      if (change.insert !== text || change.from !== fromA || change.to !== toA)
        altered = true;
      if (change.insert.length === 0 && change.from === change.to) return;
      kept.push(change);
    });
    const lb = tr.startState.lineBreak;
    const plain = ChangeSet.of(kept, doc.length, lb);
    const escaped = withEscapes(doc, range, plain, lb);
    // A composition gets its edge pipe once it has ended (composeEdges).
    const edged = tr.isUserEvent("input.type.compose")
      ? { changes: escaped, range }
      : withEdges(doc, range, escaped, lb);
    const changes = edged.changes;
    if (changes !== plain) altered = true;
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
    // The content range in the new document: the inclusive mapping, unless
    // R24 added an edge pipe, which stays outside the cell.
    const mapped =
      edged.changes === escaped
        ? (() => {
            const from = changes.mapPos(range.from, -1);
            return { from, to: Math.max(from, changes.mapPos(range.to, 1)) };
          })()
        : edged.range;
    // An explicit selection is in the coordinates of the refused changes:
    // take it back to the start document, then through the kept ones.
    const base = tr.selection
      ? tr.selection.map(tr.changes.invertedDesc, 1)
      : tr.startState.selection;
    return [
      {
        ...(changes.empty ? {} : { changes }),
        selection: clampSelection(base.map(changes, 1), mapped),
        effects:
          edged.changes === escaped
            ? tr.effects
            : [...tr.effects, setRange.of(mapped)],
        scrollIntoView: tr.scrollIntoView,
        ...(changes.empty ? {} : userEventOf(tr)),
      },
    ];
  },
);

/**
 * Where an undo should put the open cell back: a key that moved the cell and
 * changed the document in one step (Tab or Enter adding a row) records the
 * document it left and the cell it left from. Neither CodeMirror's history
 * nor Y.UndoManager keeps the open cell, so the undo that brings exactly
 * that document back reopens it.
 */
export interface UndoLanding {
  /** The document before the step. */
  before: Text;
  /** The document right after it. */
  after: Text;
  /** Reopens the cell the step left from. */
  restore(): void;
}

const landings = new WeakMap<EditorView, UndoLanding>();

/** Records `landing` as the one an undo on `main` may land on. */
export function noteUndoLanding(main: EditorView, landing: UndoLanding): void {
  landings.set(main, landing);
}

function stackUndo(main: EditorView, which: "undo" | "redo"): boolean {
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

/**
 * The undo stack of `main`: CodeMirror's history solo, Y.UndoManager in a
 * room. An undo that takes the document from a recorded landing's `after`
 * back to its `before` reopens the landing's cell.
 */
export function runMainUndo(main: EditorView, which: "undo" | "redo"): boolean {
  const start = main.state.doc;
  const done = stackUndo(main, which);
  const landing = landings.get(main);
  if (!done || which !== "undo" || !landing) return done;
  if (start.eq(landing.after) && main.state.doc.eq(landing.before)) {
    landings.delete(main);
    landing.restore();
  }
  return done;
}

/** The main view a cell view edits a cell of (kept in `openCell.ts`). */
export { cellMain };

/**
 * Whether the cell view's caret is empty and at the visible start (`side`
 * -1) or end (1) of the cell's content: only hidden escapes or hidden marks
 * lie between it and the edge (C3). A leading `\|` hides its `\`, so the
 * caret after it still looks like the start; a canonical caret before the
 * closing `**` of a bold cell is at its end.
 */
export function atCellEdge(view: EditorView, side: -1 | 1): boolean {
  const { selection } = view.state;
  if (selection.ranges.length > 1 || !selection.main.empty) return false;
  const head = selection.main.head;
  const range = view.state.field(rangeField, false);
  if (!range) return false;
  if (side === 1 ? head >= range.to : head <= range.from) return true;
  if (stepVisible(view.state, head, side === 1, viewAtoms(view)) === null)
    return true;
  if (side === 1) return false;
  const first = cellUnits(view.state.doc, range)[0];
  return first?.hide === range.from && head === range.from + 1;
}

/**
 * A search command of the main view, run from the cell view. Main's
 * selection is stale while a cell is open (the cell sends only its caret),
 * so it first becomes the cell's: the search starts from the cell's caret and
 * a selected word seeds the query. The move carries `fromCell` and no select
 * event, so it neither opens nor closes a cell.
 */
function onMain(command: (main: EditorView) => boolean) {
  return (view: EditorView): boolean => {
    const main = view.state.facet(cellMain);
    if (!main || isDestroyed(main)) return false;
    if (view.state.doc.eq(main.state.doc)) {
      const { anchor, head } = view.state.selection.main;
      main.dispatch({
        selection: { anchor, head },
        annotations: [fromCell.of(true)],
      });
    }
    if (!view.hasFocus) return command(main);
    return searchFromCell(main, () => command(main));
  };
}

/** The main view's search keys, from inside the cell. */
const cellSearchKeys = keymap.of([
  { key: "Mod-f", run: onMain(openSearchPanel), preventDefault: true },
  {
    key: "F3",
    run: onMain(findNext),
    shift: onMain(findPrevious),
    preventDefault: true,
  },
  {
    key: "Mod-g",
    run: onMain(findNext),
    shift: onMain(findPrevious),
    preventDefault: true,
  },
]);

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

/**
 * The marks setting inside a cell view, read from main at mount and
 * reconfigured by `Forwarding` when main's flips, so the open cell switches
 * in place. One compartment serves every cell view: it is only a key.
 */
const cellMarksMode = new Compartment();

/**
 * Puts the cell view's caret into main's `openCellField`, so main holds the
 * head the cell shows (F1): after the mount, which settles the opening caret
 * on its drawn place, and after an outside change the cell's normalizer moved
 * the caret for. Main is often mid-update then, so it waits for a microtask;
 * it does nothing once the cell is gone or the views disagree.
 */
function settleMainCaret(main: EditorView, view: EditorView): void {
  const run = (): void => {
    if (isDestroyed(main) || mountedCell(main)?.view !== view) return;
    if (inUpdate(main) || inUpdate(view)) {
      queueMicrotask(run);
      return;
    }
    const open = main.state.field(openCellField, false);
    if (!open || !view.state.doc.eq(main.state.doc)) return;
    const { anchor, head } = view.state.selection.main;
    if (open.anchor === anchor && open.head === head) return;
    main.dispatch({
      effects: setOpenCell.of({ anchor, head, focused: open.focused }),
      annotations: [fromCell.of(true)],
    });
  };
  run();
}

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
      // Turned read-only (an ended room): the cell steps out to the grid.
      if (!gridWritable(update.state)) closeCell(main, "user", "grid");
      else if (closedOnPurpose) closeCell(main, "user", "none");
      else closeCellAt(main, "gone", "main", update.changes.mapPos(was.head));
      return;
    }
    const cell = mountedCell(main);
    if (!cell || !now) return;
    const view = cell.view;
    // The setting flipped: the open cell follows in place, with its caret.
    const show = update.state.facet(marksOnEditLine);
    if (view.state.facet(marksOnEditLine) !== show) {
      view.dispatch({
        effects: cellMarksMode.reconfigure(marksOnEditLine.of(show)),
        annotations: [fromMain.of(true)],
      });
      // The drawn place stays; its one position may differ in the new mode.
      const { main: range } = view.state.selection;
      const place = canonical(view.state, range.head);
      if (range.empty && place !== range.head) {
        view.dispatch({
          selection: { anchor: place },
          annotations: [fromMain.of(true)],
        });
      }
      settleMainCaret(main, view);
    }
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
    // The normalizer may have moved main's head to its drawn place.
    settleMainCaret(main, view);
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
 * How long a cell may parse the page up to its row when it opens: a
 * watchdog, not a budget. Main has parsed the same text already, so this is
 * tens of milliseconds on the longest page. Read from main, so a test can
 * make it fire.
 */
export const cellParseBudget = Facet.define<number, number>({
  combine: (values) => values[0] ?? 2_000,
});

/** What a delete in a cell says while its row is not read yet. */
export const CELL_NOT_READ =
  "This cell is still loading. Try again in a moment.";

/**
 * When the watchdog fired before the parse reached the row, the cell opens
 * with no marks hidden and no atoms drawn: a delete then could split a
 * label or leave a lone mark, and a format command would wrap raw marks.
 * Every delete and every command edit (a bare `input`: the format keys and
 * bar, links, popover commits) is refused, and so is a format waiting for
 * typing, with one sentence each, until the background parse has read the
 * row (D13). Plain typing stays.
 */
const unreadRowGuard = EditorState.transactionFilter.of((tr) => {
  const cell = tr.startState.field(rangeField, false);
  if (!cell || syntaxTree(tr.startState).length >= cell.to) return tr;
  const command =
    tr.docChanged &&
    (tr.isUserEvent("delete") ||
      tr.annotation(Transaction.userEvent) === "input");
  const waiting =
    (tr.startState.field(pendingFormat, false) ?? null) === null &&
    (tr.state.field(pendingFormat, false) ?? null) !== null;
  if (!command && !waiting) return tr;
  return {
    selection: tr.startState.selection,
    effects: EditorView.announce.of(CELL_NOT_READ),
  };
});

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
      const { userEvent, annotations = [] } = userEventOf(tr);
      tellMain(
        {
          changes: tr.changes,
          effects: setOpenCell.of(own(tr.state)),
          // The cell judged its own deletion with its row's marks shown or
          // hidden by its own caret; main would read the row as hidden.
          annotations: [
            fromCell.of(true),
            judgedElsewhere.of(true),
            ...annotations,
          ],
          ...(userEvent === undefined ? {} : { userEvent }),
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

  const fresh = EditorState.create({
    doc: main.state.doc,
    selection: selectionFor(open, range),
    extensions: [
      ...(separator ? [EditorState.lineSeparator.of(separator)] : []),
      cellMain.of(main),
      rangeField.init(() => range),
      Prec.highest([hideOutside, cellMarks]),
      clampToCell,
      unreadRowGuard,
      // Backspace and Delete over hidden marks leave every hidden escape to
      // `cellChange`, which takes such a unit whole.
      keptWhole.of(keepsEscape),
      composeEdges,
      forward,
      Prec.high([
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
        cellSearchKeys,
      ]),
      cellPresence,
      documentLanguage,
      // The cell's content is the segment the hidden-mark layer's caret,
      // delete and format rules stay inside; outside it, the line.
      markSegment.of((state, pos) => {
        const cell = state.field(rangeField, false);
        return cell && pos >= cell.from && pos <= cell.to ? cell : null;
      }),
      cellMarksMode.of(marksOnEditLine.of(main.state.facet(marksOnEditLine))),
      // A Reflection never shows a comment, in a cell too.
      pageHidesComments.of(hidesIdLines(main.state)),
      // The room's undo manager, so a gesture in the cell closes its
      // capture window (`stopCapturing`) as it does in main.
      roomUndo.of(main.state.facet(roomUndo)),
      ...main.state.facet(cellPreviewLayers),
      // Copy and cut carry HTML beside the Markdown, as in main.
      copyAsHtml(),
      formatKeymap,
      ...extensions,
      keymap.of([
        { key: "Shift-Enter", run: insertLineBreakTag },
        // Never text; a caller's Enter (navigation) comes first.
        { key: "Enter", run: () => true },
      ]),
      keymap.of(defaultKeymap),
    ],
  });
  // A new state parses only what fits a short time budget, and the rest
  // later in the background: on a long page that stops above the table, so
  // the cell would open with no marks hidden and no atoms drawn, and a key
  // pressed then would edit the raw text. Its row is read at once instead;
  // the empty update publishes the parse into the state.
  ensureSyntaxTree(fresh, range.to, main.state.facet(cellParseBudget));
  const created = fresh.update({}).state;
  // The caret the cell opens with rests on its drawn place, as every caret a
  // transaction sets does: "start" in a bold cell is inside the `**`. Main
  // takes that head once the cell is registered (`settleMainCaret`).
  const opened = created.selection.main;
  const settled = canonical(created, opened.head);
  const view = new EditorView({
    state:
      opened.empty && settled !== opened.head
        ? created.update({ selection: { anchor: settled } }).state
        : created,
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
  settleMainCaret(main, view);
  return editor;
}
