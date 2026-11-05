/**
 * Bold, Italic, Strikethrough and Inline code where the marks are hidden.
 *
 * A format is a set of visible characters, so the commands change which
 * visible characters have it and write whatever marks that takes:
 *
 * - On a selection (hidden and show mode) the visible characters picked
 *   get the format, or lose it when every one of them has it already. Marks of the
 *   format inside the selection go, a pair the selection starts or ends in is
 *   merged with, and a mark right beside it is moved rather than written
 *   twice. The changed line is parsed again before anything is sent: when
 *   the parser would not read the result as asked, nothing is written and
 *   the person is told so.
 * - With no selection nothing is written. The format waits at the caret
 *   (`pendingFormat`) and the next typed text gets it, in hidden and show
 *   mode alike. Raw and a table cell (which has no hidden marks yet) keep
 *   the source view's wrap, `toggleInline`. A shown mark inside a selection
 *   is read as the mark it is, so a pair the selection falls inside is split
 *   rather than broken by a wrap.
 * - Typing at the edge of a format continues it (`formatInput`): a letter at
 *   the end of bold is bold, a space goes after the marks and the next
 *   letter is bold again, and a closing mark typed by hand ends the format,
 *   so `**word** next` leaves `next` plain.
 *
 * The positions here are document positions. Every insert is one `input.type`
 * transaction, so it joins the typing around it in one undo step; a format on
 * a selection is a gesture of its own and one undo step, solo and in a room.
 */

import { isolateHistory } from "@codemirror/commands";
import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension } from "@codemirror/state";
import {
  ChangeSet,
  EditorSelection,
  Facet,
  Prec,
  StateEffect,
  StateField,
  Transaction,
} from "@codemirror/state";
import type { ViewUpdate } from "@codemirror/view";
import { EditorView, ViewPlugin, keymap } from "@codemirror/view";

import { linkKey, plainLinkKey } from "../elementEdit";
import { verbTarget } from "../floatingContext";
import { judgeBeforeCommand } from "./commandJudge";
import { isIdLineAt } from "../hiddenIdLines";
import { mainSelectionParked, parkingChanged } from "../parkedSelection";
import { roomUndo } from "../roomUndo";
import { clearOfFoldedFrontmatter, toggleInline } from "../toolbar";
import { canonical, composing, snapRange } from "./caret";
import { partnerAcrossLines } from "./deleting";
import type { Mark, MarkRun } from "./hiddenMarks";
import { marksOnLine, runsOf } from "./hiddenMarks";
import { selectedPicture } from "../pictures/refs";
import { hiddenMarkLayerOn, marksHiddenOnLine, pictureInline } from "./mode";
import { PAGE_LOADING, PARSE_BUDGET, deadlineIn, parsedTo } from "./parse";
import type { Deadline } from "./parse";

export type Format = "strong" | "em" | "strike" | "code";

export const FORMAT_MARKER: Record<Format, string> = {
  strong: "**",
  em: "*",
  strike: "~~",
  code: "`",
};

/** No format pressed: what a screen holds before its buffer reports any. */
export const NO_FORMATS: ReadonlySet<Format> = new Set();

/** In the order the buttons stand. */
const FORMATS: readonly Format[] = ["strong", "em", "strike", "code"];

/** The formats a letter typed at the inner end of their closing mark takes. */
const CONTINUING = new Set<Format>(["strong", "em", "strike"]);

/** What a refused format says. */
export const CANNOT_FORMAT = "This text cannot be formatted here.";

/**
 * Asked first by every format command: a sentence the buffer says instead,
 * when it cannot take a format yet, or null. A table cell whose row is not
 * read gives its own (D13), before the command reads that short tree.
 */
export const formatGate = Facet.define<(state: EditorState) => string | null>();

function isFormat(kind: string): kind is Format {
  return (FORMATS as readonly string[]).includes(kind);
}

function writable(state: EditorState): boolean {
  return state.facet(EditorView.editable) && !state.readOnly;
}

function isSpace(char: string): boolean {
  return char === " " || char === "\t";
}

interface Pair {
  format: Format;
  open: Mark;
  close: Mark;
}

/** One line's marks as the format commands read them, shown or hidden. */
interface LineModel {
  number: number;
  from: number;
  to: number;
  marks: Mark[];
  runs: MarkRun[];
  pairs: Pair[];
}

