/**
 * An insert the person makes beside hidden marks is checked like a deletion
 * (D5, I2): a paste, a drop, a completion, typing over a selection (D17
 * treats it as a delete with text put in) and Enter, which at the end of a
 * bold line would otherwise split its closing marks onto the next line. `formatInput` handles typing at a
 * caret; CodeMirror's paste never reaches an input handler, so this filter
 * is the place for the rest.
 *
 * The insert goes to the first of these places whose re-read keeps every
 * pair it did not delete, as rule 4 of the typing rules does:
 *
 * 1. where it lands;
 * 2. with the whitespace at its edges moved outside the marks it would sit
 *    inside (`**bold` + `word ` writes `**boldword** `, a `**` after a space
 *    closes nothing);
 * 3. at the caret's drawn place (inside a closing run, continuing its
 *    format), then on the far side of the run.
 *
 * A pair the insert leaves with nothing visible inside loses its marks
 * (D6). Where no place keeps the pairs, the insert is written as it landed:
 * a paste is never refused, and neither is typing. Only lines whose marks
 * are hidden are judged; where they show, the person sees what they write.
 */

import type {
  ChangeSet,
  ChangeSpec,
  EditorState,
  Extension,
} from "@codemirror/state";
import {
  EditorSelection,
  EditorState as State,
  StateEffect,
  Transaction,
} from "@codemirror/state";

import { composing } from "./caret";
import { canonical } from "./caret";
import type { Lines, Range } from "./deleteCheck";
import { blockLines, carried, linesOf, merged, readsSame } from "./deleteCheck";
import { droppable, judgedElsewhere } from "./deleting";
import { marksOnLine, runAt } from "./hiddenMarks";
import { marksHiddenOnLine } from "./mode";
import { PARSE_BUDGET, deadlineIn, parsedTo } from "./parse";
import type { Deadline } from "./parse";

/** The inserts judged here; typing only when it replaces a selection. */
const CHECKED = new Set([
  "input.paste",
  "input.drop",
  "move.drop",
  "input.complete",
  "input.type",
  // Enter: a line break put in (CodeMirror's newline commands, `listEnter`).
  "input",
]);

/** One place to try: the pieces the text goes in as, in document order. */
interface Place {
  pieces: { at: number; insert: string }[];
}

function isSpace(char: string): boolean {
  return /\s/.test(char);
}

/**
 * The block marks (bullet, number, task box, quote mark) each line the
 * pieces start begins with in `trial`, as one comparable word.
 */
function blockShape(
  trial: EditorState,
  changes: ChangeSet,
  pieces: Place["pieces"],
  deadline: Deadline,
): string {
  const starts: number[] = [];
  for (const piece of pieces) {
    const at = changes.mapPos(piece.at, -1);
    for (const found of piece.insert.matchAll(/\r?\n/g)) {
      starts.push(
        Math.min(at + (found.index ?? 0) + found[0].length, trial.doc.length),
      );
    }
  }
  const last = Math.max(0, ...starts);
  const parsed = parsedTo(trial, trial.doc.lineAt(last).to, deadline) ?? trial;
  const out: string[] = [];
  for (const pos of starts) {
    const line = parsed.doc.lineAt(pos);
    const kinds: string[] = [];
    let end = line.from;
    for (const mark of marksOnLine(parsed, line.number, { always: true })) {
      if (mark.side !== "prefix" || mark.from > end) break;
      kinds.push(`${mark.kind}:${parsed.sliceDoc(mark.from, mark.to)}`);
      end = mark.to;
    }
    out.push(kinds.join(" "));
  }
  return out.join("|");
}

