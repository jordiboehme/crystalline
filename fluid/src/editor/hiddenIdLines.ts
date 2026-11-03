/**
 * Section id lines, hidden in the editor.
 *
 * Every `##` of a reflection carries `<!-- r:<id> -->` on the line below it
 * (the core On-disk formats). The ids are bookkeeping: the live room saves the
 * text verbatim and the engine adds ids on the room's final save (R12), so
 * the lines stay in the document and are never shown. A block replacement
 * per id line, from a state field (block decorations cannot come from a
 * plugin), marked atomic so the caret steps over them.
 *
 * Hidden is not enough on its own: a person cannot see what their keys do to
 * a line they never see. An id only follows its section while it stays alone
 * on the line right below the heading (the engine's `reattach_ids` reads it
 * there); merged into a line, split off by a new line or deleted, it is lost,
 * the next save mints a new id, and every thread on the section is orphaned.
 * So the person's own typing is kept off the id line: a new line typed at a
 * heading's end goes below the id, a single-character delete never touches
 * the id line or its line breaks (it goes only with its whole heading), a
 * selection from inside a heading into the body leaves the heading its id,
 * and copied text leaves the ids out, so a pasted copy cannot take a section's id
 * and its threads. Only this person's typing is steered: a change from the
 * room carries no user event and is applied as it came.
 */

import type {
  Extension,
  Line,
  SelectionRange,
  Text,
  TransactionSpec,
} from "@codemirror/state";
import {
  Annotation,
  ChangeSet,
  EditorSelection,
  EditorState,
  Facet,
  Prec,
  RangeSetBuilder,
  StateField,
  Transaction,
} from "@codemirror/state";
import type { DecorationSet } from "@codemirror/view";
import { Decoration, EditorView, keymap } from "@codemirror/view";
import { isolateHistory } from "@codemirror/commands";

import { isIdLine, stripIdLines } from "../reflectionText";

function build(state: EditorState): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  for (let n = 1; n <= state.doc.lines; n += 1) {
    const line = state.doc.line(n);
    if (isIdLine(line.text)) {
      builder.add(line.from, line.to, Decoration.replace({ block: true }));
    }
  }
  return builder.finish();
}

const field = StateField.define<DecorationSet>({
  create: build,
  update: (value, tr) => (tr.docChanged ? build(tr.state) : value),
  provide: (f) => [
    EditorView.decorations.from(f),
    EditorView.atomicRanges.of((view) => view.state.field(f)),
  ],
});

/**
 * Whether line `lineNo` (1-based) is a hidden section id line. Only where
 * these ids are hidden: in an engram, the same comment is ordinary text, so
 * the block verbs that ask this treat it like any other line there.
 */
export function isIdLineAt(state: EditorState, lineNo: number): boolean {
  if (state.field(field, false) === undefined) {
    return false;
  }
  if (lineNo < 1 || lineNo > state.doc.lines) {
    return false;
  }
  return isIdLine(state.doc.line(lineNo).text);
}

/**
 * Whether this buffer hides section id lines: a Reflection page, which the
 * page renders one section at a time.
 */
export function hidesIdLines(state: EditorState): boolean {
  return state.field(field, false) !== undefined;
}

/**
 * Whether the page this buffer edits a part of is a Reflection, which never
 * shows an HTML comment. The table cell editor holds no id line field of its
 * own (it hides everything outside the cell), so it is told this by its main
 * view's `hidesIdLines`.
 */
export const pageHidesComments = Facet.define<boolean, boolean>({
  combine: (values) => values.some(Boolean),
});

/**
 * The annotations a filter that rebuilds `tr` carries over: its user event
 * (as `input` when it had none), and whether and how it enters the undo
 * history and whether it came from elsewhere, so a rebuilt edit is the same
 * edit to every other part of the editor.
 */
