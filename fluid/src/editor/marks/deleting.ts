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
 * line being edited and in Raw today's keys and deletions run unchanged. One
 * rule holds on every line: Backspace and Delete never join a fence line the
 * code box hides (`codeBlocks.ts`) to the line beside it.
 */

import {
  deleteCharBackward,
  deleteCharForward,
  deleteGroupBackward,
  deleteGroupForward,
  deleteLineBoundaryBackward,
  deleteLineBoundaryForward,
} from "@codemirror/commands";
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
import { syntaxTree } from "@codemirror/language";

import { hiddenFenceLine } from "../codeBlocks";
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
 * Whether deleting the line break at `pos` (the start of its line for
 * Backspace, the end for Delete) would join a fence line the code box hides
 * to the line beside it: the first code line to the opening fence, the last
 * to the closing one, or the lines around the box to its fences. That key is
 * refused: it changes nothing.
 */
function joinsHiddenFence(
  state: EditorState,
  pos: number,
  forward: boolean,
): boolean {
  const line = state.doc.lineAt(pos);
  if (pos !== (forward ? line.to : line.from)) {
    return false;
  }
  const next = line.number + (forward ? 1 : -1);
  return next >= 1 && next <= state.doc.lines && hiddenFenceLine(state, next);
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
  if (joinsHiddenFence(state, pos, forward)) {
    return true;
  }
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
    if (atom === null && escapesNext(state, q)) {
      // A backslash and the character it escapes go as one.
      removal.to = q + 2;
    }
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
    if (
      atom === null &&
      removal.from > line.from &&
      escapesNext(state, removal.from - 1)
    ) {
      // The character is escaped: its backslash goes with it, or the
      // backslash left behind would escape whatever comes next.
      removal.from -= 1;
    }
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

/** What a backslash escapes in Markdown: ASCII punctuation. */
const ESCAPABLE = /^[!-/:-@[-`{-~]$/;

/**
 * Whether the character at `pos` is a backslash that escapes the one after
 * it: a punctuation character follows, and the backslash is not itself
 * escaped (an even run of backslashes before it).
 */
function escapesNext(state: EditorState, pos: number): boolean {
  const doc = state.doc;
  if (doc.sliceString(pos, pos + 1) !== "\\") {
    return false;
  }
  if (!ESCAPABLE.test(doc.sliceString(pos + 1, pos + 2))) {
    return false;
  }
  const line = doc.lineAt(pos);
  let before = 0;
  while (
    pos - before - 1 >= line.from &&
    doc.sliceString(pos - before - 1, pos - before) === "\\"
  ) {
    before += 1;
  }
  return before % 2 === 0;
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

/**
 * An empty caret right beside a hidden fence line, whichever mode its own
 * line is in: the key that would join them is spent.
 */
function refuseFenceJoin(view: EditorView, forward: boolean): boolean {
  const { state } = view;
  const { selection } = state;
  return (
    !state.readOnly &&
    selection.ranges.length === 1 &&
    selection.main.empty &&
    joinsHiddenFence(state, selection.main.head, forward)
  );
}

function key(forward: boolean): Command {
  return (view) =>
    refuseFenceJoin(view, forward) ||
    deleteVisible(view, forward) ||
    guardShownHeading(view, forward);
}

/** The range a CodeMirror delete command would take from `pos`, no atoms. */
function reach(
  state: EditorState,
  command: StateCommand,
  pos: number,
): Range | null {
  let found: Range | null = null;
  // A bare state on the same text: the command's own transaction would
  // otherwise pass this buffer's filters, which keep the marks it reaches.
  command({
    state: State.create({
      doc: state.doc,
      selection: EditorSelection.cursor(pos),
      extensions: [
        State.languageData.of(() =>
          state
            .languageDataAt("wordChars", pos)
            .map((wordChars) => ({ wordChars })),
        ),
      ],
    }),
    dispatch: (tr) => {
      tr.changes.iterChangedRanges((from, to) => {
        found = {
          from: Math.min(found?.from ?? from, from),
          to: Math.max(found?.to ?? to, to),
        };
      });
    },
  });
  return found;
}

/**
 * A word or line delete (`command`) that would reach only hidden marks, so
 * the press would change nothing a person sees: it runs again from the far
 * side of those marks, the way `group()` in `caret.ts` moves. The whole
 * stretch is deleted as one `delete.forward` or `delete.backward`, and
 * `keepPartnerMarks` keeps or drops the marks in it. Anywhere else the
 * command runs as today.
 */
function overHidden(command: Command, forward: boolean): Command {
  const plain = command as unknown as StateCommand;
  return (view) => {
    const { state } = view;
    const { selection } = state;
    if (
      state.readOnly ||
      selection.ranges.length > 1 ||
      !selection.main.empty
    ) {
      return false;
    }
    const head = selection.main.head;
    const line = state.doc.lineAt(head);
    if (!marksHiddenOnLine(state, line.number)) {
      return false;
    }
    const marks = marksOnLine(state, line.number);
    const first = reach(state, plain, head);
    if (!first || visibleBetween(first.from, first.to, [], marks)) {
      return false;
    }
    // On from the far side of what it reached, while that is only marks.
    // A line holds few marks; the bound only guards a command that stops.
    const range = { from: first.from, to: first.to };
    for (let step = 0; step < 64; step += 1) {
      const again = reach(state, plain, forward ? range.to : range.from);
      if (!again) {
        break;
      }
      range.from = Math.min(range.from, again.from);
      range.to = Math.max(range.to, again.to);
      if (visibleBetween(range.from, range.to, [], marks)) {
        break;
      }
    }
    if (!visibleBetween(range.from, range.to, [], marks)) {
      // Nothing visible lies that way: the key is spent.
      return true;
    }
    view.dispatch({
      changes: range,
      selection: EditorSelection.cursor(range.from),
      userEvent: forward ? "delete.forward" : "delete.backward",
      scrollIntoView: true,
    });
    return true;
  };
}

/**
 * Backspace and Delete. `Prec.highest`, ahead of the Markdown language's own
 * Backspace (`deleteMarkupBackward`). The word and line deletes run as today,
 * except where they would reach only hidden marks (`overHidden`).
 */
export const deletionKeymap: Extension = Prec.highest(
  keymap.of([
    { key: "Backspace", run: key(false) },
    { key: "Delete", run: key(true) },
    {
      key: "Mod-Backspace",
      mac: "Alt-Backspace",
      run: overHidden(deleteGroupBackward, false),
    },
    {
      key: "Mod-Delete",
      mac: "Alt-Delete",
      run: overHidden(deleteGroupForward, true),
    },
    {
      mac: "Mod-Backspace",
      run: overHidden(deleteLineBoundaryBackward, false),
    },
    { mac: "Mod-Delete", run: overHidden(deleteLineBoundaryForward, true) },
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
  // The word and line deletes (Mod/Alt/Cmd+Backspace and Delete), and the
  // plain keys where they fall through to CodeMirror's own command. A
  // deletion from `deleteVisible` already keeps every partner, so it passes.
  "delete.backward",
  "delete.forward",
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
 * The partner of a mark whose pair spans two lines (the model gives it no
 * partner on its own line): the other end of the syntax node the mark
 * belongs to, or null.
 */
export function partnerAcrossLines(
  state: EditorState,
  mark: Mark,
): Range | null {
  const node = syntaxTree(state).resolveInner(mark.from, 1);
  const parent =
    node.from === mark.from && node.to === mark.to ? node.parent : null;
  if (!parent) {
    return null;
  }
  const other = mark.side === "open" ? parent.lastChild : parent.firstChild;
  if (!other || other.from === mark.from || other.name !== node.name) {
    return null;
  }
  return { from: other.from, to: other.to };
}

/** The hidden marks of every line from `from`'s to `to`'s. */
function marksBetween(state: EditorState, from: number, to: number): Mark[] {
  const first = state.doc.lineAt(from).number;
  const last = state.doc.lineAt(to).number;
  const out: Mark[] = [];
  for (let n = first; n <= last; n += 1) {
    out.push(...marksOnLine(state, n));
  }
  return out;
}

/**
 * What a range deletion does to the marks: `kept` are the marks it must
 * leave (one wholly inside a deleted range whose partner lies outside every
 * deleted range, and any mark a deleted range only cuts into); `dropped` are
 * the marks of a pair the deletion leaves with no visible character and no
 * inserted text between them, which go too. A prefix, a comment or a mark
 * whose partner goes too goes with the range.
 */
function markFate(
  state: EditorState,
  deleted: readonly Range[],
  inserts: readonly number[],
): { kept: Range[]; dropped: Range[] } {
  const kept: Mark[] = [];
  const dropped: Range[] = [];
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
      if (!inside(mark) || (mark.side !== "open" && mark.side !== "close")) {
        continue;
      }
      const partner =
        mark.partner === null
          ? partnerAcrossLines(state, mark)
          : (marks[mark.partner] ?? null);
      if (partner && !inside(partner)) {
        kept.push(mark);
      }
    }
    // A pair whose marks both stay but whose inside the deletion emptied.
    const stays = (mark: Range) =>
      !inside(mark) || kept.some((k) => k.from === mark.from);
    for (const open of marks) {
      if (open.side !== "open") {
        continue;
      }
      const close =
        open.partner === null
          ? partnerAcrossLines(state, open)
          : (marks[open.partner] ?? null);
      if (!close || !stays(open) || !stays(close)) {
        continue;
      }
      const touched = deleted.some(
        (d) => d.from < close.from && open.to < d.to,
      );
      const typed = inserts.some((at) => open.to <= at && at <= close.from);
      if (
        touched &&
        !typed &&
        !visibleBetween(
          open.to,
          close.from,
          deleted,
          marksBetween(state, open.to, close.from),
        )
      ) {
        dropped.push(open, close);
      }
    }
  }
  const keptRanges = kept.filter(
    (mark) => !dropped.some((d) => d.from === mark.from),
  );
  return { kept: merged(keptRanges), dropped: merged(dropped) };
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
  const inserts: number[] = [];
  tr.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
    if (fromA < toA) {
      deleted.push({ from: fromA, to: toA });
    }
    if (inserted.length > 0) {
      inserts.push(fromA);
    }
  });
  if (deleted.length === 0) {
    return tr;
  }
  const { kept, dropped } = markFate(start, deleted, inserts);
  if (kept.length === 0 && dropped.length === 0) {
    return tr;
  }
  const specs: ChangeSpec[] = [];
  // The marks of an emptied pair that no deleted range covers.
  for (const mark of dropped) {
    if (!deleted.some((d) => d.from <= mark.from && mark.to <= d.to)) {
      specs.push({ from: mark.from, to: mark.to });
    }
  }
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