/** The places to try for `text` put in at `at` (after `deleted` went). */
function places(state: EditorState, at: number, text: string): Place[] {
  const out: Place[] = [{ pieces: [{ at, insert: text }] }];
  const run = runAt(state, at);
  if (!run) {
    return out;
  }
  if (/[\r\n]/.test(text)) {
    // Text that breaks the line is never split across a run (D55): it goes
    // whole to the run's far side, after a closing run, or before the
    // formats an opening run holds (after a line's block marks). A run that
    // closes one format and opens the next (`**a***b*`) is tried between
    // the two first, so each keeps its pair (D61).
    const closing = run.marks.filter((mark) => mark.side === "close");
    if (closing.length > 0 && run.marks.some((mark) => mark.side === "open")) {
      const between = Math.max(...closing.map((mark) => mark.to));
      if (between !== at && between !== run.to) {
        out.push({ pieces: [{ at: between, insert: text }] });
      }
    }
    if (closing.length > 0 && run.to !== at) {
      out.push({ pieces: [{ at: run.to, insert: text }] });
    }
    const format = run.marks.find((mark) => mark.side === "open");
    if (
      run.marks.every((mark) => mark.side !== "close") &&
      format &&
      format.from !== at
    ) {
      out.push({ pieces: [{ at: format.from, insert: text }] });
    }
    return out;
  }
  let lead = 0;
  while (lead < text.length && isSpace(text[lead] ?? "")) lead += 1;
  let trail = text.length;
  while (trail > lead && isSpace(text[trail - 1] ?? "")) trail -= 1;
  const opens = run.marks.every((mark) => mark.side !== "close");
  const closes = run.marks.every((mark) => mark.side === "close");
  const before = text.slice(0, lead);
  const core = text.slice(lead, trail);
  const after = text.slice(trail);
  // Inside an opening run (its end): leading whitespace goes before it.
  // Inside a closing run (its start): trailing whitespace goes after it.
  // Text that is all whitespace goes out the way the run faces.
  if (at === run.to && opens && before !== "") {
    out.push({
      pieces: [
        { at: run.from, insert: before },
        ...(core + after === "" ? [] : [{ at, insert: core + after }]),
      ],
    });
  } else if (at === run.from && closes && (after !== "" || core === "")) {
    const tail = core === "" ? before : after;
    out.push({
      pieces: [
        ...(core === "" ? [] : [{ at, insert: before + core }]),
        { at: run.to, insert: tail },
      ],
    });
  }
  for (const other of [canonical(state, at), run.to, run.from]) {
    if (other !== at) out.push({ pieces: [{ at: other, insert: text }] });
  }
  return out;
}

/**
 * The marks of every pair `deleted` leaves with nothing visible inside and
 * no piece of the insert between its marks (D6), images aside (D40).
 */
function emptied(
  state: EditorState,
  lines: Lines,
  deleted: readonly Range[],
  pieces: Place["pieces"],
): Range[] {
  const out: Range[] = [];
  const inside = (p: number) => deleted.some((r) => r.from <= p && p < r.to);
  const isMark = (p: number) =>
    lines.marks.some((mark) => mark.from <= p && p < mark.to);
  for (const pair of lines.pairs) {
    if (inside(pair.open.from) || inside(pair.close.from)) continue;
    if (
      pieces.some(
        (piece) => pair.open.to <= piece.at && piece.at <= pair.close.from,
      )
    ) {
      continue;
    }
    if (
      !droppable(state, pair.open, pair.open.to, pair.close.from, lines.marks)
    ) {
      continue;
    }
    let seen = false;
    let all = true;
    for (let p = pair.open.to; all && p < pair.close.from; p += 1) {
      if (isMark(p)) continue;
      seen = true;
      if (!inside(p)) all = false;
    }
    if (seen && all) out.push(pair.open, pair.close);
  }
  return out;
}

/** Where an insert goes instead of where it landed, and its text's end. */
export interface InsertJudgement {
  changes: ChangeSet;
  /** Where the inserted text ends in the result. */
  from: number;
  end: number;
}

/**
 * Where `text`, put in over `[from, to)` on `begin` beside the pure
 * `deletions`, goes so that every pair keeps reading as it did: the first of
 * the places above that keeps them, or null to write it as it landed (it
 * keeps them already, no place does, or the parse ran out of `deadline`).
 * Shared by the filter below and by a composition's end (`composition.ts`).
 */