function carried(tr: Transaction): Annotation<unknown>[] {
  const out: Annotation<unknown>[] = [
    Transaction.userEvent.of(tr.annotation(Transaction.userEvent) ?? "input"),
  ];
  const history = tr.annotation(Transaction.addToHistory);
  if (history !== undefined) {
    out.push(Transaction.addToHistory.of(history));
  }
  const remote = tr.annotation(Transaction.remote);
  if (remote !== undefined) {
    out.push(Transaction.remote.of(remote));
  }
  const isolate = tr.annotation(isolateHistory);
  if (isolate !== undefined) {
    out.push(isolateHistory.of(isolate));
  }
  return out;
}

/** A heading line of any level: what a kept id may sit under. */
const HEADING_LINE = /^#{1,6}[ \t]/;

/** The id line `pos` is on, or null. The first line is never one: no heading above. */
function idLineAt(doc: Text, pos: number): Line | null {
  const line = doc.lineAt(pos);
  return line.number > 1 && isIdLine(line.text) ? line : null;
}

/**
 * Where a caret that landed on id line `line` rests instead: the end of the
 * heading above, or the start of the line below. `down` says which; without
 * it the start of the id line goes up and every other position goes down. An
 * id line at the end of the page has no line below, so it goes up.
 */
function offIdLine(doc: Text, line: Line, pos: number, down?: boolean): number {
  const below = line.number < doc.lines ? line.to + 1 : null;
  const goDown = down ?? pos !== line.from;
  return goDown && below !== null ? below : line.from - 1;
}

/**
 * A caret never rests on an id line. The line is atomic, but both its ends
 * are caret positions, and a verb run from there would write into the id
 * (`****<!-- r:... -->`). A caret that lands there goes on in the direction it
 * came from: one ArrowRight from a heading's end reaches the next visible
 * line, one ArrowLeft from there comes back to the heading's end.
 */
