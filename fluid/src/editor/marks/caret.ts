/**
 * The caret over hidden marks. A run of marks is zero width on screen, so
 * every position inside it is one drawn caret place, and each drawn place has
 * exactly one position: `canonical`. An arrow press steps one visible
 * character (`stepVisible`), so no press leaves the drawn caret where it was,
 * and every empty caret a local transaction sets rests on its canonical
 * position (`caretNormalizer`). A caret move never changes a byte.
 *
 * The rule for where a canonical caret sits: at the start of a line (or a
 * cell) it goes past the prefix marks and the openings of formatting that
 * continues (bold, italic, strikethrough, an observation's category), so the
 * first letter typed takes the formatting on its right; anywhere else the
 * formatting on its left wins, and it never goes past an opening mark.
 */

import type {
  EditorState,
  Extension,
  RangeSet,
  RangeValue,
  SelectionRange,
} from "@codemirror/state";
import {
  EditorSelection,
  EditorState as State,
  Facet,
  Prec,
  findClusterBreak,
} from "@codemirror/state";
import type { Command } from "@codemirror/view";
import { EditorView, ViewPlugin, keymap } from "@codemirror/view";

import { mainSelectionParked } from "../parkedSelection";
import type { MarkKind, MarkRun } from "./hiddenMarks";
import { runAt, runsOnLine } from "./hiddenMarks";

/** A stretch a caret stays inside: a line, or a table cell's content. */
export interface Segment {
  from: number;
  to: number;
}

type SegmentOf = (state: EditorState, pos: number) => Segment | null;

/**
 * The segment `pos` lies in. The table cell editor provides its content
 * range (Task 12); without a provider, or when the provider answers null, it
 * is `pos`'s line.
 */
export const markSegment = Facet.define<SegmentOf, SegmentOf | null>({
  combine: (values) => values[0] ?? null,
});

export function segmentAt(state: EditorState, pos: number): Segment {
  const segment = state.facet(markSegment)?.(state, pos) ?? null;
  if (segment) {
    return segment;
  }
  const line = state.doc.lineAt(pos);
  return { from: line.from, to: line.to };
}

/** Formatting that a letter typed beside its mark takes on. */
const CONTINUING = new Set<MarkKind>(["strong", "em", "strike", "category"]);

/** The first position of `segment` that is not indentation. */
export function contentStart(state: EditorState, segment: Segment): number {
  const text = state.doc.sliceString(segment.from, segment.to);
  const indent = /^[ \t]*/.exec(text)?.[0].length ?? 0;
  return segment.from + indent;
}

function canonicalIn(state: EditorState, pos: number, run: MarkRun): number {
  const start = contentStart(state, segmentAt(state, pos));
  let p = run.from;
  if (run.from === start) {
    // At the start of a line or cell: past the prefixes and the openings of
    // continuing formatting, never into a link or code.
    for (const mark of run.marks) {
      if (
        mark.side === "prefix" ||
        (mark.side === "open" && CONTINUING.has(mark.kind))
      ) {
        p = mark.to;
      } else {
        break;
      }
    }
    return p;
  }
  // Mid-line and at the line end the formatting on the left wins: past the
  // closing of a link or code, never past an opening mark.
  for (const mark of run.marks) {
    if (mark.side === "close" && !CONTINUING.has(mark.kind)) {
      p = mark.to;
    }
  }
  return p;
}

/** The one position of the drawn caret place `pos` is in. */
export function canonical(state: EditorState, pos: number): number {
  const run = runAt(state, pos);
  return run ? canonicalIn(state, pos, run) : pos;
}

/**
 * The atomic ranges of a view's decorations (chips, pills, widgets): what a
 * step crosses whole. A state alone has none.
 */
export type Atoms = readonly RangeSet<RangeValue>[];

export function viewAtoms(view: EditorView): Atoms {
  return view.state
    .facet(EditorView.atomicRanges)
    .map((ranges) => ranges(view));
}