function lineModel(state: EditorState, number: number): LineModel {
  const line = state.doc.line(number);
  const marks = marksOnLine(state, number, { always: true });
  const pairs: Pair[] = [];
  for (const mark of marks) {
    if (!isFormat(mark.kind)) {
      continue;
    }
    if (mark.partner !== null) {
      const close = marks[mark.partner];
      if (mark.side === "open" && close) {
        pairs.push({ format: mark.kind, open: mark, close });
      }
      continue;
    }
    // A pair across a soft line break: its other mark is on another line,
    // found from the syntax node the two belong to.
    const far = partnerAcrossLines(state, mark);
    if (!far) {
      continue;
    }
    const other: Mark = {
      from: far.from,
      to: far.to,
      kind: mark.kind,
      side: mark.side === "open" ? "close" : "open",
      partner: null,
    };
    pairs.push(
      mark.side === "open"
        ? { format: mark.kind, open: mark, close: other }
        : { format: mark.kind, open: other, close: mark },
    );
  }
  return {
    number,
    from: line.from,
    to: line.to,
    marks,
    runs: runsOf(marks),
    pairs,
  };
}

function inMark(model: LineModel, pos: number): boolean {
  return model.marks.some((mark) => mark.from <= pos && pos < mark.to);
}

/** The visible characters of `from`..`to` on the model's line. */
function visibleIn(model: LineModel, from: number, to: number): number[] {
  const out: number[] = [];
  for (
    let pos = Math.max(from, model.from);
    pos < Math.min(to, model.to);
    pos++
  ) {
    if (!inMark(model, pos)) {
      out.push(pos);
    }
  }
  return out;
}

/** The formats of the visible character at `pos`. */
function formatsOfChar(model: LineModel, pos: number): Set<Format> {
  const out = new Set<Format>();
  for (const pair of model.pairs) {
    if (pair.open.to <= pos && pos < pair.close.from) {
      out.add(pair.format);
    }
  }
  return out;
}

/** The formats a character typed at `pos` lands in, with nothing waiting. */
function formatsOfPlace(model: LineModel, pos: number): Set<Format> {
  const out = new Set<Format>();
  for (const pair of model.pairs) {
    if (pair.open.to <= pos && pos <= pair.close.from) {
      out.add(pair.format);
    }
  }
  return out;
}

/** The innermost pair of `format` that a caret at `pos` is in. */
function pairAround(
  model: LineModel,
  format: Format,
  pos: number,
): Pair | null {
  let found: Pair | null = null;
  for (const pair of model.pairs) {
    if (
      pair.format === format &&
      pair.open.to <= pos &&
      pos <= pair.close.from &&
      (found === null || pair.open.from > found.open.from)
    ) {
      found = pair;
    }
  }
  return found;
}

/** The formats of the visible character at `pos`, shown marks or hidden. */
export function charFormats(state: EditorState, pos: number): Set<Format> {
  const model = lineModel(state, state.doc.lineAt(pos).number);
  return inMark(model, pos) ? new Set() : formatsOfChar(model, pos);
}

/**
 * The formats a character typed at `pos` gets: those of the pairs its drawn
 * caret place is in, with what waits there (`pendingFormat`) put on or
 * taken off.
 */
export function formatsAt(state: EditorState, pos: number): Set<Format> {
  const place = canonical(state, pos);
  const out = formatsOfPlace(
    lineModel(state, state.doc.lineAt(place).number),
    place,
  );
  const pending = state.field(pendingFormat, false) ?? null;
  if (pending && pending.pos === place) {
    for (const format of pending.on) out.add(format);
    for (const format of pending.off) out.delete(format);
  }
  return out;
}

/**
 * What the format buttons show as pressed: the formats at the caret, or the
 * formats every visible character of the selection has.
 */
export function formatsOfSelection(state: EditorState): Set<Format> {
  const range = state.selection.main;
  if (range.empty) {
    return formatsAt(state, range.head);
  }
  const out = new Set<Format>(FORMATS);
  let seen = false;
  const first = state.doc.lineAt(range.from).number;
  const last = state.doc.lineAt(range.to).number;
  for (let number = first; number <= last; number++) {
    const model = lineModel(state, number);
    for (const pos of visibleIn(model, range.from, range.to)) {
      seen = true;
      const formats = formatsOfChar(model, pos);
      for (const format of FORMATS) {
        if (!formats.has(format)) out.delete(format);
      }
      // The set only shrinks: a plain character ends the look.
      if (out.size === 0) {
        return out;
      }
    }
  }
  return seen ? out : new Set();
}

// The format waiting for typing.

export interface Pending {
  /** The canonical caret it waits at. */
  pos: number;
  /** Formats the next typed text gets. */
  on: Format[];
  /** Formats the next typed text does not get, though the caret is in them. */
  off: Format[];
}

/** Sets what waits at the caret; a composition's end sets it too. */
export const setPending = StateEffect.define<Pending | null>();

/**
 * What Bold or Italic with no selection left at the caret. It maps through
 * every change, and goes when the caret leaves its place, on any local edit
 * that is not its own insert, on blur and when the editor is reconfigured
 * (the marks setting, Raw). A peer's edit has no user event and only maps it.
 */