const caretOffIdLines = EditorState.transactionFilter.of((tr) => {
  const doc = tr.newDoc;
  const selection = tr.newSelection;
  // Only a step of this person's own says where the caret came from. Any
  // other caret (one a remote change, a Y undo or a dispatch mapped there)
  // was below the heading before the change, so it goes down, never up into
  // the heading where the next key would land in it.
  const stepped = tr.isUserEvent("select");
  const before = tr.changes.mapPos(tr.startState.selection.main.head);
  let moved = false;
  const ranges = selection.ranges.map((range) => {
    // A selection that ends on an id line is left as it is: what an edit of
    // it does to the id is the other filters' to put right.
    const line = range.empty ? idLineAt(doc, range.head) : null;
    if (line === null) {
      return range;
    }
    moved = true;
    // A mapped caret on an id line that ends the page has no line below to
    // go to, and going up would put the next key into the heading: it stays
    // at the id line's end, where typing lands after a new line break
    // (`insertsOffIdLines`).
    if (!stepped && line.number === doc.lines) {
      return EditorSelection.cursor(line.to);
    }
    const down = !stepped
      ? true
      : before < line.from
        ? true
        : before > line.to
          ? false
          : undefined;
    return EditorSelection.cursor(offIdLine(doc, line, range.head, down));
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

/**
 * Move the caret off an id line before a verb writes at it, the way
 * `caretOffIdLines` would have: the start of the id line goes to the end of
 * the heading above, the end to the start of the line below. For the state a
 * buffer opened with, which no filter has seen. Nothing happens where the
 * ids are not hidden.
 */
export function stepOffIdLines(view: EditorView): void {
  const { state } = view;
  if (state.field(field, false) === undefined) {
    return;
  }
  let moved = false;
  const fix = (pos: number): number => {
    const line = idLineAt(state.doc, pos);
    if (line === null) {
      return pos;
    }
    moved = true;
    return offIdLine(state.doc, line, pos);
  };
  const ranges = state.selection.ranges.map((range) =>
    EditorSelection.range(fix(range.anchor), fix(range.head)),
  );
  if (moved) {
    view.dispatch({
      selection: EditorSelection.create(ranges, state.selection.mainIndex),
    });
  }
}

/**
 * Text this person puts in at an id line goes beside it, so the id line stays
 * byte for byte what it was: typing, a paste or a verb from a caret on either
 * end of it. It goes to the start of the line below (after a new line break
 * when the id line ends the page), never up into the heading: a caret there
 * was mapped from below. A text that starts with a line break at the end of
 * the id line is already beside it and stays.
 */
const insertsOffIdLines = EditorState.transactionFilter.of((tr) => {
  if (!tr.docChanged || !tr.isUserEvent("input")) {
    return tr;
  }
  const doc = tr.startState.doc;
  const specs: { from: number; to: number; insert: Text | string }[] = [];
  const spans: { fromB: number; toB: number; at: number; lead: number }[] = [];
  let moved = false;
  tr.changes.iterChanges((fromA, toA, fromB, toB, inserted) => {
    const line = fromA === toA ? idLineAt(doc, fromA) : null;
    const beside =
      line !== null &&
      fromA === line.to &&
      inserted.lines > 1 &&
      inserted.line(1).length === 0;
    if (line === null || beside) {
      specs.push({ from: fromA, to: toA, insert: inserted });
      spans.push({ fromB, toB, at: fromA, lead: 0 });
      return;
    }
    moved = true;
    // Never up into the heading: a caret at an id line came there by being
    // mapped from the line below, or it is where the buffer opened, which
    // no filter has seen (`stepOffIdLines` is for the verbs there). Typing
    // at the heading's own end is `newLineBelowId`'s.
    if (line.number < doc.lines) {
      specs.push({ from: line.to + 1, to: line.to + 1, insert: inserted });
      spans.push({ fromB, toB, at: line.to + 1, lead: 0 });
    } else {
      const lead = tr.startState.lineBreak;
      specs.push({
        from: line.to,
        to: line.to,
        insert: `${lead}${inserted.toString()}`,
      });
      // A line break is one position however it is written.
      spans.push({ fromB, toB, at: line.to, lead: 1 });
    }
  });
  if (!moved) {
    return tr;
  }
  const changes = tr.startState.changes(specs);
  // The selection the change asked for, carried along with the text it was
  // in, or mapped through untouched text.
  const place = (pos: number): number => {
    for (const span of spans) {
      if (pos >= span.fromB && pos <= span.toB) {
        return changes.mapPos(span.at, -1) + span.lead + (pos - span.fromB);
      }
    }
    return changes.mapPos(tr.changes.invertedDesc.mapPos(pos), 1);
  };
  const asked = tr.selection;
  return {
    changes,
    ...(asked === undefined
      ? {}
      : {
          selection: EditorSelection.create(
            asked.ranges.map((r) =>
              EditorSelection.range(place(r.anchor), place(r.head)),
            ),
            asked.mainIndex,
          ),
        }),
    effects: tr.effects,
    scrollIntoView: tr.scrollIntoView,
    annotations: carried(tr),
  };
});

/**
 * Marks the one change that may take a section's id away while its heading
 * stays: a confirmed heading change that joins the section into the one
 * above (`##` made `###`, a title or plain text). It lets through the id line
 * of exactly that heading, the one whose mark the same transaction changes
 * and whose id line it deletes whole. Every other id the transaction reaches
 * is kept as without it, and a single-character delete is refused as ever.
 */
export const joinSections = Annotation.define<boolean>();

/**
 * A new line typed at the end of a heading lands below the heading's id, and
 * so does a block inserted from a heading line (`insertBlock` writes it at
 * the line's end, led by a line break): a toolbar block or an uploaded file's
 * link never splits a heading from its id.
 */
const newLineBelowId = EditorState.transactionFilter.of((tr) => {
  if (!tr.docChanged || !tr.isUserEvent("input")) {
    return tr;
  }
  const doc = tr.startState.doc;
  const inserts: { at: number; text: string }[] = [];
  let other = false;
  tr.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
    if (fromA === toA) {
      inserts.push({ at: fromA, text: inserted.toString() });
    } else {
      other = true;
    }
  });
  const only = inserts[0];
  if (other || inserts.length !== 1 || only === undefined) {
    return tr;
  }
  if (!only.text.startsWith("\n")) {
    return tr;
  }
  const line = doc.lineAt(only.at);
  if (only.at !== line.to || line.number >= doc.lines) {
    return tr;
  }
  const below = doc.line(line.number + 1);
  if (!isIdLine(below.text)) {
    return tr;
  }
  // The selection the insert asked for moves down with it: a typed new line
  // leaves the caret after it, a block inserted from the toolbar or an upload
  // keeps the caret or selection it put inside itself.
  const shift = below.to - only.at;
  const end = only.at + only.text.length;
  const asked = tr.selection;
  const inside =
    asked !== undefined &&
    asked.ranges.every((r) => r.from >= only.at && r.to <= end);
  return {
    changes: { from: below.to, insert: only.text },
    selection: inside
      ? EditorSelection.create(
          asked.ranges.map((r) =>
            EditorSelection.range(r.anchor + shift, r.head + shift),
          ),
          asked.mainIndex,
        )
      : EditorSelection.cursor(below.to + only.text.length),
    scrollIntoView: true,
    annotations: carried(tr),
  };
});

/**
 * A single-character delete (Backspace, Delete) that would reach an id line
 * or one of its line breaks. The one such delete that is let through is the
 * break after the id when a blank line follows: that removes the blank line
 * and leaves the id alone, and the caret then goes to the start of the next
 * line rather than staying inside the hidden one. Anything else is dropped,
 * unless it takes the whole heading with it.
 */
const keepIdLines = EditorState.transactionFilter.of((tr) => {
  if (
    !tr.docChanged ||
    !(tr.isUserEvent("delete.backward") || tr.isUserEvent("delete.forward"))
  ) {
    return tr;
  }
  const doc = tr.startState.doc;
  let blocked = false;
  let caret: number | null = null;
  tr.changes.iterChanges((fromA, toA) => {
    const first = doc.lineAt(Math.max(0, fromA - 1)).number;
    const last = doc.lineAt(Math.min(doc.length, toA + 1)).number;
    for (let n = first; n <= last; n += 1) {
      const line = doc.line(n);
      if (!isIdLine(line.text)) {
        continue;
      }
      // The id line and the breaks on both sides of it.
      const guardFrom = n > 1 ? line.from - 1 : line.from;
      const guardTo = n < doc.lines ? line.to + 1 : line.to;
      if (fromA >= guardTo || toA <= guardFrom) {
        continue;
      }
      const heading = n > 1 ? doc.line(n - 1) : null;
      if (heading !== null && fromA <= heading.from && toA >= line.to) {
        continue; // the whole section head goes, id and all
      }
      const blankBelow = n < doc.lines && doc.line(n + 1).length === 0;
      if (fromA === line.to && toA === line.to + 1 && blankBelow) {
        caret = line.to + 1;
        continue;
      }
      blocked = true;
    }
  });
  if (blocked) {
    return [];
  }
  if (caret === null) {
    return tr;
  }
  const after: TransactionSpec = {
    selection: EditorSelection.cursor(Math.min(caret, tr.newDoc.length)),
    sequential: true,
  };
  return [tr, after];
});

/**
 * A change that crosses the line break after a heading: a selection deleted,
 * cut, typed over, pasted over or dropped, or a whole line deleted. The id
 * line is hidden, so the line break a person sees after a heading is the
 * whole stretch from the heading's end to the start of the next visible line,
 * id and all, and the edit is read that way:
 *
 * - The heading's first character goes (a selection from the heading's start,
 *   a triple-click, a deleted line): the whole heading goes, and its id with
 *   it, wherever the change ended inside that stretch. Unless the text put in
 *   its place starts a new heading line ("## New name", or the "#" a person
 *   types first when they retype a heading): then the section is renamed,
 *   and its id goes on the line right below that new heading line, so its
 *   threads stay with it. Any other text ("X") takes the heading's place
 *   and the id goes.
 * - The heading's start stays: the id goes back on the line right below what
 *   is left of the heading (below the first line of a pasted text), wherever
 *   the change left it. When the line below already holds an id, the
 *   surviving heading's own id takes that place: the text that is left is
 *   this heading's, and the heading that id belonged to has gone.
 * - A change that stays on the heading line (its mark made `###` or taken
 *   off) leaves the heading's line where it was, so the id stays below it
 *   too. Only a confirmed join (`joinSections`) takes that id away.
 *
 * A heading counts as gone too when the line break before it was deleted,
 * so two selections that end on one line leave one id there, not two. The
 * edit itself happens as the person made it; the ids are put right in the
 * same transaction, so one Undo takes both back.
 */
const keepHeadingIds = Prec.high(
  EditorState.transactionFilter.of((tr) => {
    if (
      !tr.docChanged ||
      tr.isUserEvent("delete.backward") ||
      tr.isUserEvent("delete.forward") ||
      tr.isUserEvent("move.line") ||
      !(
        tr.isUserEvent("input") ||
        tr.isUserEvent("delete") ||
        tr.isUserEvent("move")
      )
    ) {
      return tr;
    }
    const doc = tr.startState.doc;
    const joining = tr.annotation(joinSections) === true;
    const changes: { fromA: number; toA: number; inserted: string }[] = [];
    tr.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
      changes.push({ fromA, toA, inserted: inserted.toString() });
    });
    // The headings whose line break the transaction reached into.
    const touched: {
      heading: Line;
      id: Line;
      survives: boolean;
      renamed: boolean;
    }[] = [];
    const seen = new Set<number>();
    for (const { fromA, toA } of changes) {
      if (fromA === toA) {
        continue; // a pure insert: `newLineBelowId` places it
      }
      const first = doc.lineAt(fromA).number;
      const last = doc.lineAt(toA).number;
      for (let n = Math.max(1, first - 1); n <= last; n += 1) {
        if (n >= doc.lines || seen.has(n)) {
          continue;
        }
        const heading = doc.line(n);
        const id = doc.line(n + 1);
        if (!isIdLine(id.text)) {
          continue;
        }
        const seamEnd = n + 1 < doc.lines ? id.to + 1 : id.to;
        if (!(fromA < seamEnd && toA > heading.to)) {
          continue;
        }
        seen.add(n);
        // A heading replaced whole by a new heading line: the section is
        // renamed, not gone, and keeps its id.
        const renamed = changes.some(
          (c) =>
            c.fromA === heading.from &&
            c.toA > heading.to &&
            c.inserted.startsWith("#"),
        );
        // A change that stays on the heading line (its mark changed or
        // taken off) leaves the line where it was: the heading is gone only
        // when a change runs from its start past its end, or takes the line
        // break before it.
        // And the line left there must still be a heading: one change that
        // wrote "Note: " over the mark while another crossed the line break
        // leaves plain text, which keeps no id.
        const survives =
          renamed ||
          (!changes.some(
            (c) =>
              (c.fromA <= heading.from && c.toA > heading.to) ||
              (heading.from > 0 &&
                c.fromA < heading.from &&
                c.toA >= heading.from),
          ) &&
            HEADING_LINE.test(
              tr.newDoc.lineAt(tr.changes.mapPos(heading.from, -1)).text,
            ));
        // The confirmed join: this heading's mark changes and its id line
        // goes whole, as the person agreed to.
        const joined =
          joining &&
          changes.some(
            (c) => c.fromA === heading.from && c.toA <= heading.to,
          ) &&
          changes.some((c) => c.fromA <= id.from && c.toA >= id.to);
        if (joined) {
          continue;
        }
        touched.push({ heading, id, survives, renamed });
      }
    }
    if (touched.length === 0) {
      return tr;
    }
    const after = tr.newDoc;
    // First: every id text the change left standing goes, with one line
    // break beside it, unless it already sits alone below its heading.
    const removals: { from: number; to: number }[] = [];
    const placing: { headingAt: number; id: string }[] = [];
    for (const { heading, id, survives, renamed } of touched) {
      const intact = !changes.some((c) => c.fromA < id.to && c.toA > id.from);
      const headingAt = tr.changes.mapPos(heading.from, -1);
      if (intact) {
        const at = tr.changes.mapPos(id.from, 1);
        const end = at + id.text.length;
        if (survives) {
          const line = after.lineAt(headingAt);
          if (
            line.number < after.lines &&
            after.line(line.number + 1).from === at &&
            after.line(line.number + 1).to === end
          ) {
            continue; // still alone right below its heading
          }
        }
        if (renamed) {
          // The line break after the old heading stays: the new heading
          // line took the old one's place, not the body's.
          removals.push({ from: at, to: end });
        } else if (
          end < after.length &&
          after.sliceString(end, end + 1) === "\n"
        ) {
          removals.push({ from: at, to: end + 1 });
        } else if (at > 0 && after.sliceString(at - 1, at) === "\n") {
          removals.push({ from: at - 1, to: end });
        } else {
          removals.push({ from: at, to: end });
        }
      }
      if (survives) {
        placing.push({ headingAt, id: id.text });
      }
    }
    const removed = ChangeSet.of(removals, after.length);
    const middle = removed.apply(after);
    const inserts: { from: number; to: number; insert: string }[] = [];
    const lines = new Set<number>();
    for (const { headingAt, id } of placing) {
      const line = middle.lineAt(removed.mapPos(headingAt, -1));
      if (lines.has(line.number)) {
        continue;
      }
      lines.add(line.number);
      const below =
        line.number < middle.lines ? middle.line(line.number + 1) : null;
      if (below !== null && isIdLine(below.text)) {
        if (below.text !== id) {
          inserts.push({ from: below.from, to: below.to, insert: id });
        }
      } else {
        inserts.push({ from: line.to, to: line.to, insert: `\n${id}` });
      }
    }
    const specs: TransactionSpec[] = [tr];
    if (removals.length > 0) {
      specs.push({ changes: removals, sequential: true });
    }
    if (inserts.length > 0) {
      specs.push({ changes: inserts, sequential: true });
    }
    return specs.length === 1 ? tr : specs;
  }),
);