/** The far end of the atom with an edge at `pos` in direction `forward`. */
export function atomFrom(
  atoms: Atoms,
  pos: number,
  forward: boolean,
): number | null {
  let end: number | null = null;
  for (const set of atoms) {
    set.between(pos, pos, (from, to) => {
      if (forward && from === pos && to > pos) {
        end = Math.max(end ?? to, to);
      } else if (!forward && to === pos && from < pos) {
        end = Math.min(end ?? from, from);
      }
    });
  }
  return end;
}

/**
 * The canonical position one visible character on from `pos`, or null at a
 * segment edge (only hidden marks lie between `pos` and the edge). A run of
 * marks is crossed for free, an atom whole, anything else by one grapheme.
 */
export function stepVisible(
  state: EditorState,
  pos: number,
  forward: boolean,
  atoms: Atoms = [],
): number | null {
  const segment = segmentAt(state, pos);
  const line = state.doc.lineAt(pos);
  let q = pos;
  const run = runAt(state, q);
  if (forward) {
    if (run) {
      q = run.to;
    }
    if (q >= segment.to) {
      return null;
    }
    const atom = atomFrom(atoms, q, true);
    q =
      atom ??
      line.from + findClusterBreak(line.text, q - line.from, true, true);
  } else {
    if (run) {
      q = run.from;
    }
    if (q <= segment.from) {
      return null;
    }
    const atom = atomFrom(atoms, q, false);
    q =
      atom ??
      line.from + findClusterBreak(line.text, q - line.from, false, true);
  }
  return canonical(state, q);
}

/**
 * `range` with each edge outside half a run: a left edge inside a run moves
 * to the run's end, a right edge to its start, so a format command never sees
 * half a pair. The direction of the range is kept.
 */
export function snapRange(
  state: EditorState,
  range: SelectionRange,
): SelectionRange {
  if (range.empty) {
    return range;
  }
  const from = runAt(state, range.from)?.to ?? range.from;
  const to = runAt(state, range.to)?.from ?? range.to;
  if (from >= to) {
    return range;
  }
  return range.head < range.anchor
    ? EditorSelection.range(to, from)
    : EditorSelection.range(from, to);
}

/** Whether `pos`'s line holds a run at all; without one the default keys run. */
function hasRuns(state: EditorState, pos: number): boolean {
  return runsOnLine(state, state.doc.lineAt(pos).number).length > 0;
}

function arrow(forward: boolean, extend: boolean): Command {
  return (view) => {
    const { state } = view;
    const { selection } = state;
    if (selection.ranges.length > 1) {
      return false;
    }
    const range = selection.main;
    if ((!extend && !range.empty) || !hasRuns(state, range.head)) {
      return false;
    }
    const head = stepVisible(state, range.head, forward, viewAtoms(view));
    if (head === null) {
      return false;
    }
    view.dispatch({
      selection: extend
        ? EditorSelection.range(range.anchor, head)
        : EditorSelection.cursor(head),
      userEvent: "select",
      scrollIntoView: true,
    });
    return true;
  };
}

/**
 * CodeMirror's group motion, taken again while it only crossed hidden marks
 * (a `**` is a group of its own to it), so the press always moves the drawn
 * caret; the head lands canonical.
 */
function group(forward: boolean, extend: boolean): Command {
  return (view) => {
    const { state } = view;
    const { selection } = state;
    if (selection.ranges.length > 1) {
      return false;
    }
    const range = selection.main;
    if ((!extend && !range.empty) || !hasRuns(state, range.head)) {
      return false;
    }
    const start = canonical(state, range.head);
    let at = EditorSelection.cursor(range.head);
    let head: number | null = null;
    // A line holds few runs; the bound only guards against a motion that
    // stops moving.
    for (let step = 0; step < 64; step += 1) {
      const next = view.moveByGroup(at, forward);
      if (next.head === at.head) {
        break;
      }
      at = next;
      const place = canonical(state, next.head);
      if (place !== start) {
        head = place;
        break;
      }
    }
    if (head === null) {
      return false;
    }
    view.dispatch({
      selection: extend
        ? EditorSelection.range(range.anchor, head)
        : EditorSelection.cursor(head),
      userEvent: "select",
      scrollIntoView: true,
    });
    return true;
  };
}