export const pendingFormat = StateField.define<Pending | null>({
  create: () => null,
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setPending)) {
        return effect.value;
      }
    }
    if (value === null || tr.reconfigured) {
      return null;
    }
    // An IME composition types the text the format waits for: the format
    // stays where it was, and the composition's end writes it (I3, D34).
    if (tr.isUserEvent("input.type.compose") || composing(tr.startState)) {
      const pos = tr.changes.mapPos(value.pos, -1);
      return pos === value.pos ? value : { ...value, pos };
    }
    if (tr.docChanged && tr.annotation(Transaction.userEvent) !== undefined) {
      return null;
    }
    if (!tr.docChanged && tr.selection === undefined) {
      return value;
    }
    const pos = tr.changes.mapPos(value.pos, -1);
    const { selection } = tr.state;
    if (
      selection.ranges.length !== 1 ||
      !selection.main.empty ||
      canonical(tr.state, selection.main.head) !== pos
    ) {
      return null;
    }
    return pos === value.pos ? value : { ...value, pos };
  },
});

const forgetOnBlur = EditorView.focusChangeEffect.of((state, focusing) =>
  !focusing && (state.field(pendingFormat, false) ?? null) !== null
    ? setPending.of(null)
    : null,
);

function togglePending(view: EditorView, format: Format): void {
  const { state } = view;
  const place = canonical(state, state.selection.main.head);
  const model = lineModel(state, state.doc.lineAt(place).number);
  const held = state.field(pendingFormat, false) ?? null;
  const now: Pending =
    held && held.pos === place
      ? { pos: place, on: [...held.on], off: [...held.off] }
      : { pos: place, on: [], off: [] };
  if (now.on.includes(format)) {
    now.on = now.on.filter((each) => each !== format);
  } else if (now.off.includes(format)) {
    now.off = now.off.filter((each) => each !== format);
  } else if (formatsOfPlace(model, place).has(format)) {
    now.off.push(format);
  } else {
    now.on.push(format);
  }
  const next = now.on.length === 0 && now.off.length === 0 ? null : now;
  view.dispatch({ effects: setPending.of(next) });
}

// A format on a selection.

interface Piece {
  model: LineModel;
  from: number;
  to: number;
}

interface Change {
  from: number;
  to: number;
  insert: string;
}

/** `from`..`to` without the marks and the spaces at either end. */
function trimmed(
  state: EditorState,
  model: LineModel,
  from: number,
  to: number,
): [number, number] {
  let a = from;
  let b = to;
  while (a < b) {
    const run = model.runs.find((each) => each.from <= a && a < each.to);
    if (run) {
      a = run.to;
    } else if (isSpace(state.sliceDoc(a, a + 1))) {
      a += 1;
    } else {
      break;
    }
  }
  while (b > a) {
    const run = model.runs.find((each) => each.from < b && b <= each.to);
    if (run) {
      b = run.from;
    } else if (isSpace(state.sliceDoc(b - 1, b))) {
      b -= 1;
    } else {
      break;
    }
  }
  return [a, b];
}

/** Whether a visible character lies in `from`..`to`, on any line. */
function visibleBetween(state: EditorState, from: number, to: number): boolean {
  if (from >= to) {
    return false;
  }
  const first = state.doc.lineAt(from).number;
  const last = state.doc.lineAt(to).number;
  for (let number = first; number <= last; number++) {
    if (visibleIn(lineModel(state, number), from, to).length > 0) {
      return true;
    }
  }
  return false;
}

function isBlank(char: string): boolean {
  return char === " " || char === "\t" || char === "\n" || char === "\r";
}

/** What one piece of the selection changes. */
interface PieceEdit {
  changes: Change[];
  /**
   * The spaces outside the piece that change format with it: a closing mark
   * written before them and an opening after them, so neither stands next
   * to a space, where it would not close or open.
   */
  picked: { from: number; to: number }[];
  /** The first and last position the edit's pairs reach. */
  reach: { from: number; to: number };
}