export function judgeInsert(
  begin: EditorState,
  deletions: readonly Range[],
  insert: { from: number; to: number; text: string },
  deadline: Deadline,
): InsertJudgement | null {
  const start = parsedTo(begin, begin.doc.lineAt(insert.to).to, deadline);
  // Short of time: written as it landed, never refused.
  if (!start) return null;
  const line = start.doc.lineAt(insert.from).number;
  if (!marksHiddenOnLine(start, line)) return null;
  const { first, last } = blockLines(start, insert.from, insert.to);
  for (let n = first; n <= last; n += 1) {
    if (!marksHiddenOnLine(start, n)) return null;
  }
  const lines = linesOf(start, first, last);
  if (lines.marks.length === 0) return null;
  const removed: Range[] = [
    ...deletions,
    ...(insert.from < insert.to ? [{ from: insert.from, to: insert.to }] : []),
  ];
  const breaks = /[\r\n]/.test(insert.text);
  // The block marks each new line starts with where the text lands as put:
  // a place elsewhere must start them the same (D55), or Enter in a list
  // item would make a line that is no item.
  let shape: string | null = null;
  for (const [index, place] of places(
    start,
    insert.from,
    insert.text,
  ).entries()) {
    const deleted = merged([
      ...removed,
      ...emptied(start, lines, removed, place.pieces),
    ]);
    const specs: ChangeSpec[] = [
      ...deleted.map(({ from, to }) => ({ from, to })),
      ...place.pieces.map(({ at, insert: text }) => ({
        from: at,
        insert: text,
      })),
    ];
    const changes = start.changes(specs);
    const trial = start.update({ changes, filter: false }).state;
    const reads = readsSame(
      start,
      lines,
      first,
      last,
      changes,
      deleted,
      trial,
      deadline,
      undefined,
      true,
    );
    if (reads === null) return null;
    if (breaks) {
      const read = blockShape(trial, changes, place.pieces, deadline);
      if (index === 0) shape = read;
      else if (read !== shape) continue;
    }
    if (!reads) continue;
    // The text as it landed, keeping its pairs: nothing to change.
    if (index === 0 && deleted.length === merged(removed).length) return null;
    const lastPiece = place.pieces.at(-1);
    const firstPiece = place.pieces[0];
    if (!lastPiece || !firstPiece) return null;
    return {
      changes,
      from: changes.mapPos(firstPiece.at, -1),
      end: changes.mapPos(lastPiece.at, -1) + lastPiece.insert.length,
    };
  }
  return null;
}

export const checkedInsert: Extension = State.transactionFilter.of((tr) => {
  const event = tr.annotation(Transaction.userEvent);
  if (
    !tr.docChanged ||
    event === undefined ||
    !CHECKED.has(event) ||
    tr.annotation(Transaction.remote) !== undefined ||
    tr.annotation(judgedElsewhere) === true ||
    tr.isUserEvent("input.type.compose") ||
    composing(tr.startState)
  ) {
    return tr;
  }
  // One insert, with any pure deletions beside it (a drag-move's source).
  const deletions: Range[] = [];
  let insert: { from: number; to: number; text: string } | null = null;
  let inserts = 0;
  tr.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
    if (inserted.length > 0) {
      inserts += 1;
      insert = { from: fromA, to: toA, text: inserted.toString() };
    } else if (fromA < toA) {
      deletions.push({ from: fromA, to: toA });
    }
  });
  const found = insert as { from: number; to: number; text: string } | null;
  if (inserts !== 1 || !found) return tr;
  // Typing at a caret is `formatInput`'s; here only over a selection.
  if (event === "input.type" && found.from === found.to) return tr;
  // A bare `input` is judged only when it breaks the line (Enter).
  if (event === "input" && !/[\r\n]/.test(found.text)) return tr;
  const judged = judgeInsert(
    tr.startState,
    deletions,
    found,
    deadlineIn(
      event === "input.type" ? PARSE_BUDGET.key : PARSE_BUDGET.gesture,
    ),
  );
  if (!judged) return tr;
  const selection = tr.newSelection.main.empty
    ? EditorSelection.cursor(judged.end)
    : EditorSelection.range(judged.from, judged.end);
  return {
    changes: judged.changes,
    selection,
    annotations: carried(tr),
    effects: StateEffect.mapEffects(
      StateEffect.mapEffects(tr.effects, tr.changes.invertedDesc),
      judged.changes.desc,
    ),
    scrollIntoView: tr.scrollIntoView,
  };
});