/**
 * ArrowLeft and ArrowRight, with and without Shift, one visible character per
 * press, and the group motion (Mod, Alt on macOS) one visible group. At a segment edge it hands the key on: to the grid's way into a
 * table, the cell's cross-cell arrows, the id lines' caret guard, or
 * CodeMirror's default.
 */
export const caretKeymap: Extension = Prec.highest(
  keymap.of([
    { key: "ArrowLeft", run: arrow(false, false), shift: arrow(false, true) },
    { key: "ArrowRight", run: arrow(true, false), shift: arrow(true, true) },
    {
      key: "Mod-ArrowLeft",
      mac: "Alt-ArrowLeft",
      run: group(false, false),
      shift: group(false, true),
    },
    {
      key: "Mod-ArrowRight",
      mac: "Alt-ArrowRight",
      run: group(true, false),
      shift: group(true, true),
    },
  ]),
);

function cross(forward: boolean): Command {
  return (view) => {
    const { state } = view;
    const { selection } = state;
    if (selection.ranges.length > 1 || !selection.main.empty) {
      return false;
    }
    const head = selection.main.head;
    const line = state.doc.lineAt(head);
    // Only where hidden marks lie between the caret and the line's edge: from
    // the raw edge the default arrow already crosses.
    if (head === (forward ? line.to : line.from)) {
      return false;
    }
    if (forward ? line.number === state.doc.lines : line.number === 1) {
      return false;
    }
    if (stepVisible(state, head, forward, viewAtoms(view)) !== null) {
      return false;
    }
    const target = forward ? line.to + 1 : line.from - 1;
    view.dispatch({
      selection: EditorSelection.cursor(canonical(state, target)),
      userEvent: "select",
      scrollIntoView: true,
    });
    return true;
  };
}

/**
 * An empty caret at the visible edge of a line with hidden marks after it
 * (or before it) goes over the line break to the neighbour line's canonical
 * stop. Without it the default arrow reaches only the raw edge across the
 * atomic run, and the normalizer pulls the caret back. `Prec.high` and placed
 * after the table grid, so the way into a table comes first.
 */
export const crossEdge: Extension = Prec.high(
  keymap.of([
    { key: "ArrowLeft", run: cross(false) },
    { key: "ArrowRight", run: cross(true) },
  ]),
);

/** The views the hidden-mark layer runs in, to ask whether one composes. */
const liveViews = new Set<EditorView>();

const composingWatch = ViewPlugin.define((view) => {
  liveViews.add(view);
  return {
    destroy() {
      liveViews.delete(view);
    },
  };
});

function composing(state: EditorState): boolean {
  for (const view of liveViews) {
    if (view.state === state) {
      return view.composing;
    }
  }
  return false;
}

/**
 * Every empty range a local transaction sets goes to its canonical position,
 * judged on the resulting state. Non-empty ranges stay: a run is zero width,
 * so nothing moves on screen, and a copy keeps the marks at its edges. It
 * never steers an IME composition, a remote change (which sets no
 * selection), or main's selection while it is parked behind an open cell.
 */
const normalizer = State.transactionFilter.of((tr) => {
  if (
    tr.selection === undefined ||
    tr.isUserEvent("input.type.compose") ||
    tr.startState.facet(mainSelectionParked) ||
    composing(tr.startState)
  ) {
    return tr;
  }
  const state = tr.state;
  if (state.facet(mainSelectionParked)) {
    return tr;
  }
  const selection = state.selection;
  let moved = false;
  const ranges = selection.ranges.map((range) => {
    if (!range.empty) {
      return range;
    }
    const pos = canonical(state, range.head);
    if (pos === range.head) {
      return range;
    }
    moved = true;
    return EditorSelection.cursor(
      pos,
      range.assoc,
      range.bidiLevel ?? undefined,
      range.goalColumn,
    );
  });
  if (!moved) {
    return tr;
  }
  return [
    tr,
    {
      selection: EditorSelection.create(ranges, selection.mainIndex),
      sequential: true,
    },
  ];
});

export const caretNormalizer: Extension = [composingWatch, normalizer];