/**
 * Ids inside pasted or dropped text go, whatever path the text came by: the
 * clipboard filter below covers a real paste, and this covers a paste or a
 * drop built some other way. An id taken along into another place would give
 * a second section the id, and the threads, of the first. Only whole id lines
 * that the paste itself brought are taken out, with one line break of the
 * paste beside them; the page's own ids are never touched here.
 */
const stripPastedIds = Prec.high(
  EditorState.transactionFilter.of((tr) => {
    if (
      !tr.docChanged ||
      !(tr.isUserEvent("input.paste") || tr.isUserEvent("input.drop"))
    ) {
      return tr;
    }
    const doc = tr.newDoc;
    const cuts: { from: number; to: number }[] = [];
    tr.changes.iterChanges((_fromA, _toA, fromB, toB) => {
      if (fromB === toB) {
        return;
      }
      const first = doc.lineAt(fromB).number;
      const last = doc.lineAt(toB).number;
      for (let n = first; n <= last; n += 1) {
        const line = doc.line(n);
        if (line.from < fromB || line.to > toB || !isIdLine(line.text)) {
          continue;
        }
        if (line.to + 1 <= toB) {
          cuts.push({ from: line.from, to: line.to + 1 });
        } else if (line.from - 1 >= fromB) {
          cuts.push({ from: line.from - 1, to: line.to });
        } else {
          cuts.push({ from: line.from, to: line.to });
        }
      }
    });
    return cuts.length === 0 ? tr : [tr, { changes: cuts, sequential: true }];
  }),
);