/** The marks to write and delete for one line's piece of the selection. */
function pieceChanges(
  state: EditorState,
  piece: Piece,
  format: Format,
  removing: boolean,
): PieceEdit {
  const { model, from, to } = piece;
  const marker = FORMAT_MARKER[format];
  const out = new Map<string, Change>();
  const picked: { from: number; to: number }[] = [];
  const reach = { from, to };
  const spread = (pair: Pair) => {
    reach.from = Math.min(reach.from, pair.open.from);
    reach.to = Math.max(reach.to, pair.close.to);
  };
  const drop = (mark: Mark) => {
    out.set(`drop ${String(mark.from)}`, {
      from: mark.from,
      to: mark.to,
      insert: "",
    });
  };
  const write = (at: number) => {
    out.set(`write ${String(at)}`, { from: at, to: at, insert: marker });
  };
  const pairs = model.pairs.filter((pair) => pair.format === format);
  // Marks of the format inside the piece go either way.
  for (const pair of pairs) {
    for (const mark of [pair.open, pair.close]) {
      if (mark.from >= from && mark.to <= to) {
        drop(mark);
        spread(pair);
      }
    }
  }
  const start = pairAround(model, format, from);
  const end = pairAround(model, format, to);
  if (start) spread(start);
  if (end) spread(end);
  if (removing) {
    // Inside a pair at an edge: close it there, before the spaces in front
    // of the piece, or, where that leaves an empty pair, take its mark away.
    if (start) {
      let at = from;
      while (at > start.open.to && isBlank(state.sliceDoc(at - 1, at))) {
        at -= 1;
      }
      if (visibleBetween(state, start.open.to, at)) {
        write(at);
        picked.push({ from: at, to: from });
      } else {
        drop(start.open);
      }
    }
    if (end) {
      let at = to;
      while (at < end.close.from && isBlank(state.sliceDoc(at, at + 1))) {
        at += 1;
      }
      if (visibleBetween(state, at, end.close.from)) {
        write(at);
        picked.push({ from: to, to: at });
      } else {
        drop(end.close);
      }
    }
  } else {
    // Inside a pair at an edge: the piece joins it. Right beside a mark of
    // the format: the mark moves, rather than a second pair touching it.
    if (!start) {
      const touching = pairs.find((pair) => pair.close.to === from);
      if (touching) {
        drop(touching.close);
        spread(touching);
      } else {
        write(from);
      }
    }
    if (!end) {
      const touching = pairs.find((pair) => pair.open.from === to);
      if (touching) {
        drop(touching.open);
        spread(touching);
      } else {
        write(to);
      }
    }
  }
  return { changes: [...out.values()], picked, reach };
}

function sameFormats(a: ReadonlySet<Format>, b: ReadonlySet<Format>): boolean {
  return a.size === b.size && [...a].every((format) => b.has(format));
}

/**
 * Whether the lines from `reach.from`'s to `reach.to`'s, parsed again with
 * `changes`, read as asked: the same visible characters, the `picked`
 * characters with `format` given or taken, every other character with the
 * formats it had. Null when the result could not be parsed that far before
 * `deadline`.
 */
function readsAsAsked(
  state: EditorState,
  changes: ChangeSet,
  picked: readonly { from: number; to: number }[],
  reach: { from: number; to: number },
  format: Format,
  removing: boolean,
  deadline: Deadline,
): boolean | null {
  const trial = parsedTo(
    state.update({ changes, filter: false }).state,
    changes.mapPos(state.doc.lineAt(reach.to).to, 1),
    deadline,
  );
  if (!trial) {
    return null;
  }
  const first = state.doc.lineAt(reach.from).number;
  const last = state.doc.lineAt(reach.to).number;
  for (let number = first; number <= last; number++) {
    const old = lineModel(state, number);
    const next = lineModel(trial, number);
    const before = visibleIn(old, old.from, old.to);
    const after = visibleIn(next, next.from, next.to);
    if (before.length !== after.length) {
      return false;
    }
    for (const [index, pos] of before.entries()) {
      const moved = after[index];
      if (moved === undefined || changes.mapPos(pos, 1) !== moved) {
        return false;
      }
      const expected = formatsOfChar(old, pos);
      if (picked.some((range) => range.from <= pos && pos < range.to)) {
        if (removing) expected.delete(format);
        else expected.add(format);
      }
      if (!sameFormats(expected, formatsOfChar(next, moved))) {
        return false;
      }
    }
  }
  return true;
}

function refuse(view: EditorView, said = CANNOT_FORMAT): void {
  view.dispatch({ effects: EditorView.announce.of(said) });
}

