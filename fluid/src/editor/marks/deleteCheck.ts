/**
 * Every deletion a key makes is checked against the text it leaves (D5), the
 * way the format commands check theirs: the characters a person still sees
 * keep the formats they had. Removing a visible character can stop a
 * neighbouring mark from pairing (`**a.** b` without its space reads
 * `**a.**b`, which is no bold at all), so a deletion is one of:
 *
 * 1. The exact deletion.
 * 2. The deletion with one named repair: a mark moved across the space now
 *    at its edge (`** b**` written ` **b**`), or two pairs of the same kind
 *    the deletion made touch merged into one (`**a****b**` written
 *    `**ab**`).
 * 3. Refused: the bytes stay and the page says so.
 *
 * Before the check, a pair the deletion left with no visible character loses
 * its marks too (D6, in show mode as well, where the keys are today's).
 *
 * Only Backspace, Delete and their word and line forms (`delete.backward`,
 * `delete.forward`) and a selection deleted (`delete.selection`) are
 * checked, and only the lines they touched are read again. Where a line
 * shows its marks, a key that takes one of them is left as it is: the person
 * is editing the Markdown there, and the plan keeps today's keys.
 */

import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import type {
  ChangeSet,
  ChangeSpec,
  EditorState,
  Extension,
  TransactionSpec,
} from "@codemirror/state";
import {
  Annotation,
  EditorSelection,
  EditorState as State,
  StateEffect,
  Transaction,
} from "@codemirror/state";
import { isolateHistory } from "@codemirror/commands";
import { EditorView } from "@codemirror/view";

import type { Mark, MarkKind } from "./hiddenMarks";
import { marksOnLine } from "./hiddenMarks";
import { marksHiddenOnLine } from "./mode";

/** What a refused deletion says. It names no mode: a Reflection has no Raw. */
export const CANNOT_JOIN = "This text cannot be joined here.";

/** How a checked deletion went; an exact one carries none. */
export type DeleteOutcome = "moved" | "merged" | "refused";

/** Names the outcome on the transaction a checked deletion became. */
export const deletionOutcome = StateEffect.define<DeleteOutcome>();

const CHECKED = new Set([
  "delete.backward",
  "delete.forward",
  "delete.selection",
]);

/** The formats a character can have that a deletion must keep. */
const KINDS = new Set<MarkKind>(["strong", "em", "strike", "code", "link"]);

interface Range {
  from: number;
  to: number;
}

interface Pair {
  kind: MarkKind;
  open: Range;
  close: Range;
  /** A link whose words are its label (`[ref]`, `[ref][]`). */
  labelled: boolean;
}

interface Lines {
  marks: Mark[];
  pairs: Pair[];
}

/** The marks and pairs of lines `first` to `last`, shown or hidden alike. */
function linesOf(state: EditorState, first: number, last: number): Lines {
  const marks: Mark[] = [];
  const pairs: Pair[] = [];
  for (let n = first; n <= last; n += 1) {
    const line = marksOnLine(state, n, { always: true });
    marks.push(...line);
    for (const mark of line) {
      if (mark.side !== "open" || mark.partner === null) continue;
      const close = line[mark.partner];
      if (!close || !KINDS.has(mark.kind)) continue;
      const tail = state.doc.sliceString(close.from, close.to);
      pairs.push({
        kind: mark.kind,
        open: { from: mark.from, to: mark.to },
        close: { from: close.from, to: close.to },
        labelled: mark.kind === "link" && (tail === "]" || tail === "][]"),
      });
    }
  }
  return { marks, pairs };
}

function inAny(ranges: readonly Range[], pos: number): boolean {
  return ranges.some((r) => r.from <= pos && pos < r.to);
}

function touches(ranges: readonly Range[], span: Range): boolean {
  return ranges.some((r) => r.from < span.to && span.from < r.to);
}

function isBlank(char: string): boolean {
  return char === "" || /\s/.test(char);
}

