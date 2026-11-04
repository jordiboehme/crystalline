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
 * `delete.forward`), a selection deleted (`delete.selection`) and a cut
 * (`delete.cut`, D12) are checked. The check reads the paragraph or setext
 * heading around the lines they touched, since a pair can wrap onto the next
 * line, and never the whole document; one pass over that range answers it.
 *
 * Where marks show (show mode, the caret's lines), the lines the key touched
 * decide, not the paragraph read: a key that takes a shown mark is left as
 * it is (D9), and so is a key on shown lines that no pair reaches, even when
 * it makes a new pair (D11). The person is editing the Markdown there, and
 * the plan keeps today's keys. A key on such lines that takes no mark is
 * left alone for the lines that show their marks only: a pair it makes or
 * breaks that changes a character on a line whose marks are hidden is
 * refused (D23). A key that takes only shown marks stays unchecked even
 * then (D25); one that takes a normal character beside a shown mark is
 * judged (D28). A deletion during an IME composition and an
 * edit a table cell has judged already (`judgedElsewhere`) are left alone.
 */

import { syntaxTree } from "@codemirror/language";
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
import { scanMarks, walkTree } from "./hiddenMarks";
import { composing } from "./caret";
import { droppable, judgedElsewhere } from "./deleting";
import { marksHiddenOnLine } from "./mode";
import { PAGE_LOADING, PARSE_BUDGET, deadlineIn, parsedTo } from "./parse";
import type { Deadline } from "./parse";

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
  // A cut removes a selection too (D12).
  "delete.cut",
  // A picture's removal from its tools (P7).
  "delete.picture",
]);

/**
 * A test hook: the work the check has done, in all, beside the tree walks
 * `treeWalkWork` counts. `positions` are the characters its sweeps read and
 * `reads` the results it parsed and read. A cost test counts these instead
 * of timing a press, so it gives the same answer at any load.
 */
export const checkWork = { positions: 0, reads: 0 };

/** The formats a character can have that a deletion must keep. */
const KINDS = new Set<MarkKind>(["strong", "em", "strike", "code", "link"]);

export interface Range {
  from: number;
  to: number;
}

export interface Pair {
  kind: MarkKind;
  open: Range;
  close: Range;
  /** A link whose words are its label (`[ref]`, `[ref][]`). */
  labelled: boolean;
}

export interface Lines {
  marks: Mark[];
  pairs: Pair[];
}

/**
 * The marks and pairs of lines `first` to `last`, shown or hidden alike, in
 * one walk over the tree. A pair across a soft line break (its marks on two
 * lines) is paired through the syntax node the two belong to.
 */
export function linesOf(
  state: EditorState,
  first: number,
  last: number,
): Lines {
  const marks: Mark[] = [];
  const pairs: Pair[] = [];
  const add = (open: Range, close: Range, kind: MarkKind) => {
    const tail = state.doc.sliceString(close.from, close.to);
    pairs.push({
      kind,
      open: { from: open.from, to: open.to },
      close: { from: close.from, to: close.to },
      labelled: kind === "link" && (tail === "]" || tail === "][]"),
    });
  };
  const across: Mark[] = [];
  const scan = scanMarks(state, first, last, { always: true });
  for (const line of scan.lines) {
    marks.push(...line);
    for (const mark of line) {
      if (mark.side !== "open" || !KINDS.has(mark.kind)) continue;
      if (mark.partner === null) {
        across.push(mark);
        continue;
      }
      const close = line[mark.partner];
      if (close) add(mark, close, mark.kind);
    }
  }
  if (across.length > 0) {
    const closes = new Map<number, Mark>();
    for (const mark of marks) {
      if (mark.side === "close" && !closes.has(mark.from)) {
        closes.set(mark.from, mark);
      }
    }
    for (const open of across) {
      const far = scan.closers.get(open.from);
      const close = far ? closes.get(far.from) : undefined;
      if (close) add(open, close, open.kind);
    }
  }
  return { marks, pairs };
}

