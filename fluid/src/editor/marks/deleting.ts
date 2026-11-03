/**
 * Backspace and Delete over hidden marks. A key removes exactly the visible
 * character the person sees (or one whole atom, such as a wikilink chip),
 * plus the marks a pair would otherwise be left with and nothing between
 * them. At the visible start of a line Backspace takes the line's block
 * prefix (a heading's `## `, a bullet and its box, the innermost `>`) whole.
 *
 * A range deletion a person makes (a selection deleted, cut, typed or pasted
 * over, a drag-move, an autocorrect replacement) keeps every mark whose
 * partner survives it (`keepPartnerMarks`), so `**` is never left alone.
 *
 * All of it runs only on lines whose marks are hidden: in show mode on the
 * line being edited and in Raw today's keys and deletions run unchanged.
 */

import { deleteCharBackward, deleteCharForward } from "@codemirror/commands";
import { isolateHistory } from "@codemirror/commands";
import type {
  ChangeSpec,
  EditorState,
  Extension,
  StateCommand,
  TransactionSpec,
} from "@codemirror/state";
import {
  Annotation,
  EditorSelection,
  EditorState as State,
  Facet,
  Prec,
  StateEffect,
  Transaction,
  findClusterBreak,
} from "@codemirror/state";
import type { Command } from "@codemirror/view";
import { EditorView, keymap } from "@codemirror/view";

import { roomUndo } from "../roomUndo";
import {
  atomFrom,
  canonical,
  contentStart,
  segmentAt,
  viewAtoms,
} from "./caret";
import type { Mark } from "./hiddenMarks";
import { marksOnLine, runAt } from "./hiddenMarks";
import { marksHiddenOnLine } from "./mode";

type HeadingQuestion = (
  state: EditorState,
  line: number,
  nextLevel: number,
) => string | null;

/**
 * The adapter's `confirmHeadingChange`: asked before Backspace takes a
 * heading's prefix away. A sentence means the change is refused.
 */
export const headingQuestion = Facet.define<
  HeadingQuestion,
  HeadingQuestion | null
>({
  combine: (values) => values[0] ?? null,
});

type KeptWhole = (state: EditorState, from: number, to: number) => boolean;

/**
 * Whether `[from, to)` touches a unit the segment keeps whole: in a table
 * cell, an escape whose backslash is hidden (`\|`, `\\`). The cell's own
 * change filter widens such a deletion to the whole unit, so Backspace and
 * Delete leave it to the default keys.
 */
export const keptWhole = Facet.define<KeptWhole>();

/** What a refused section heading change says. */
export const SECTION_HEADING = "Use Text style to change a section heading.";

interface Range {
  from: number;
  to: number;
}

/** Whether any position of `[from, to)` is neither removed nor a mark. */
function visibleBetween(
  from: number,
  to: number,
  removed: readonly Range[],
  marks: readonly Mark[],
): boolean {
  let pos = from;
  while (pos < to) {
    const skip =
      removed.find((r) => r.from <= pos && pos < r.to) ??
      marks.find((m) => m.from <= pos && pos < m.to);
    if (!skip) {
      return true;
    }
    pos = skip.to;
  }
  return false;
}

/**
 * `removal` plus every pair on the line it leaves with no visible character
 * between its marks.
 */
function withEmptyPairs(marks: readonly Mark[], removal: Range): Range[] {
  const removed: Range[] = [removal];
  for (const mark of marks) {
    if (mark.side !== "open" || mark.partner === null) {
      continue;
    }
    const close = marks[mark.partner];
    if (!close || mark.to > removal.from || removal.to > close.from) {
      continue;
    }
    if (!visibleBetween(mark.to, close.from, [removal], marks)) {
      removed.push(mark, close);
    }
  }
  return merged(removed);
}

function merged(ranges: Range[]): Range[] {
  const sorted = [...ranges].sort((a, b) => a.from - b.from);
  const out: Range[] = [];
  for (const range of sorted) {
    const last = out.at(-1);
    if (last && last.to >= range.from) {
      last.to = Math.max(last.to, range.to);
    } else {
      out.push({ from: range.from, to: range.to });
    }
  }
  return out;
}

/**
 * Removes `ranges` in one transaction and puts the caret at the canonical
 * place of `caretAt` (a position before the change) in the result.
 */