function formatSelection(view: EditorView, format: Format): void {
  // One bound for the whole press (D35): the start state is parsed first, to
  // the selection's last line, so the trial reparses only what it changed.
  const deadline = deadlineIn(PARSE_BUDGET.command);
  const state = parsedTo(
    view.state,
    view.state.doc.lineAt(view.state.selection.main.to).to,
    deadline,
  );
  if (!state) {
    refuse(view, PAGE_LOADING);
    return;
  }
  const range = snapRange(state, state.selection.main);
  const pieces: Piece[] = [];
  const first = state.doc.lineAt(range.from).number;
  const last = state.doc.lineAt(range.to).number;
  for (let number = first; number <= last; number++) {
    // A Reflection's hidden id line is no text to format: it stays as it is.
    if (isIdLineAt(state, number)) {
      continue;
    }
    const model = lineModel(state, number);
    const [from, to] = trimmed(
      state,
      model,
      Math.max(range.from, model.from),
      Math.min(range.to, model.to),
    );
    if (from < to) {
      pieces.push({ model, from, to });
    }
  }
  if (pieces.length === 0) {
    return;
  }
  // A code span holds no other mark.
  if (
    format === "code" &&
    pieces.some((piece) =>
      piece.model.marks.some(
        (mark) =>
          mark.kind !== "code" && mark.from < piece.to && mark.to > piece.from,
      ),
    )
  ) {
    refuse(view);
    return;
  }
  const removing = pieces.every((piece) =>
    visibleIn(piece.model, piece.from, piece.to).every((pos) =>
      formatsOfChar(piece.model, pos).has(format),
    ),
  );
  const edits = pieces.map((piece) =>
    pieceChanges(state, piece, format, removing),
  );
  const changes = ChangeSet.of(
    edits.flatMap((edit) => edit.changes),
    state.doc.length,
    state.lineBreak,
  );
  const picked = [
    ...pieces.map(({ from, to }) => ({ from, to })),
    ...edits.flatMap((edit) => edit.picked),
  ];
  const reach = {
    from: Math.min(...edits.map((edit) => edit.reach.from)),
    to: Math.max(...edits.map((edit) => edit.reach.to)),
  };
  const reads = readsAsAsked(
    state,
    changes,
    picked,
    reach,
    format,
    removing,
    deadline,
  );
  if (reads !== true) {
    refuse(view, reads === null ? PAGE_LOADING : CANNOT_FORMAT);
    return;
  }
  const from = changes.mapPos(range.from, 1);
  const to = changes.mapPos(range.to, -1);
  const room = state.facet(roomUndo);
  // A gesture, not typing: one undo step of its own, solo and in a room.
  room?.stopCapturing();
  view.dispatch({
    changes,
    selection:
      range.head < range.anchor
        ? EditorSelection.range(to, from)
        : EditorSelection.range(from, to),
    userEvent: "input",
    annotations: isolateHistory.of("full"),
    scrollIntoView: true,
  });
  room?.stopCapturing();
}

/**
 * Bold, Italic, Strikethrough or Inline code, from a button or Mod-b and
 * Mod-i. With the hidden-mark layer, in hidden and show mode: a selection
 * changes the format of its visible characters, and no selection makes the
 * format wait for typing. Raw and a buffer without the hidden-mark layer (a
 * table cell) keep `toggleInline`. A page
 * that is not writable takes nothing, and neither does a selection that is
 * exactly one drawn picture.
 */
export function toggleFormat(view: EditorView, format: Format): boolean {
  judgeBeforeCommand(view);
  if (!writable(view.state)) {
    return true;
  }
  for (const gate of view.state.facet(formatGate)) {
    const said = gate(view.state);
    if (said !== null) {
      view.dispatch({ effects: EditorView.announce.of(said) });
      return true;
    }
  }
  // A selection that is exactly one drawn picture has no words to format:
  // its alt text is hidden behind the picture, so a mark written there is a
  // change nobody sees (and asterisks a screen reader reads out). Handled,
  // so no other binding of the key moves the selection off the picture.
  const picture = selectedPicture(view.state);
  if (picture !== null && pictureInline(view.state, picture)) {
    return true;
  }
  const marker = FORMAT_MARKER[format];
  if (!view.state.facet(hiddenMarkLayerOn)) {
    return toggleInline(view, marker);
  }
  if (!clearOfFoldedFrontmatter(view)) {
    return false;
  }
  // Read after the guard: it may have moved the caret out of the block.
  const { state } = view;
  const { selection } = state;
  if (selection.ranges.length !== 1) {
    return toggleInline(view, marker);
  }
  const range = selection.main;
  if (range.empty) {
    togglePending(view, format);
    view.focus();
    return true;
  }
  // Where the marks show too: the visible characters picked change format,
  // so a pair the selection falls inside is split, never broken by a wrap.
  formatSelection(view, format);
  view.focus();
  return true;
}

/**
 * Mod-b and Mod-i, in Raw too (where `toggleFormat` wraps as the source view
 * always has). `Prec.high`, because defaultKeymap binds Mod-i itself.
 */
export const formatKeymap: Extension = Prec.high(
  keymap.of([
    { key: "Mod-b", run: (view) => toggleFormat(view, "strong") },
    { key: "Mod-i", run: (view) => toggleFormat(view, "em") },
    // Where the "Link" popover does not answer (Raw); `linkKey` in the
    // layer, at a higher precedence, opens it everywhere else.
    { key: "Mod-k", run: plainLinkKey },
  ]),
);

// Typing.

/** Characters that write or end Markdown marks: typed as they are written. */
const MARK_CHARS = /[*_~`\\[\]()<>!#|$]/;

/** Characters that can close a format. */
const FORMAT_CHARS = /[*_~`]/;

/** Where typed text goes and what waits after it. */
interface Plan {
  at: number;
  insert: string;
  /** Where the typed text starts inside `insert`. */
  offset: number;
  pending: Pending | null;
  /** The formats the typed text must have, or null for "any". */
  expect: Set<Format> | null;
  /** Whether the line's marks are checked after it. */
  verify: boolean;
  /** The formats whose pairs the insert splits, so their partners change. */
  splits?: Format[];
}