/**
 * The lines a check reads: those the deletion touched, widened to the
 * paragraphs around them, so a pair that wraps onto the next line is read
 * whole. A paragraph is local; the document is never read whole. A deletion
 * that takes a character of a fence's marks reads the whole fence (to the
 * document's end for an unclosed one): its code lines may read as text after
 * it (M2, D10).
 */
export function blockLines(
  state: EditorState,
  from: number,
  to: number,
): { first: number; last: number } {
  let lo = from;
  let hi = to;
  walkTree(syntaxTree(state), from, to, (node) => {
    if (node.name === "Paragraph" || node.name.startsWith("SetextHeading")) {
      lo = Math.min(lo, node.from);
      hi = Math.max(hi, node.to);
      return false;
    }
    if (node.name === "FencedCode") {
      const fence = node.node;
      const marks = fence.getChildren("CodeMark");
      if (marks.some((mark) => mark.from < to && from < mark.to)) {
        lo = Math.min(lo, fence.from);
        hi = Math.max(hi, fence.to);
      }
      return false;
    }
    return undefined;
  });
  return {
    first: state.doc.lineAt(lo).number,
    last: state.doc.lineAt(Math.min(hi, state.doc.length)).number,
  };
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

export function merged(ranges: readonly Range[]): Range[] {
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
 * Whether a position lies in one of `ranges`, for positions asked in order
 * (never smaller than the one before): one pass over the ranges in all.
 */
function cover(ranges: readonly Range[]): (pos: number) => boolean {
  const sorted = merged(ranges);
  let i = 0;
  return (pos) => {
    while (i < sorted.length && (sorted[i]?.to ?? 0) <= pos) i += 1;
    const range = sorted[i];
    return range !== undefined && range.from <= pos;
  };
}

const KIND_ORDER = [...KINDS];

/**
 * The formats of a position (how many pairs of each kind are around it),
 * for positions asked in order: one pass over the pairs' edges in all.
 */
function formats(pairs: readonly Pair[]): (pos: number) => string {
  const opens = [...pairs].sort((a, b) => a.open.to - b.open.to);
  const closes = [...pairs].sort((a, b) => a.close.from - b.close.from);
  const counts = KIND_ORDER.map(() => 0);
  const bump = (kind: MarkKind, by: number) => {
    const i = KIND_ORDER.indexOf(kind);
    counts[i] = (counts[i] ?? 0) + by;
  };
  let o = 0;
  let c = 0;
  return (pos) => {
    for (; o < opens.length && (opens[o]?.open.to ?? 0) <= pos; o += 1) {
      const kind = opens[o]?.kind;
      if (kind) bump(kind, 1);
    }
    for (; c < closes.length && (closes[c]?.close.from ?? 0) <= pos; c += 1) {
      const kind = closes[c]?.kind;
      if (kind) bump(kind, -1);
    }
    return counts.join(" ");
  };
}

/**
 * `deleted` plus the marks of every pair it leaves with nothing visible. An
 * image, and a pair holding one, is never empty (D40).
 */
function withEmptied(
  start: EditorState,
  lines: Lines,
  deleted: readonly Range[],
): Range[] {
  const out = [...deleted];
  for (const pair of lines.pairs) {
    if (touches(deleted, pair.open) || touches(deleted, pair.close)) continue;
    if (
      !droppable(start, pair.open, pair.open.to, pair.close.from, lines.marks)
    ) {
      continue;
    }
    // Only a pair the deletion reaches into can be left empty.
    if (!touches(deleted, { from: pair.open.to, to: pair.close.from })) {
      continue;
    }
    const isMark = cover(lines.marks);
    const isDeleted = cover(deleted);
    let seen = false;
    let all = true;
    for (let p = pair.open.to; all && p < pair.close.from; p += 1) {
      checkWork.positions += 1;
      if (isMark(p)) continue;
      seen = true;
      if (!isDeleted(p)) all = false;
    }
    if (seen && all) out.push(pair.open, pair.close);
  }
  return merged(out);
}

/**
 * Whether `after` (the start state with `changes`, which delete `deleted`)
 * shows every character the deletion kept with the formats it had. A pair
 * the deletion targeted (it took one of its marks, or a letter of a link
 * whose words are its label) frees the characters inside it. Null when
 * `after` could not be parsed far enough to tell.
 *
 * With `shown` (the lines that show their marks), only the characters on
 * the other lines are judged, and no pair frees them: where the marks show
 * the person edits the Markdown (D9, D11), but a line whose marks are
 * hidden keeps its formats (D23).
 *
 * With `keepMarks`, every mark the change did not delete must still be a
 * mark after it: an insert (`insertCheck.ts`) can break a pair without
 * changing a visible character, its marks turning into page text.
 */
export function readsSame(
  start: EditorState,
  lines: Lines,
  first: number,
  last: number,
  changes: ChangeSet,
  deleted: readonly Range[],
  after: EditorState,
  deadline: Deadline,
  shown?: readonly Range[],
  keepMarks = false,
): boolean | null {
  const from = start.doc.line(first).from;
  const to = start.doc.line(last).to;
  const aFrom = changes.mapPos(from, -1);
  const aTo = changes.mapPos(to, 1);
  checkWork.reads += 1;
  const parsed = parsedTo(after, aTo, deadline);
  // Not parsed far enough to tell: the caller refuses rather than writes
  // the deletion unchecked.
  if (!parsed) return null;
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
  // One sweep over the range: `p` and its image `q` only grow, so every
  // question below is answered from a cursor, not a search (a hard-wrapped
  // paragraph stays as cheap per character as one line).
  const isDeleted = cover(deleted);
  const isMark = cover(lines.marks);
  const isFreed = cover(
    shown
      ? []
      : targeted.map((pair) => ({ from: pair.open.to, to: pair.close.from })),
  );
  const isShown = cover(shown ?? []);
  const isMarkAfter = cover(next.marks);
  const was = formats(lines.pairs);
  const now = formats(next.pairs);
  for (let p = from; p < to; p += 1) {
    checkWork.positions += 1;
    if (keepMarks && !isDeleted(p) && isMark(p)) {
      if (!isMarkAfter(changes.mapPos(p, 1))) return false;
      continue;
    }
    if (isDeleted(p) || isMark(p) || isShown(p)) continue;
    if (isBlank(text[p - from] ?? "")) continue;
    if (isFreed(p)) continue;
    const q = changes.mapPos(p, 1);
    if (isMarkAfter(q)) return false;
    if (was(p) !== now(q)) return false;
  }
  return true;
}

/** One repair: the changes it adds, and for a move where the space went. */
interface Repair {
  specs: ChangeSpec[];
  outcome: DeleteOutcome;
  /** The space a move takes (start positions) and where it puts it. */
  moved?: { space: number; insert: number };
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
): Repair[] {
  const doc = start.doc;
  const out: Repair[] = [];
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
          moved: { space: s, insert: pair.open.from },
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
          moved: { space: e, insert: pair.close.to },
        });
      }
    }
  }
  // Two pairs the deletion made touch: exactly one deleted range (they come
  // merged) lies between the first's closer and the second's opener.
  const byClose = new Map<number, Pair[]>();
  const byOpen = new Map<number, Pair[]>();
  for (const pair of free) {
    if (pair.kind === "link") continue;
    byClose.set(pair.close.to, [...(byClose.get(pair.close.to) ?? []), pair]);
    byOpen.set(pair.open.from, [...(byOpen.get(pair.open.from) ?? []), pair]);
  }
  for (const gap of deleted) {
    for (const left of byClose.get(gap.from) ?? []) {
      for (const right of byOpen.get(gap.to) ?? []) {
        if (
          left.kind !== right.kind ||
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
  }
  return out;
}

/** The annotations a rebuilt deletion carries over from the one it replaces. */
export function carried(tr: Transaction): Annotation<unknown>[] {
  const out: Annotation<unknown>[] = [];
  const event = tr.annotation(Transaction.userEvent);
  if (event !== undefined) out.push(Transaction.userEvent.of(event));
  const history = tr.annotation(Transaction.addToHistory);
  if (history !== undefined) out.push(Transaction.addToHistory.of(history));
  const isolate = tr.annotation(isolateHistory);
  if (isolate !== undefined) out.push(isolateHistory.of(isolate));
  return out;
}

/**
 * Where a position of the deletion's own result lands in `changes`: next
 * to the moved space on the side it stood on, when a move took the space
 * the caret touched (so typing shows where it showed), else mapped.
 */
export function placed(
  start: number,
  changes: ChangeSet,
  deleted: readonly Range[],
  moved: Repair["moved"],
): number {
  if (moved) {
    const { space, insert } = moved;
    let before = start <= space;
    for (let p = start; before && p < space; p += 1) {
      if (!inAny(deleted, p)) before = false;
    }
    let after = start >= space + 1;
    for (let p = space + 1; after && p < start; p += 1) {
      if (!inAny(deleted, p)) after = false;
    }
    if (before) return changes.mapPos(insert, -1);
    if (after) return changes.mapPos(insert, 1);
  }
  return changes.mapPos(start, -1);
}

/** `tr` rebuilt with `changes` in place of its own. */
function rebuilt(
  tr: Transaction,
  changes: ChangeSet,
  outcome: DeleteOutcome | null,
  deleted: readonly Range[] = [],
  moved?: Repair["moved"],
): TransactionSpec {
  const back = tr.changes.invertedDesc;
  const selection = tr.newSelection;
  const at = (pos: number) =>
    placed(back.mapPos(pos, -1), changes, deleted, moved);
  return {
    changes,
    selection: EditorSelection.create(
      selection.ranges.map((range) =>
        EditorSelection.range(at(range.anchor), at(range.head)),
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
 * How a deletion was judged (D5): left alone (where the person edits the
 * Markdown itself, D9, D11, D25), exact (with `changes` when emptied pairs
 * went too, D6), repaired, or refused with what the page says.
 */
export type Judgement =
  | { outcome: "unchecked" }
  | { outcome: "exact"; changes: ChangeSet | null }
  | {
      outcome: "moved" | "merged";
      changes: ChangeSet;
      ranges: Range[];
      moved?: Repair["moved"];
    }
  | { outcome: "refused"; said: string };

/** A refusal for want of a parse before the deadline (D35). */
const LOADING: Judgement = { outcome: "refused", said: PAGE_LOADING };

/**
 * The judge (D5), pure: `changes`, a deletion on `state`, judged against the
 * text it leaves, within `deadline` for every parse it reads. Shared by the
 * filter below and by the check at a composition's end (`composition.ts`),
 * so the shown-line rules (D9, D11, D23, D25, D28), the empty-pair drop (D6)
 * and the repair order are one.
 */
export function judgeDeletion(
  state: EditorState,
  changes: ChangeSet,
  deadline: Deadline,
): Judgement {
  const deleted: Range[] = [];
  changes.iterChanges((fromA, toA) => {
    if (fromA < toA) deleted.push({ from: fromA, to: toA });
  });
  if (deleted.length === 0) return { outcome: "unchecked" };
  const firstPos = Math.min(...deleted.map((d) => d.from));
  const lastPos = Math.max(...deleted.map((d) => d.to));
  // The start state parsed first (D35), to the paragraph the check reads, so
  // each trial reparses only the lines it changed.
  const touched = parsedTo(state, state.doc.lineAt(lastPos).to, deadline);
  if (!touched) return LOADING;
  // A key that takes only a fence's marks is the person editing the
  // Markdown, as with shown marks (D45): the lines below may read anew.
  if (takesOnlyFenceMarks(touched, deleted)) return { outcome: "unchecked" };
  const { first, last: reached } = blockLines(touched, firstPos, lastPos);
  const start = parsedTo(touched, touched.doc.line(reached).to, deadline);
  if (!start) return LOADING;
  const { last } = blockLines(start, firstPos, lastPos);
  const lines = linesOf(start, first, last);
  // A line with no pair can still gain one (`q * a* r` without its space
  // reads italic): where marks are hidden that is checked too (D10). Where
  // they show, such a line is left to today's keys (D11). The lines asked
  // about are those the key touched, not the whole paragraph read: a pair
  // counts when it has a mark on one of them or runs across them.
  const touchedFrom = start.doc.lineAt(firstPos).from;
  const touchedTo = start.doc.lineAt(lastPos).to;
  const onTouched = lines.pairs.some(
    (pair) => pair.open.from <= touchedTo && pair.close.to >= touchedFrom,
  );
  let shownTouched = false;
  if (!onTouched) {
    const lo = start.doc.lineAt(firstPos).number;
    const hi = start.doc.lineAt(lastPos).number;
    for (let n = lo; n <= hi && !shownTouched; n += 1) {
      if (!marksHiddenOnLine(start, n)) shownTouched = true;
    }
  }
  // Where a line shows its marks, a key that takes only such marks is the
  // person editing the Markdown itself (today's keys): the pairs after it
  // may pair anew, also where a partner is on a line whose marks are hidden
  // (D9, D25). A key that takes a normal character too is judged (D28).
  if (takesOnlyShownMarks(start, lines, deleted)) {
    return { outcome: "unchecked" };
  }
  const asked = merged(deleted);
  if (shownTouched) {
    // Today's keys, for the formats of the lines that show their marks. A
    // pair a key that takes no mark makes or breaks must leave every line
    // whose marks are hidden as it read (D23): the person sees no mark
    // there.
    const shown: Range[] = [];
    for (let n = first; n <= last; n += 1) {
      if (marksHiddenOnLine(start, n)) continue;
      const line = start.doc.line(n);
      shown.push({ from: line.from, to: line.to + 1 });
    }
    if (shown.length === last - first + 1) return { outcome: "unchecked" };
    const unseen = readsSame(
      start,
      lines,
      first,
      last,
      changes,
      asked,
      start.update({ changes, filter: false }).state,
      deadline,
      shown,
    );
    if (unseen === null) return LOADING;
    return unseen
      ? { outcome: "unchecked" }
      : { outcome: "refused", said: CANNOT_JOIN };
  }
  const ranges = withEmptied(start, lines, asked);
  const grown =
    ranges.length !== asked.length ||
    ranges.some((r, i) => r.from !== asked[i]?.from || r.to !== asked[i]?.to);
  const exact = grown ? start.changes(ranges) : changes;
  const exactState = start.update({ changes: exact, filter: false }).state;
  const candidates = repairs(start, lines, ranges);
  // Two pairs of one kind the deletion made touch are always written as one
  // (`**a****b**` reads as bold, but nobody writes it that way).
  const merges = candidates.filter((c) => c.outcome === "merged");
  const ordered = [
    ...merges,
    ...candidates.filter((c) => c.outcome !== "merged"),
  ];
  // Whether any read ran out of time: then a refusal says the page loads.
  let timedOut = false;
  // The exact deletion is read at most once a press. A read the parse
  // could not finish is not kept: the next one parses further.
  let exactReads: boolean | null = null;
  const exactIsSame = () => {
    exactReads ??= readsSame(
      start,
      lines,
      first,
      last,
      exact,
      ranges,
      exactState,
      deadline,
    );
    if (exactReads === null) timedOut = true;
    return exactReads === true;
  };
  const asIs: Judgement = { outcome: "exact", changes: grown ? exact : null };
  if (merges.length === 0 && exactIsSame()) return asIs;
  for (const repair of ordered) {
    const repaired = start.changes([
      ...ranges.map(({ from, to }) => ({ from, to })),
      ...repair.specs,
    ]);
    const trial = start.update({ changes: repaired, filter: false }).state;
    const reads = readsSame(
      start,
      lines,
      first,
      last,
      repaired,
      ranges,
      trial,
      deadline,
    );
    if (reads === null) timedOut = true;
    if (reads === true) {
      // A move is a repair of the exact deletion: it never stands in for an
      // exact read that ran out of time (D52). A merge always wins.
      if (repair.outcome !== "merged") {
        exactIsSame();
        if (exactReads === null) return LOADING;
      }
      return {
        outcome: repair.outcome as "moved" | "merged",
        changes: repaired,
        ranges,
        ...(repair.moved ? { moved: repair.moved } : {}),
      };
    }
  }
  if (exactIsSame()) return asIs;
  return timedOut ? LOADING : { outcome: "refused", said: CANNOT_JOIN };
}

/** The bound of one press of `event` (D35). */
function budgetOf(event: string): number {
  return event === "delete.selection" ||
    event === "delete.cut" ||
    event === "delete.picture"
    ? PARSE_BUDGET.gesture
    : PARSE_BUDGET.key;
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
    tr.annotation(Transaction.remote) !== undefined ||
    tr.annotation(judgedElsewhere) === true
  ) {
    return tr;
  }
  const start = tr.startState;
  // An IME composition is the browser's to finish: a repair written beside
  // it could end it (the caret normalizer leaves it alone too). It is
  // judged when it ends (`composition.ts`).
  if (composing(start) || tr.isUserEvent("input.type.compose")) return tr;
  let inserts = false;
  tr.changes.iterChanges((_fromA, _toA, _fromB, _toB, inserted) => {
    if (inserted.length > 0) inserts = true;
  });
  // A replacement is not a deletion to judge.
  if (inserts) return tr;
  const judged = judgeDeletion(start, tr.changes, deadlineIn(budgetOf(event)));
  switch (judged.outcome) {
    case "unchecked":
      return tr;
    case "exact":
      return judged.changes ? rebuilt(tr, judged.changes, null) : tr;
    case "refused":
      return refusal(start, judged.said);
    default:
      return rebuilt(
        tr,
        judged.changes,
        judged.outcome,
        judged.ranges,
        judged.moved,
      );
  }
});

/** Whether every character `deleted` takes is a fence's backtick or tilde. */
function takesOnlyFenceMarks(
  state: EditorState,
  deleted: readonly Range[],
): boolean {
  const marks: Range[] = [];
  for (const range of deleted) {
    walkTree(syntaxTree(state), range.from, range.to, (node) => {
      if (node.name === "CodeMark" && node.node.parent?.name === "FencedCode") {
        marks.push({ from: node.from, to: node.to });
      }
      return undefined;
    });
  }
  if (marks.length === 0) return false;
  const isMark = cover(marks);
  for (const range of merged(deleted)) {
    for (let p = range.from; p < range.to; p += 1) {
      if (!isMark(p)) return false;
    }
  }
  return true;
}

/**
 * Whether every character `deleted` takes is a mark on a line that shows
 * its marks (D25, D28).
 */
function takesOnlyShownMarks(
  start: EditorState,
  lines: Lines,
  deleted: readonly Range[],
): boolean {
  const shown = lines.marks.filter(
    (mark) =>
      touches(deleted, mark) &&
      !marksHiddenOnLine(start, start.doc.lineAt(mark.from).number),
  );
  if (shown.length === 0) return false;
  const isShownMark = cover(shown);
  for (const range of merged(deleted)) {
    for (let p = range.from; p < range.to; p += 1) {
      checkWork.positions += 1;
      if (!isShownMark(p)) return false;
    }
  }
  return true;
}

/** A refused deletion: the bytes stay and the page says so. */
function refusal(start: EditorState, said = CANNOT_JOIN): TransactionSpec {
  return {
    selection: start.selection,
    effects: [EditorView.announce.of(said), deletionOutcome.of("refused")],
  };
}