function merged(ranges: readonly Range[]): Range[] {
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

/** The visible positions strictly between a pair's marks. */
function inside(pair: Pair, marks: readonly Mark[]): number[] {
  const out: number[] = [];
  for (let p = pair.open.to; p < pair.close.from; p += 1) {
    if (!inAny(marks, p)) out.push(p);
  }
  return out;
}

/** `deleted` plus the marks of every pair it leaves with nothing visible. */
function withEmptied(lines: Lines, deleted: readonly Range[]): Range[] {
  const out = [...deleted];
  for (const pair of lines.pairs) {
    if (touches(deleted, pair.open) || touches(deleted, pair.close)) continue;
    const seen = inside(pair, lines.marks);
    if (seen.length > 0 && seen.every((p) => inAny(deleted, p))) {
      out.push(pair.open, pair.close);
    }
  }
  return merged(out);
}

/** The formats of position `pos`: the pairs of `pairs` around it. */
function formatsAt(pairs: readonly Pair[], pos: number): string {
  const kinds = pairs
    .filter((pair) => pair.open.to <= pos && pos < pair.close.from)
    .map((pair) => pair.kind)
    .sort();
  return kinds.join(" ");
}

/** `state` parsed at least to `upto`, or null when that does not fit. */
function parsedTo(state: EditorState, upto: number): EditorState | null {
  if (syntaxTree(state).length >= upto) return state;
  ensureSyntaxTree(state, upto, 50);
  const settled = state.update({}).state;
  return syntaxTree(settled).length >= upto ? settled : null;
}

/**
 * Whether `after` (the start state with `changes`, which delete `deleted`)
 * shows every character the deletion kept with the formats it had. A pair
 * the deletion targeted (it took one of its marks, or a letter of a link
 * whose words are its label) frees the characters inside it.
 */
function readsSame(
  start: EditorState,
  lines: Lines,
  first: number,
  last: number,
  changes: ChangeSet,
  deleted: readonly Range[],
  after: EditorState,
): boolean {
  const from = start.doc.line(first).from;
  const to = start.doc.line(last).to;
  const aFrom = changes.mapPos(from, -1);
  const aTo = changes.mapPos(to, 1);
  const parsed = parsedTo(after, aTo);
  // Not parsed far enough to tell: the deletion stands as it is.
  if (!parsed) return true;
  const next = linesOf(
    parsed,
    parsed.doc.lineAt(aFrom).number,
    parsed.doc.lineAt(aTo).number,
  );
  const targeted = lines.pairs.filter(
    (pair) =>
      touches(deleted, pair.open) ||
      touches(deleted, pair.close) ||
      (pair.labelled &&
        touches(deleted, { from: pair.open.to, to: pair.close.from })),
  );
  const text = start.doc.sliceString(from, to);
  for (let p = from; p < to; p += 1) {
    if (inAny(deleted, p) || inAny(lines.marks, p)) continue;
    if (isBlank(text[p - from] ?? "")) continue;
    if (targeted.some((pair) => pair.open.to <= p && p < pair.close.from)) {
      continue;
    }
    const q = changes.mapPos(p, 1);
    if (inAny(next.marks, q)) return false;
    if (formatsAt(lines.pairs, p) !== formatsAt(next.pairs, q)) return false;
  }
  return true;
}

/**
 * The repairs to try when the exact deletion breaks a pair: for each pair
 * it left untouched, a mark moved across the space the deletion put at its
 * edge, and two pairs of one kind whose marks the deletion made touch,
 * merged.
 */
function repairs(
  start: EditorState,
  lines: Lines,
  deleted: readonly Range[],
): { specs: ChangeSpec[]; outcome: DeleteOutcome }[] {
  const doc = start.doc;
  const out: { specs: ChangeSpec[]; outcome: DeleteOutcome }[] = [];
  const free = lines.pairs.filter(
    (pair) => !touches(deleted, pair.open) && !touches(deleted, pair.close),
  );
  for (const pair of free) {
    // A space now right inside the opening mark goes before it.
    let s = pair.open.to;
    while (s < pair.close.from && inAny(deleted, s)) s += 1;
    if (s > pair.open.to && s < pair.close.from) {
      const char = doc.sliceString(s, s + 1);
      if (char === " " || char === "\t") {
        out.push({
          specs: [
            { from: pair.open.from, insert: char },
            { from: s, to: s + 1 },
          ],
          outcome: "moved",
        });
      }
    }
    // A space now right inside the closing mark goes after it.
    let e = pair.close.from - 1;
    while (e >= pair.open.to && inAny(deleted, e)) e -= 1;
    if (e < pair.close.from - 1 && e >= pair.open.to) {
      const char = doc.sliceString(e, e + 1);
      if (char === " " || char === "\t") {
        out.push({
          specs: [
            { from: e, to: e + 1 },
            { from: pair.close.to, insert: char },
          ],
          outcome: "moved",
        });
      }
    }
  }
  for (const left of free) {
    for (const right of free) {
      if (
        left === right ||
        left.kind !== right.kind ||
        left.kind === "link" ||
        left.close.to >= right.open.from
      ) {
        continue;
      }
      // Only what the deletion took lies between them.
      let p = left.close.to;
      while (p < right.open.from && inAny(deleted, p)) p += 1;
      if (p !== right.open.from) continue;
      if (
        doc.sliceString(left.close.from, left.close.to) !==
        doc.sliceString(right.open.from, right.open.to)
      ) {
        continue;
      }
      out.push({
        specs: [
          { from: left.close.from, to: left.close.to },
          { from: right.open.from, to: right.open.to },
        ],
        outcome: "merged",
      });
    }
  }
  return out;
}

/** The annotations a rebuilt deletion carries over from the one it replaces. */
function carried(tr: Transaction): Annotation<unknown>[] {
  const out: Annotation<unknown>[] = [];
  const event = tr.annotation(Transaction.userEvent);
  if (event !== undefined) out.push(Transaction.userEvent.of(event));
  const history = tr.annotation(Transaction.addToHistory);
  if (history !== undefined) out.push(Transaction.addToHistory.of(history));
  const isolate = tr.annotation(isolateHistory);
  if (isolate !== undefined) out.push(isolateHistory.of(isolate));
  return out;
}

/** `tr` rebuilt with `changes` in place of its own. */
function rebuilt(
  tr: Transaction,
  changes: ChangeSet,
  outcome: DeleteOutcome | null,
): TransactionSpec {
  const back = tr.changes.invertedDesc;
  const selection = tr.newSelection;
  return {
    changes,
    selection: EditorSelection.create(
      selection.ranges.map((range) =>
        EditorSelection.range(
          changes.mapPos(back.mapPos(range.anchor, -1), -1),
          changes.mapPos(back.mapPos(range.head, -1), -1),
        ),
      ),
      selection.mainIndex,
    ),
    annotations: carried(tr),
    effects: [
      ...StateEffect.mapEffects(
        StateEffect.mapEffects(tr.effects, back),
        changes.desc,
      ),
      ...(outcome ? [deletionOutcome.of(outcome)] : []),
    ],
    scrollIntoView: tr.scrollIntoView,
  };
}

/**
 * The check (D5). Listed before `keepPartnerMarks` in the layer, so it runs
 * after that filter (filters run last listed first) and judges the deletion
 * as it will land.
 */
export const checkedDeletion: Extension = State.transactionFilter.of((tr) => {
  const event = tr.annotation(Transaction.userEvent);
  if (
    !tr.docChanged ||
    event === undefined ||
    !CHECKED.has(event) ||
    tr.annotation(Transaction.remote) !== undefined
  ) {
    return tr;
  }
  const start = tr.startState;
  const deleted: Range[] = [];
  let inserts = false;
  tr.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
    if (fromA < toA) deleted.push({ from: fromA, to: toA });
    if (inserted.length > 0) inserts = true;
  });
  // A replacement is not a deletion to judge; neither is nothing removed.
  if (inserts || deleted.length === 0) return tr;
  const firstPos = Math.min(...deleted.map((d) => d.from));
  const lastPos = Math.max(...deleted.map((d) => d.to));
  const first = start.doc.lineAt(firstPos).number;
  const last = start.doc.lineAt(lastPos).number;
  const lines = linesOf(start, first, last);
  // A line with no pair can still gain one (`q * a* r` without its space
  // reads italic): where marks are hidden that is checked too (D10). Where
  // they show, such a line is left to today's keys.
  if (lines.pairs.length === 0) {
    for (let n = first; n <= last; n += 1) {
      if (!marksHiddenOnLine(start, n)) return tr;
    }
  }
  // Where a line shows its marks, a key that takes one is the person editing
  // the Markdown itself (today's keys): the pairs after it may pair anew.
  if (
    lines.marks.some(
      (mark) =>
        touches(deleted, mark) &&
        !marksHiddenOnLine(start, start.doc.lineAt(mark.from).number),
    )
  ) {
    return tr;
  }
  const asked = merged(deleted);
  const ranges = withEmptied(lines, asked);
  const grown =
    ranges.length !== asked.length ||
    ranges.some((r, i) => r.from !== asked[i]?.from || r.to !== asked[i]?.to);
  const exact = grown ? start.changes(ranges) : tr.changes;
  const exactState = grown
    ? start.update({ changes: exact, filter: false }).state
    : tr.state;
  const candidates = repairs(start, lines, ranges);
  // Two pairs of one kind the deletion made touch are always written as one
  // (`**a****b**` reads as bold, but nobody writes it that way).
  const merges = candidates.filter((c) => c.outcome === "merged");
  const ordered = [
    ...merges,
    ...candidates.filter((c) => c.outcome !== "merged"),
  ];
  if (
    merges.length === 0 &&
    readsSame(start, lines, first, last, exact, ranges, exactState)
  ) {
    return grown ? rebuilt(tr, exact, null) : tr;
  }
  for (const repair of ordered) {
    const changes = start.changes([
      ...ranges.map(({ from, to }) => ({ from, to })),
      ...repair.specs,
    ]);
    const trial = start.update({ changes, filter: false }).state;
    if (readsSame(start, lines, first, last, changes, ranges, trial)) {
      return rebuilt(tr, changes, repair.outcome);
    }
  }
  if (readsSame(start, lines, first, last, exact, ranges, exactState)) {
    return grown ? rebuilt(tr, exact, null) : tr;
  }
  return {
    selection: start.selection,
    effects: [
      EditorView.announce.of(CANNOT_JOIN),
      deletionOutcome.of("refused"),
    ],
  };
});