function plain(at: number, text: string, verify: boolean): Plan {
  return { at, insert: text, offset: 0, pending: null, expect: null, verify };
}

function opening(formats: readonly Format[]): string {
  return formats.map((format) => FORMAT_MARKER[format]).join("");
}

function closing(formats: readonly Format[]): string {
  return [...formats]
    .reverse()
    .map((format) => FORMAT_MARKER[format])
    .join("");
}

/** Rule 1: a format waits at the caret. */
function pendingPlan(
  model: LineModel,
  place: number,
  text: string,
  pending: Pending,
  space: boolean,
): Plan | null {
  const base = formatsOfPlace(model, place);
  const on = pending.on.filter((format) => !base.has(format));
  const off = pending.off.filter((format) => base.has(format));
  if (on.length === 0 && off.length === 0) {
    return null;
  }
  if (space && on.length > 0) {
    // A space opens no format: it goes in plain and the format waits on.
    return {
      ...plain(place, text, true),
      pending: { pos: place + text.length, on: pending.on, off: pending.off },
    };
  }
  const expect = new Set([...base, ...on]);
  for (const format of off) expect.delete(format);
  const core = opening(on) + text + closing(on);
  const offset = opening(on).length;
  if (off.length === 0) {
    return {
      at: place,
      insert: core,
      offset,
      pending: null,
      expect,
      verify: true,
    };
  }
  // At the inner end of the pairs: after their closing marks.
  const run = model.runs.find((each) => each.from === place);
  const lead: Format[] = [];
  let end = place;
  for (const mark of run?.marks ?? []) {
    if (
      mark.side === "close" &&
      isFormat(mark.kind) &&
      off.includes(mark.kind) &&
      !lead.includes(mark.kind)
    ) {
      lead.push(mark.kind);
      end = mark.to;
    } else {
      break;
    }
  }
  if (lead.length === off.length) {
    return {
      at: end,
      insert: core,
      offset,
      pending: null,
      expect,
      verify: true,
    };
  }
  // Inside the pairs: closed before the text and opened again after it,
  // the innermost pair first.
  const inner = off
    .map((format) => pairAround(model, format, place))
    .filter((pair): pair is Pair => pair !== null)
    .sort((a, b) => b.open.from - a.open.from)
    .map((pair) => pair.format);
  const close = inner.map((format) => FORMAT_MARKER[format]).join("");
  const open = [...inner]
    .reverse()
    .map((format) => FORMAT_MARKER[format])
    .join("");
  return {
    at: place,
    insert: close + core + open,
    offset: close.length + offset,
    pending: null,
    expect,
    verify: true,
    splits: inner,
  };
}

/** Rule 2: the caret at the inner end of a continuing closing mark. */
function continuePlan(
  model: LineModel,
  place: number,
  text: string,
  space: boolean,
): Plan | null {
  const run = model.runs.find((each) => each.from === place);
  const first = run?.marks[0];
  if (
    !run ||
    !first ||
    first.side !== "close" ||
    !isFormat(first.kind) ||
    !CONTINUING.has(first.kind)
  ) {
    return null;
  }
  if (!space) {
    // The letter extends the format: where the default insert puts it.
    return {
      ...plain(place, text, true),
      expect: formatsOfPlace(model, place),
    };
  }
  // A `**` after a space closes nothing, so the space goes after the
  // closing marks and their formats wait for the next letter.
  const kinds: Format[] = [];
  let end = place;
  for (const mark of run.marks) {
    if (
      mark.side === "close" &&
      isFormat(mark.kind) &&
      CONTINUING.has(mark.kind)
    ) {
      kinds.push(mark.kind);
      end = mark.to;
    } else {
      break;
    }
  }
  return {
    ...plain(end, text, true),
    pending: { pos: end + text.length, on: kinds, off: [] },
  };
}

/**
 * Whether the line, parsed again with `plan`'s insert, keeps every mark it
 * had paired as it was, and the typed text has the formats it must have.
 */
function keepsMarks(
  old: LineModel,
  trial: EditorState,
  plan: Plan,
  text: string,
): boolean {
  const next = lineModel(trial, old.number);
  const length = plan.insert.length;
  const map = (pos: number, assoc: -1 | 1) =>
    pos < plan.at || (pos === plan.at && assoc < 0) ? pos : pos + length;
  const found = old.marks.map((mark) =>
    next.marks.findIndex(
      (each) =>
        each.from === map(mark.from, 1) &&
        each.to === map(mark.to, -1) &&
        each.kind === mark.kind &&
        each.side === mark.side,
    ),
  );
  if (found.some((index) => index < 0)) {
    return false;
  }
  for (const [index, mark] of old.marks.entries()) {
    if (
      mark.partner === null ||
      (isFormat(mark.kind) && plan.splits?.includes(mark.kind) === true)
    ) {
      continue;
    }
    const now = next.marks[found[index] ?? -1];
    if (!now || now.partner !== found[mark.partner]) {
      return false;
    }
  }
  if (plan.expect) {
    const start = plan.at + plan.offset;
    for (let pos = start; pos < start + text.length; pos++) {
      if (
        inMark(next, pos) ||
        !sameFormats(plan.expect, formatsOfChar(next, pos))
      ) {
        return false;
      }
    }
  }
  return true;
}