function remove(
  view: EditorView,
  ranges: readonly Range[],
  caretAt: number,
  forward: boolean,
  gesture = false,
): void {
  const { state } = view;
  const changes = state.changes(ranges.map(({ from, to }) => ({ from, to })));
  const userEvent = forward ? "delete.forward" : "delete.backward";
  const mapped = changes.mapPos(caretAt, -1);
  const after = state.update({ changes, userEvent }).state;
  const spec: TransactionSpec = {
    changes,
    selection: EditorSelection.cursor(canonical(after, mapped)),
    userEvent,
    scrollIntoView: true,
  };
  if (!gesture) {
    view.dispatch(spec);
    return;
  }
  // A block prefix taken away is a gesture of its own, not typing: one undo
  // step solo and in a room.
  const room = state.facet(roomUndo);
  room?.stopCapturing();
  view.dispatch({ ...spec, annotations: isolateHistory.of("full") });
  room?.stopCapturing();
}

/**
 * The default delete as it would run with the caret at `pos` and no atomic
 * range in the way: the line break (or the indentation) next to a visible
 * edge, never the hidden marks between the caret and the raw edge.
 */
function deleteAtEdge(
  view: EditorView,
  pos: number,
  forward: boolean,
): boolean {
  const { state } = view;
  if (state.selection.main.head === pos) {
    // The raw edge already: the default key does exactly this.
    return false;
  }
  let changes: Transaction["changes"] | null = null;
  const target = {
    // Unfiltered: the caret normalizer would put `pos` back at the caret.
    state: state.update({
      selection: EditorSelection.cursor(pos),
      filter: false,
    }).state,
    dispatch: (tr: Transaction) => {
      changes = tr.changes;
    },
  };
  // CodeMirror's char deletes are state commands at heart: on a target
  // that is no view they skip no atomic range, which is the point here.
  const command = (forward
    ? deleteCharForward
    : deleteCharBackward) as unknown as StateCommand;
  command(target);
  const found = changes as Transaction["changes"] | null;
  if (found === null) {
    // Nothing beyond the edge (the start or end of the document): the key
    // is spent, so the default does not take the hidden marks instead.
    return true;
  }
  const userEvent = forward ? "delete.forward" : "delete.backward";
  const head = state.selection.main.head;
  const mapped = found.mapPos(head, forward ? 1 : -1);
  const after = state.update({ changes: found, userEvent }).state;
  view.dispatch({
    changes: found,
    selection: EditorSelection.cursor(canonical(after, mapped)),
    userEvent,
    scrollIntoView: true,
  });
  return true;
}

/** Backspace at the visible start of a segment: the block rule. */
function blockRule(view: EditorView, start: number): boolean {
  const { state } = view;
  const run = runAt(state, start);
  const prefix: Mark[] = [];
  if (run && run.from === start) {
    for (const mark of run.marks) {
      if (mark.side !== "prefix") {
        break;
      }
      prefix.push(mark);
    }
  }
  const last = prefix.at(-1);
  if (!last) {
    return deleteAtEdge(view, start, false);
  }
  const line = state.doc.lineAt(start);
  const marks = marksOnLine(state, line.number);
  let ranges: Range[];
  if (last.kind === "heading") {
    const sentence =
      state.facet(headingQuestion)?.(state, line.number, 0) ?? null;
    if (sentence !== null) {
      view.dispatch({ effects: EditorView.announce.of(SECTION_HEADING) });
      return true;
    }
    const close = last.partner === null ? undefined : marks[last.partner];
    ranges = close ? [last, close] : [last];
  } else if (last.kind === "list" || last.kind === "task") {
    let first = prefix.length - 1;
    while (first > 0 && prefix[first]?.kind !== "list") {
      first -= 1;
    }
    ranges = merged(prefix.slice(first));
  } else {
    ranges = [last];
  }
  remove(view, ranges, state.selection.main.head, false, true);
  return true;
}

/**
 * Backspace (`forward` false) or Delete on a line whose marks are hidden,
 * from an empty caret. Returns false where today's key runs instead: show
 * mode on the line, Raw, a selection, several carets, indentation, a
 * hidden escape in a cell.
 */