/**
 * The lines a move takes along: a heading with the id line below it, or the
 * one line. An id line on its own is taken with the heading above it.
 */
function unitAt(doc: Text, n: number): { first: number; last: number } {
  if (isIdLine(doc.line(n).text) && n > 1) {
    return { first: n - 1, last: n };
  }
  if (n < doc.lines && isIdLine(doc.line(n + 1).text)) {
    return { first: n, last: n + 1 };
  }
  return { first: n, last: n };
}

/**
 * Move the selected lines up or down past the next line, with a heading and
 * its id line moved as one: neither the lines that move nor the line they
 * pass can separate a heading from its id. Without an id line in reach it
 * leaves the move to the default command.
 */
function moveLinesWithIds(view: EditorView, forward: boolean): boolean {
  const { state } = view;
  if (state.readOnly) {
    return false;
  }
  const doc = state.doc;
  const blocks: { first: number; last: number; ranges: SelectionRange[] }[] =
    [];
  for (const range of state.selection.ranges) {
    let last = doc.lineAt(range.to).number;
    const first = doc.lineAt(range.from).number;
    if (!range.empty && range.to === doc.line(last).from && last > first) {
      last -= 1;
    }
    const block = {
      first: unitAt(doc, first).first,
      last: unitAt(doc, last).last,
      ranges: [range],
    };
    const previous = blocks.at(-1);
    if (previous !== undefined && block.first <= previous.last) {
      previous.last = Math.max(previous.last, block.last);
      previous.ranges.push(range);
    } else {
      blocks.push(block);
    }
  }
  let ids = false;
  const changes: { from: number; to: number; insert: string }[] = [];
  const ranges: SelectionRange[] = [];
  for (const block of blocks) {
    if (forward ? block.last === doc.lines : block.first === 1) {
      // Nowhere to go. A block that holds an id stays as it is: the default
      // command would move its lines inside it and part a heading from its
      // id.
      for (let n = block.first; n <= block.last && !ids; n += 1) {
        ids = isIdLine(doc.line(n).text);
      }
      ranges.push(...block.ranges);
      continue;
    }
    const pass = unitAt(doc, forward ? block.last + 1 : block.first - 1);
    const low = Math.min(block.first, pass.first);
    const high = Math.max(block.last, pass.last);
    for (let n = low; n <= high && !ids; n += 1) {
      ids = isIdLine(doc.line(n).text);
    }
    const from = doc.line(block.first).from;
    const to = doc.line(block.last).to;
    const passFrom = doc.line(pass.first).from;
    const passTo = doc.line(pass.last).to;
    const text = doc.sliceString(from, to);
    const passed = doc.sliceString(passFrom, passTo);
    const shift = (passed.length + state.lineBreak.length) * (forward ? 1 : -1);
    changes.push(
      forward
        ? { from, to: passTo, insert: `${passed}${state.lineBreak}${text}` }
        : { from: passFrom, to, insert: `${text}${state.lineBreak}${passed}` },
    );
    for (const r of block.ranges) {
      ranges.push(EditorSelection.range(r.anchor + shift, r.head + shift));
    }
  }
  if (!ids) {
    return false;
  }
  // With an id in reach the default command never runs: it would part a
  // heading from its id. So a move that cannot be made (a block at the edge
  // of the page, or several carets whose moves overlap) does nothing.
  if (changes.length === 0) {
    return true;
  }
  const sorted = [...changes].sort((x, y) => x.from - y.from);
  if (sorted.some((c, i) => i > 0 && c.from < (sorted[i - 1]?.to ?? 0))) {
    return true;
  }
  view.dispatch({
    changes,
    selection: EditorSelection.create(ranges, state.selection.mainIndex),
    scrollIntoView: true,
    userEvent: "move.line",
  });
  return true;
}

const moveWithIds = Prec.high(
  keymap.of([
    { key: "Alt-ArrowUp", run: (view) => moveLinesWithIds(view, false) },
    { key: "Alt-ArrowDown", run: (view) => moveLinesWithIds(view, true) },
  ]),
);

/**
 * Pasted text without ids: an id copied from elsewhere (another page, or this
 * one through a channel the copy filter does not see) would give a second
 * section the id, and the threads, of the first.
 */
const pasteWithoutIds = EditorView.clipboardInputFilter.of((text) =>
  stripIdLines(text),
);

/** Copied text without the ids: a pasted copy must not take a section's id. */
const copyWithoutIds = EditorView.clipboardOutputFilter.of((text) =>
  stripIdLines(text),
);

export function hiddenIdLines(): Extension {
  return [
    field,
    newLineBelowId,
    keepIdLines,
    insertsOffIdLines,
    caretOffIdLines,
    // Later in this list runs first: the paste loses its ids before the
    // heading ids are put right.
    keepHeadingIds,
    stripPastedIds,
    moveWithIds,
    copyWithoutIds,
    pasteWithoutIds,
  ];
}