function trialOf(
  state: EditorState,
  plan: Plan,
  lineTo: number,
  deadline: Deadline,
): EditorState | null {
  return parsedTo(
    state.update({
      changes: { from: plan.at, insert: plan.insert },
      filter: false,
    }).state,
    lineTo + plan.insert.length,
    deadline,
  );
}

/** Rule 3: the formats a closing mark in the typed text ended. */
function closedBy(
  model: LineModel,
  trial: EditorState,
  plan: Plan,
  text: string,
): Format[] {
  const next = lineModel(trial, model.number);
  const start = plan.at + plan.offset;
  const end = start + text.length;
  const out: Format[] = [];
  for (const mark of next.marks) {
    if (
      mark.side === "close" &&
      isFormat(mark.kind) &&
      mark.from < end &&
      mark.to > start &&
      !out.includes(mark.kind)
    ) {
      out.push(mark.kind);
    }
  }
  return out;
}

/** Where typed text goes, the caret after it and what waits there. */
export interface TypedPlan {
  at: number;
  insert: string;
  /** The caret in the result. */
  caret: number;
  pending: Pending | null;
}

/**
 * Where `text`, typed at `from` on `begin` (its caret there), goes by the
 * typing rules (rule 1 with a format waiting, the continue rules, rule 4's
 * places), or null for the default insert as it stands. Shared by typing at
 * a caret and by a composition's end (`composition.ts`), which plans the
 * committed text on the state from before it began.
 */
export function typedPlan(
  begin: EditorState,
  from: number,
  text: string,
  deadline: Deadline,
): TypedPlan | null {
  if (
    text.length === 0 ||
    /[\r\n]/.test(text) ||
    begin.selection.ranges.length !== 1 ||
    !begin.selection.main.empty ||
    begin.facet(mainSelectionParked) ||
    !writable(begin)
  ) {
    return null;
  }
  // The start state parsed to the line first (D35), so a trial reparses
  // only the line it changed. Short of time, the line reads as it stands.
  const state = parsedTo(begin, begin.doc.lineAt(from).to, deadline) ?? begin;
  const number = state.doc.lineAt(from).number;
  const hidden = marksHiddenOnLine(state, number);
  const place = canonical(state, from);
  // The text lands at the caret's own drawn place, or this is not typing at
  // the caret.
  if (place !== canonical(state, state.selection.main.head)) {
    return null;
  }
  const pending = state.field(pendingFormat, false) ?? null;
  const space = /^\s+$/.test(text);
  const formatChars = FORMAT_CHARS.test(text);
  const model = lineModel(state, number);
  const runHere = model.runs.find(
    (run) => run.from <= place && place <= run.to,
  );

  let plan: Plan | null = null;
  if (pending && pending.pos === place) {
    // A Markdown character is typed where it is written: the waiting format
    // goes, and no markers are written around it.
    plan = MARK_CHARS.test(text)
      ? plain(place, text, false)
      : pendingPlan(model, place, text, pending, space);
  }
  if (!plan && hidden) {
    plan = continuePlan(model, place, text, space);
  }
  if (!plan) {
    // Rule 4 for the default insert at a run's edge, rule 3 for a mark
    // character; anything else is the default insert as it stands.
    if (!hidden || (!runHere && !formatChars)) {
      return null;
    }
    plan = plain(place, text, runHere !== undefined);
  }

  // Rule 4: the first place that keeps the line's marks as they were, then
  // after the run, then the caret as it is. A mark character is typed where
  // it is written: the person is writing Markdown.
  const candidates: Plan[] = [plan];
  if (runHere && runHere.to !== plan.at) {
    candidates.push(plain(runHere.to, text, true));
  }
  let chosen: Plan | null = null;
  let trial: EditorState | null = null;
  for (const candidate of candidates) {
    const next = trialOf(state, candidate, model.to, deadline);
    if (
      !candidate.verify ||
      MARK_CHARS.test(text) ||
      (next !== null && keepsMarks(model, next, candidate, text))
    ) {
      chosen = candidate;
      trial = next;
      break;
    }
  }
  if (!chosen) {
    chosen = plain(place, text, false);
    trial = trialOf(state, chosen, model.to, deadline);
  }
  const typedAt = chosen.at + chosen.offset + text.length;
  const caretAt = trial ? canonical(trial, typedAt) : typedAt;
  let next: Pending | null =
    chosen.pending && chosen === plan
      ? { ...chosen.pending, pos: caretAt }
      : null;
  // Rule 3: a closing mark typed by hand ends its format at the caret.
  if (hidden && formatChars && trial) {
    const base = formatsOfPlace(lineModel(trial, number), caretAt);
    const off = closedBy(model, trial, chosen, text).filter((format) =>
      base.has(format),
    );
    if (off.length > 0) {
      next = { pos: caretAt, on: [], off };
    }
  }
  if (
    chosen.at === from &&
    chosen.insert === text &&
    next === null &&
    pending === null
  ) {
    return null;
  }
  return {
    at: chosen.at,
    insert: chosen.insert,
    caret: caretAt,
    pending: next,
  };
}