export function deleteVisible(view: EditorView, forward: boolean): boolean {
  const { state } = view;
  const { selection } = state;
  if (state.readOnly || selection.ranges.length > 1 || !selection.main.empty) {
    return false;
  }
  const head = selection.main.head;
  const line = state.doc.lineAt(head);
  if (!marksHiddenOnLine(state, line.number)) {
    return false;
  }
  const segment = segmentAt(state, head);
  const start = contentStart(state, segment);
  if (head < start) {
    // In the indentation, which holds no marks: the default unindents.
    return false;
  }
  const run = runAt(state, head);
  const atoms = viewAtoms(view);
  let removal: Range;
  if (forward) {
    const q = run ? run.to : head;
    if (q >= segment.to) {
      return deleteAtEdge(view, segment.to, true);
    }
    const atom = atomFrom(atoms, q, true);
    removal = {
      from: q,
      to:
        atom ??
        line.from + findClusterBreak(line.text, q - line.from, true, true),
    };
  } else {
    const q = run ? run.from : head;
    if (q <= start) {
      return blockRule(view, start);
    }
    const atom = atomFrom(atoms, q, false);
    removal = {
      from:
        atom ??
        line.from + findClusterBreak(line.text, q - line.from, false, true),
      to: q,
    };
  }
  if (
    state.facet(keptWhole).some((kept) => kept(state, removal.from, removal.to))
  ) {
    return false;
  }
  const ranges = withEmptyPairs(marksOnLine(state, line.number), removal);
  remove(view, ranges, forward ? head : removal.from, forward);
  return true;
}