function typed(
  view: EditorView,
  from: number,
  to: number,
  text: string,
): boolean {
  if (from !== to || view.composing || view.compositionStarted) {
    return false;
  }
  const plan = typedPlan(
    view.state,
    from,
    text,
    deadlineIn(PARSE_BUDGET.typing),
  );
  if (!plan) {
    return false;
  }
  view.dispatch({
    changes: { from: plan.at, insert: plan.insert },
    selection: EditorSelection.cursor(plan.caret),
    effects: setPending.of(plan.pending),
    userEvent: "input.type",
    scrollIntoView: true,
  });
  return true;
}

/**
 * Typing with a format waiting or at the edge of one: rule 1 in hidden and
 * show mode, the continue rules on lines whose marks are hidden. Only one
 * string typed at an empty caret, never a composition.
 *
 * The formatting on the caret's left wins: ` more` typed after a bold word
 * at the end of a line stays bold, with the space outside the marks
 * (`**bold** **more**`), because a `**` after a space closes nothing.
 */
export const formatInput: Extension = EditorView.inputHandler.of(
  (view, from, to, text) => typed(view, from, to, text),
);

/** What the hidden-mark layer installs for the format commands. */
export const formatting: Extension = [
  pendingFormat,
  forgetOnBlur,
  formatInput,
  // Mod-k: the "Link" popover, where marks are hidden.
  linkKey,
];

// The pressed state.

interface FormatHub {
  main: EditorView;
  onChange: (formats: ReadonlySet<Format>) => void;
  last: string | null;
}

const formatHubs = new WeakMap<EditorView, FormatHub>();

function isDestroyed(view: EditorView): boolean {
  return (view as unknown as { destroyed?: unknown }).destroyed === true;
}

/** Reports what the view a format verb would act on has, if it changed. */
function report(hub: FormatHub): void {
  if (formatHubs.get(hub.main) !== hub || isDestroyed(hub.main)) {
    return;
  }
  const target = verbTarget(hub.main);
  const formats =
    target && !isDestroyed(target)
      ? formatsOfSelection(target.state)
      : new Set<Format>();
  const key = FORMATS.filter((format) => formats.has(format)).join(" ");
  if (key === hub.last) {
    return;
  }
  hub.last = key;
  hub.onChange(formats);
}

function formatsMayHaveChanged(update: ViewUpdate): boolean {
  return (
    update.selectionSet ||
    update.docChanged ||
    update.focusChanged ||
    update.transactions.some((tr) => tr.reconfigured) ||
    parkingChanged(update) ||
    update.startState.field(pendingFormat, false) !==
      update.state.field(pendingFormat, false) ||
    syntaxTree(update.startState) !== syntaxTree(update.state)
  );
}

/**
 * Tells `onChange` the formats the format buttons show as pressed: those of
 * the view a format verb would act on (the open cell's, else main's, none
 * while main's selection is parked with no cell mounted). The cell side
 * reports through `formatCell(main)`.
 */
export function formatStateListener(
  onChange: (formats: ReadonlySet<Format>) => void,
): Extension {
  return ViewPlugin.fromClass(
    class {
      readonly hub: FormatHub;

      constructor(view: EditorView) {
        this.hub = { main: view, onChange, last: null };
        formatHubs.set(view, this.hub);
        report(this.hub);
      }

      update(update: ViewUpdate): void {
        if (formatsMayHaveChanged(update)) report(this.hub);
      }

      destroy(): void {
        if (formatHubs.get(this.hub.main) === this.hub) {
          formatHubs.delete(this.hub.main);
        }
      }
    },
  );
}

/**
 * The cell view's side, added by the grid to every cell editor it mounts on
 * `main`: the cell's formats reach `main`'s listener while the cell is the
 * view a verb acts on.
 */
export function formatCell(main: EditorView): Extension {
  return ViewPlugin.fromClass(
    class {
      constructor() {
        const hub = formatHubs.get(main);
        if (hub) report(hub);
      }

      update(update: ViewUpdate): void {
        const hub = formatHubs.get(main);
        if (hub && formatsMayHaveChanged(update)) report(hub);
      }

      destroy(): void {
        // After the teardown, when main is the target again.
        queueMicrotask(() => {
          const hub = formatHubs.get(main);
          if (hub) report(hub);
        });
      }
    },
  );
}