/** The level a heading line's text has, 0 when it is none. */
function headingLevel(text: string): number {
  const match = /^ {0,3}(#{1,6})(?:[ \t]|$)/.exec(text);
  return match?.[1]?.length ?? 0;
}

/**
 * Backspace or Delete inside the prefix of a heading the adapter guards (a
 * Reflection section heading) on a line whose marks show: the key that would
 * change its level is refused and says why, as on a hidden line. Any other
 * heading is left to today's keys.
 */
function guardShownHeading(view: EditorView, forward: boolean): boolean {
  const { state } = view;
  const ask = state.facet(headingQuestion);
  const { selection } = state;
  if (!ask || selection.ranges.length > 1 || !selection.main.empty) {
    return false;
  }
  const head = selection.main.head;
  const line = state.doc.lineAt(head);
  const level = headingLevel(line.text);
  if (level === 0) {
    return false;
  }
  const at = head - line.from;
  const from = forward ? at : at - 1;
  if (from < 0 || from >= line.length) {
    return false;
  }
  const text = line.text.slice(0, from) + line.text.slice(from + 1);
  const next = headingLevel(text);
  if (next === level) {
    return false;
  }
  const sentence = ask(state, line.number, next);
  if (sentence === null) {
    return false;
  }
  view.dispatch({ effects: EditorView.announce.of(SECTION_HEADING) });
  return true;
}

function key(forward: boolean): Command {
  return (view) =>
    deleteVisible(view, forward) || guardShownHeading(view, forward);
}

/**
 * Backspace and Delete. `Prec.highest`, ahead of the Markdown language's own
 * Backspace (`deleteMarkupBackward`). Mod-Backspace and Mod-Delete are not
 * bound, so the word deletes run as today.
 */
export const deletionKeymap: Extension = Prec.highest(
  keymap.of([
    { key: "Backspace", run: key(false) },
    { key: "Delete", run: key(true) },
  ]),
);

/** The range replacements a person makes, exactly; never a bare `input`. */
const PERSON_REPLACES = new Set([
  "input.type",
  "input.paste",
  "input.drop",
  "input.complete",
  "delete.selection",
  "delete.cut",
  "move.drop",
]);

/** The annotations a rebuilt edit carries over from the one it replaces. */
function carried(tr: Transaction): Annotation<unknown>[] {
  const out: Annotation<unknown>[] = [];
  const event = tr.annotation(Transaction.userEvent);
  if (event !== undefined) {
    out.push(Transaction.userEvent.of(event));
  }
  const history = tr.annotation(Transaction.addToHistory);
  if (history !== undefined) {
    out.push(Transaction.addToHistory.of(history));
  }
  const isolate = tr.annotation(isolateHistory);
  if (isolate !== undefined) {
    out.push(isolateHistory.of(isolate));
  }
  return out;
}

/**
 * The marks a range deletion keeps: one wholly inside a deleted range whose
 * partner lies outside every deleted range, and any mark a deleted range
 * only cuts into. A prefix, a comment or a mark whose partner goes too goes
 * with the range.
 */
function keptMarks(state: EditorState, deleted: readonly Range[]): Range[] {
  const kept: Range[] = [];
  const inside = (mark: Range) =>
    deleted.some((d) => d.from <= mark.from && mark.to <= d.to);
  const cut = (mark: Range) =>
    deleted.some((d) => d.from < mark.to && mark.from < d.to && !inside(mark));
  const lines = new Set<number>();
  for (const d of deleted) {
    const first = state.doc.lineAt(d.from).number;
    const last = state.doc.lineAt(d.to).number;
    for (let n = first; n <= last; n += 1) {
      lines.add(n);
    }
  }
  for (const n of lines) {
    if (!marksHiddenOnLine(state, n)) {
      continue;
    }
    const marks = marksOnLine(state, n);
    for (const mark of marks) {
      if (cut(mark)) {
        kept.push(mark);
        continue;
      }
      if (!inside(mark) || mark.partner === null) {
        continue;
      }
      if (mark.side !== "open" && mark.side !== "close") {
        continue;
      }
      const partner = marks[mark.partner];
      if (partner && !inside(partner)) {
        kept.push(mark);
      }
    }
  }
  return merged(kept);
}

/**
 * A range deletion a person makes keeps every mark whose partner survives:
 * the deletion splits around it, and the text put in lands where the
 * deletion started. A deletion that already keeps every partner passes
 * unchanged. Default precedence, so the id lines' and the frontmatter's
 * guards see the rewritten edit.
 */
export const keepPartnerMarks: Extension = State.transactionFilter.of((tr) => {
  const event = tr.annotation(Transaction.userEvent);
  if (
    !tr.docChanged ||
    event === undefined ||
    !PERSON_REPLACES.has(event) ||
    tr.annotation(Transaction.remote) !== undefined
  ) {
    return tr;
  }
  const start = tr.startState;
  const deleted: Range[] = [];
  tr.changes.iterChangedRanges((fromA, toA) => {
    if (fromA < toA) {
      deleted.push({ from: fromA, to: toA });
    }
  });
  if (deleted.length === 0) {
    return tr;
  }
  const kept = keptMarks(start, deleted);
  if (kept.length === 0) {
    return tr;
  }
  const specs: ChangeSpec[] = [];
  /** Where each original change's text went: `[fromB, toB, insertAt]`. */
  const placed: [number, number, number][] = [];
  tr.changes.iterChanges((fromA, toA, fromB, toB, inserted) => {
    const pieces: Range[] = [];
    let pos = fromA;
    let insertAt = fromA;
    for (const mark of kept) {
      if (mark.to <= fromA || mark.from >= toA) {
        continue;
      }
      if (mark.from > pos) {
        pieces.push({ from: pos, to: mark.from });
      } else if (pos === fromA) {
        // The range starts inside a kept mark: its text goes after it.
        insertAt = mark.to;
      }
      pos = Math.max(pos, mark.to);
    }
    if (pos < toA) {
      pieces.push({ from: pos, to: toA });
    }
    const first = pieces[0];
    if (first && first.from === insertAt) {
      specs.push({ from: first.from, to: first.to, insert: inserted });
      for (const piece of pieces.slice(1)) {
        specs.push(piece);
      }
    } else {
      specs.push({ from: insertAt, insert: inserted });
      for (const piece of pieces) {
        specs.push(piece);
      }
    }
    placed.push([fromB, toB, insertAt]);
  });
  const changes = start.changes(specs);
  const map = (pos: number): number => {
    for (const [fromB, toB, insertAt] of placed) {
      if (fromB <= pos && pos <= toB) {
        return changes.mapPos(insertAt, -1) + (pos - fromB);
      }
    }
    return changes.mapPos(tr.changes.invertedDesc.mapPos(pos));
  };
  const spec: TransactionSpec = {
    changes,
    annotations: carried(tr),
    effects: StateEffect.mapEffects(
      StateEffect.mapEffects(tr.effects, tr.changes.invertedDesc),
      changes.desc,
    ),
    scrollIntoView: tr.scrollIntoView,
  };
  if (tr.selection !== undefined) {
    const selection = tr.newSelection;
    spec.selection = EditorSelection.create(
      selection.ranges.map((range) =>
        EditorSelection.range(map(range.anchor), map(range.head)),
      ),
      selection.mainIndex,
    );
  }
  return spec;
});
