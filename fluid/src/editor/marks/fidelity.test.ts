/**
 * Byte fidelity of the hidden marks, as properties over a real corpus: the
 * repository's own Markdown (`docs/*.md`, `release-notes/*.md`, `README.md`)
 * and one hand-written file with every construct the editor draws
 * (`fixtures/dialect/everything.md`: LF, a CRLF copy made here, and the LF
 * file as a Reflection). Every sample runs with marks hidden and with marks
 * shown on the edited line, solo; three files (`ROOM`) run in a room too.
 *
 * 1. Opening, moving the caret to every canonical stop, selecting a line and
 *    flipping the setting twice dispatch no change at all.
 * 2. At a run boundary, typing one letter adds exactly that letter at one
 *    place, and one undo gives back the bytes.
 * 3. At the same places, Backspace and Delete never change the mark pairs
 *    around the caret, except by removing a pair they left empty. In show
 *    mode the line's marks are text the keys may remove, so a key that
 *    removes a mark character is not judged there.
 * 4. Bold on a line's visible text undoes to the bytes; on a line with no
 *    bold, Bold twice gives the bytes back (or the first one is refused).
 * 5. Every element editor committed with its value unchanged writes nothing
 *    and adds no undo step; a menu pick of the checked item writes nothing.
 *
 * Lines of tables the grid draws are skipped: main never holds a caret
 * there, and the cell properties run in `grid/cellFidelity.test.ts`.
 *
 * The sample, chosen so the file stays well under a minute. Every edit on a
 * document up to 6,000 characters re-reads its footnotes inside the
 * keystroke (`footnotes.ts`), which is what an edit costs there and is kept.
 * - The LF dialect file, marks hidden, solo, is checked in full: every line,
 *   every run boundary.
 * - Every other run takes at most `BOUNDARIES` run boundaries (properties 2
 *   and 3) and `LINES` lines (properties 1 and 4) of a sample: a fixed stride
 *   over them in document order, `ceil(count / budget)`, starting at the
 *   first. Show mode starts one stride step later than hidden mode where the
 *   stride allows it, so the two modes see different places.
 *
 * Failures are collected, not thrown at the first one, so a run names every
 * case it found, each with its line and caret.
 */

import { ensureSyntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
import { EditorSelection, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { SyntaxNodeRef } from "@lezer/common";
import { act, cleanup, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import type { MarkViewOptions } from "../../test/markKit";
import {
  bytes,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  setShowMarks,
  showsMarks,
  typeText,
  undoOnce,
  undoSteps,
} from "../../test/markKit";
import { popoverView } from "../../test/popoverKit";
import { repoMarkdown } from "../../test/repoMarkdown";
import type { ElementEdit } from "../elementEdit";
import {
  elementEditAt,
  openElementEdit,
  requestElementEdit,
} from "../elementEdit";
import { codeFenceAt, mathAt } from "../elementVerbs";
import { prefetchGrid } from "../grid/loader";
import { gridTables } from "../grid/structure";
import { canonical } from "./caret";
import type { DeleteOutcome } from "./deleteCheck";
import { deletionOutcome } from "./deleteCheck";
import { toggleFormat } from "./formatting";
import { linkDefinition, marksOnLine, runsOnLine } from "./hiddenMarks";

const DIALECT = import.meta.glob<string>("../fixtures/dialect/*.md", {
  query: "?raw",
  import: "default",
  eager: true,
});

const EVERYTHING = Object.values(DIALECT)[0] ?? "";

/** Run boundaries checked per sampled run (properties 2 and 3). */
const BOUNDARIES = 4;

/**
 * The most of the judged Backspace and Delete presses that may be refused
 * (D5): a refusal is an allowed outcome, but one in twenty is the most the
 * corpus may show.
 */
const MAX_REFUSED = 0.05;

/** Lines checked per sampled run (properties 1 and 4). */
const LINES = 3;

/** The two corpus files the room runs besides the dialect file. */
const ROOM = ["release-notes/v0.19.3.md", "docs/README.md"];

type Mode = "hidden" | "show";
const MODES: Mode[] = ["hidden", "show"];

interface Sample {
  name: string;
  text: string;
  adapter?: "engram" | "reflection";
}

const CORPUS: Sample[] = repoMarkdown().map(([name, text]) => ({
  name,
  text,
}));

const LF: Sample = { name: "everything.md", text: EVERYTHING };

const CRLF: Sample = {
  name: "everything.md (CRLF)",
  text: EVERYTHING.replaceAll("\n", "\r\n"),
};

const FIXTURES: Sample[] = [
  LF,
  CRLF,
  {
    name: "everything.md (Reflection)",
    text: EVERYTHING,
    adapter: "reflection",
  },
];

/** The letter typed: one that writes and closes no mark. */
const LETTER = "q";

/** The inline nodes whose first and last children are a pair of marks. */
const PAIR_NODES = new Set([
  "Emphasis",
  "StrongEmphasis",
  "Strikethrough",
  "InlineCode",
  "Link",
  "Image",
]);

/** The children of a pair node that hold no visible text. */
const MARK_NODES = new Set([
  "EmphasisMark",
  "StrikethroughMark",
  "CodeMark",
  "LinkMark",
  "URL",
  "LinkTitle",
  "LinkLabel",
]);

/**
 * Per test: a whole sample's run, which is many thousand edits. The full
 * dialect pass takes about 15 s alone and several times that inside the
 * parallel suite on a busy machine, so the limit is a watchdog, not a budget.
 */
const LONG = 180_000;

afterEach(() => {
  cleanup();
  cleanupMarkViews();
});

beforeAll(async () => {
  await prefetchGrid();
  // The popover host is a lazy chunk; loaded here, the first open does not
  // race the wait below on a busy machine.
  await import("../popovers/ElementPopover");
});

interface Plan {
  /** Every run boundary and line, not a stride over them. */
  full: boolean;
  /** Where the stride starts, in steps (show mode takes one). */
  offset: number;
}

interface Probe {
  sample: Sample;
  mode: Mode;
  room: boolean;
  view: EditorView;
  /** How many transactions changed the document so far. */
  changes: () => number;
  /** The deletion outcome named since the last ask, then forgets it. */
  outcome: () => DeleteOutcome | null;
}

type Build = (text: string, options: MarkViewOptions) => EditorView;

/** A view on a sample, counting the document changes it dispatches. */
function probeOf(
  sample: Sample,
  mode: Mode,
  room: boolean,
  build: Build = markView,
): Probe {
  const options: MarkViewOptions = {
    mode,
    room,
    adapter: sample.adapter ?? "engram",
    // A solo screen picks the separator from the file; a room is LF.
    ...(!room && sample.text.includes("\r\n")
      ? { lineSeparator: "\r\n" as const }
      : {}),
  };
  const view = build(sample.text, options);
  let count = 0;
  let named: DeleteOutcome | null = null;
  view.dispatch({
    effects: StateEffect.appendConfig.of(
      EditorView.updateListener.of((update) => {
        if (update.docChanged) count += 1;
        for (const tr of update.transactions) {
          for (const effect of tr.effects) {
            if (effect.is(deletionOutcome)) named = effect.value;
          }
        }
      }),
    ),
  });
  return {
    sample,
    mode,
    room,
    view,
    changes: () => count,
    outcome: () => {
      const value = named;
      named = null;
      return value;
    },
  };
}

/**
 * The probe again, from the file's bytes, after a check left it changed: an
 * undo that did not give back the bytes is the failure already recorded, and
 * the next check starts from the file again.
 */
function rebuilt(probe: Probe): Probe {
  probe.view.destroy();
  return probeOf(probe.sample, probe.mode, probe.room);
}

/** A fixed stride over `items` that takes at most `budget` of them. */
function stride<T>(items: T[], budget: number, plan: Plan): T[] {
  if (plan.full || items.length <= budget) return items;
  const step = Math.ceil(items.length / budget);
  const start = Math.min(plan.offset, step - 1);
  return items.filter((_, i) => i % step === start);
}

/**
 * The lines a run takes for properties 1 and 4: the stride, and always every
 * line with an HTML comment (a Reflection's id line among them) with its two
 * neighbours, which hold no runs and a stride would mostly miss.
 */
function planned(state: EditorState, lines: number[], plan: Plan): number[] {
  const picked = new Set(stride(lines, LINES, plan));
  const all = new Set(lines);
  for (const n of lines) {
    if (!state.doc.line(n).text.includes("<!--") && !wraps(state, n)) continue;
    for (const m of [n - 1, n, n + 1]) if (all.has(m)) picked.add(m);
  }
  return [...picked].sort((a, b) => a - b);
}

/** The kinds a pair can be that may wrap onto the next line. */
const WRAPPING = new Set(["strong", "em", "strike", "code", "link"]);

/**
 * Whether line `n` holds one end of a pair whose other end is on another
 * line: a stride over lines and boundaries would mostly miss those, so
 * they are always taken.
 */
function wraps(state: EditorState, n: number): boolean {
  return marksOnLine(state, n, { always: true }).some(
    (mark) =>
      (mark.side === "open" || mark.side === "close") &&
      mark.partner === null &&
      WRAPPING.has(mark.kind),
  );
}

/** The lines a caret can stand on in main: every line not in a drawn table. */
function textLines(state: EditorState): number[] {
  const skip = new Set<number>();
  for (const table of gridTables(state)) {
    const first = state.doc.lineAt(table.from).number;
    const last = state.doc.lineAt(table.to).number;
    for (let n = first; n <= last; n++) skip.add(n);
  }
  const lines: number[] = [];
  for (let n = 1; n <= state.doc.lines; n++) {
    if (!skip.has(n)) lines.push(n);
  }
  return lines;
}

/** A caret on a line other than `n`, so `n`'s marks are hidden in show mode. */
function awayFrom(view: EditorView, n: number): void {
  const { state } = view;
  if (state.doc.lineAt(state.selection.main.head).number !== n) return;
  setCaret(view, n === 1 ? state.doc.length : 0);
}

interface Boundary {
  at: number;
  /** On a line with one end of a pair that wraps: always taken. */
  wraps: boolean;
  /** The mark ranges of its line, read with the caret off the line. */
  marks: [number, number][];
}

/** Every run boundary of the text lines, in document order. */
function boundariesOf(view: EditorView): Boundary[] {
  const out: Boundary[] = [];
  for (const n of textLines(view.state)) {
    awayFrom(view, n);
    const runs = runsOnLine(view.state, n);
    const marks = runs.flatMap((run) =>
      run.marks.map((mark): [number, number] => [mark.from, mark.to]),
    );
    const seen = new Set<number>();
    const wrapped = wraps(view.state, n);
    for (const run of runs) {
      for (const at of [run.from, run.to]) {
        if (!seen.has(at)) {
          seen.add(at);
          out.push({ at, marks, wraps: wrapped });
        }
      }
    }
  }
  return out;
}

/** How many characters of `pos`'s line a reader sees before `pos`. */
function visibleBefore(state: EditorState, pos: number): number {
  const line = state.doc.lineAt(pos);
  const marks = marksOnLine(state, line.number, { always: true });
  let count = 0;
  for (let p = line.from; p < pos; p += 1) {
    if (!marks.some((m) => m.from <= p && p < m.to)) count += 1;
  }
  return count;
}

/** `after` is `before` with exactly `letter` added at one place. */
function oneLetterAdded(
  before: string,
  after: string,
  letter: string,
): boolean {
  if (after.length !== before.length + letter.length) return false;
  let i = 0;
  while (i < before.length && before[i] === after[i]) i++;
  return (
    after.slice(i, i + letter.length) === letter &&
    after.slice(0, i) + after.slice(i + letter.length) === before
  );
}

interface Pair {
  name: string;
  /** The visible characters between its marks. */
  visible: number[];
  /** Its own marks: the delimiters, an address, a label. */
  marks: [number, number][];
}

/**
 * Whether a Link node is a link on the page: an inline one, or a reference
 * whose label a definition names. Lezer reads any bracketed text as a Link.
 */
function isLink(state: EditorState, node: SyntaxNodeRef): boolean {
  if (node.node.getChild("URL")) return true;
  const label = node.node.getChild("LinkLabel");
  const text = label
    ? state.doc.sliceString(label.from + 1, label.to - 1)
    : state.doc.sliceString(node.from + 1, node.to).replace(/\](\[\])?$/, "");
  return linkDefinition(state, text) !== undefined;
}

/** The pair nodes overlapping `from`..`to`, read off a finished parse. */
function pairsIn(state: EditorState, from: number, to: number): Pair[] {
  const tree = ensureSyntaxTree(state, state.doc.length, 10_000);
  if (!tree) throw new Error("the parse did not finish");
  const pairs: Pair[] = [];
  tree.iterate({
    from,
    to,
    enter: (node) => {
      if (!PAIR_NODES.has(node.name)) return;
      if (node.name === "Link" && !isLink(state, node)) return;
      const first = node.node.firstChild;
      const last = node.node.lastChild;
      const inner = {
        from: first?.to ?? node.from,
        to: last?.from ?? node.to,
      };
      const marks: [number, number][] = [];
      for (let child = node.node.firstChild; child; child = child.nextSibling) {
        if (MARK_NODES.has(child.name)) marks.push([child.from, child.to]);
      }
      // A reference with no label of its own is labelled by its words.
      if (node.name === "Link" && !node.node.getChild("URL")) {
        if (!node.node.getChild("LinkLabel"))
          marks.push([inner.from, inner.to]);
      }
      const visible: number[] = [];
      for (let p = inner.from; p < inner.to; p++) {
        if (!marks.some(([a, b]) => a <= p && p < b)) visible.push(p);
      }
      pairs.push({ name: node.name, visible, marks });
    },
  });
  return pairs;
}

function countsOf(pairs: Pair[]): Map<string, number> {
  const out = new Map<string, number>();
  for (const pair of pairs) out.set(pair.name, (out.get(pair.name) ?? 0) + 1);
  return out;
}

/** The positions of `before` an edit to `after` removed (one stretch). */
function removed(before: EditorState, after: EditorState): Set<number> {
  const a = before.doc.toString();
  const b = after.doc.toString();
  let head = 0;
  while (head < a.length && a[head] === b[head]) head++;
  let tail = 0;
  while (
    tail < a.length - head &&
    tail < b.length - head &&
    a[a.length - 1 - tail] === b[b.length - 1 - tail]
  ) {
    tail++;
  }
  const out = new Set<number>();
  for (let p = head; p < a.length - tail; p++) out.add(p);
  return out;
}

/**
 * How the pairs on the lines around `at` changed from `before` to `after`,
 * or null when the only pairs that went away are ones the edit emptied, or
 * ones whose drawn marks it removed (a mark the line shows is text the
 * person deletes; `hidden` are the marks drawn away).
 */
function pairChange(
  before: EditorState,
  after: EditorState,
  at: number,
  deleted: Set<number>,
  hidden: [number, number][],
  merged: boolean,
): string | null {
  const n = before.doc.lineAt(at).number;
  const first = Math.max(1, n - 1);
  const last = Math.min(before.doc.lines, n + 1);
  const shift = after.doc.length - before.doc.length;
  const from = before.doc.line(first).from;
  const to = before.doc.line(last).to;
  const old = pairsIn(before, from, to);
  const was = countsOf(old);
  const now = countsOf(pairsIn(after, from, Math.max(from, to + shift)));
  const problems: string[] = [];
  let mergeLeft = 1;
  for (const name of new Set([...was.keys(), ...now.keys()])) {
    const x = was.get(name) ?? 0;
    const y = now.get(name) ?? 0;
    const shownMarkDeleted = (pair: Pair) =>
      [...deleted].some(
        (p) =>
          pair.marks.some(([x, y]) => x <= p && p < y) &&
          !hidden.some(([x, y]) => x <= p && p < y),
      );
    const emptied = old.filter(
      (pair) =>
        pair.name === name &&
        ((pair.visible.length > 0 &&
          pair.visible.every((p) => deleted.has(p))) ||
          shownMarkDeleted(pair)),
    ).length;
    if (y > x) problems.push(`${name} ${x} -> ${y}`);
    // A merge writes two pairs of one kind as one: one pair in all.
    else if (x - y > emptied) {
      const over = x - y - emptied;
      if (merged && mergeLeft === 1 && over === 1) mergeLeft = 0;
      else problems.push(`${name} ${x} -> ${y}`);
    }
  }
  return problems.length > 0 ? problems.join(", ") : null;
}

/** The line `at` is on, with a bar at `at`, for a failure message. */
function where(state: EditorState, at: number): string {
  const line = state.doc.lineAt(at);
  const col = at - line.from;
  return `line ${line.number}: ${JSON.stringify(
    `${line.text.slice(0, col)}|${line.text.slice(col)}`,
  )}`;
}

interface Tally {
  checks: number;
  failures: string[];
  /** Backspace and Delete presses judged, by how they went. */
  deletes: Record<DeleteOutcome | "exact", number>;
}

function tallyOf(): Tally {
  return {
    checks: 0,
    failures: [],
    deletes: { exact: 0, moved: 0, merged: 0, refused: 0 },
  };
}

function fail(tally: Tally, what: string): void {
  if (tally.failures.length < 60) tally.failures.push(what);
}

/** Property 1: moving, selecting and flipping the setting write nothing. */
function moving(probe: Probe, plan: Plan, tally: Tally): void {
  const { view, changes, sample } = probe;
  const start = changes();
  for (const n of planned(view.state, textLines(view.state), plan)) {
    awayFrom(view, n);
    const line = view.state.doc.line(n);
    const stops = new Set([line.from, line.to]);
    for (const run of runsOnLine(view.state, n)) {
      stops.add(run.from);
      stops.add(run.to);
    }
    for (const stop of stops) {
      // A click, a keyboard selection and the stop's own drawn place.
      setCaret(view, stop);
      setCaret(view, stop, "select");
      setCaret(view, canonical(view.state, stop));
      tally.checks += 3;
    }
    setCaret(view, line.from);
    keys(view, "ArrowRight", "End", "ArrowLeft", "Home", "ArrowDown");
    setCaret(view, line.to);
    keys(view, "Shift-ArrowLeft", "ArrowLeft", "ArrowUp");
    view.dispatch({
      selection: EditorSelection.range(line.from, line.to),
      userEvent: "select.pointer",
    });
    tally.checks += 9;
  }
  // The setting flipped twice while the last line is selected.
  const show = showsMarks(view);
  setShowMarks(view, !show);
  setShowMarks(view, show);
  tally.checks += 1;
  if (changes() !== start || bytes(view.state) !== sample.text) {
    fail(tally, `moving dispatched ${changes() - start} document changes`);
  }
}

/** Properties 2 and 3 at the planned run boundaries. Returns the probe it left. */
function boundaryEdits(start: Probe, plan: Plan, tally: Tally): Probe {
  let probe = start;
  const text = probe.sample.text;
  const show = probe.mode === "show";
  const seen = new Set<number>();
  const all = boundariesOf(probe.view);
  const picked = new Set(stride(all, BOUNDARIES, plan));
  for (const boundary of all) if (boundary.wraps) picked.add(boundary);
  for (const { at: b, marks } of all.filter((each) => picked.has(each))) {
    const { view } = probe;
    setCaret(view, b);
    const at = view.state.selection.main.head;
    if (seen.has(at)) continue;
    seen.add(at);
    const before = view.state;
    // Property 2: one letter, one undo.
    typeText(view, LETTER);
    tally.checks += 1;
    if (!oneLetterAdded(text, bytes(view.state), LETTER)) {
      fail(
        tally,
        `typing at ${where(before, at)} wrote ${where(view.state, view.state.selection.main.head)}`,
      );
    }
    undoOnce(view);
    if (bytes(view.state) !== text) {
      fail(tally, `undo after typing at ${where(before, at)}`);
      probe = rebuilt(probe);
      continue;
    }
    // Property 3: Backspace and Delete keep the pairs they do not empty.
    for (const key of ["Backspace", "Delete"]) {
      const { view: now } = probe;
      setCaret(now, b);
      const from = now.state;
      probe.outcome();
      keys(now, key);
      const outcome = probe.outcome();
      if (outcome === "refused") {
        // Refused: nothing written, and the page says so.
        tally.deletes.refused += 1;
        if (bytes(now.state) !== text) {
          fail(tally, `a refused ${key} at ${where(from, at)} wrote`);
          probe = rebuilt(probe);
        }
        continue;
      }
      if (bytes(now.state) === text) {
        // Nothing written and nothing said: a silent refusal.
        fail(tally, `a silent ${key} at ${where(from, at)}`);
        continue;
      }
      tally.deletes[outcome ?? "exact"] += 1;
      if (outcome === "moved") {
        // The caret shows where the exact deletion would have shown it.
        const want = visibleBefore(from, at) - (key === "Backspace" ? 1 : 0);
        const head = now.state.selection.main.head;
        const got = visibleBefore(now.state, head);
        if (got !== want) {
          fail(
            tally,
            `${key} at ${where(from, at)} moved a mark and left the caret at visible ${String(got)}, not ${String(want)}: ${where(now.state, head)}`,
          );
        }
      }
      const deleted = removed(from, now.state);
      const touchesMark = [...deleted].some((p) =>
        marks.some(([x, y]) => x <= p && p < y),
      );
      // In show mode the marks are text the keys remove like any other.
      if (!(show && touchesMark)) {
        tally.checks += 1;
        const change = pairChange(
          from,
          now.state,
          at,
          deleted,
          marks,
          outcome === "merged",
        );
        if (change !== null) {
          fail(
            tally,
            `${key} at ${where(from, at)}: ${change}, wrote ${where(now.state, now.state.selection.main.head)}`,
          );
        }
      }
      undoOnce(now);
      if (bytes(now.state) !== text) {
        fail(tally, `undo after ${key} at ${where(from, at)}`);
        probe = rebuilt(probe);
      }
    }
  }
  return probe;
}

/** Whether line `n` holds no bold at all. */
function noBold(state: EditorState, n: number): boolean {
  const line = state.doc.line(n);
  return !pairsIn(state, line.from, line.to).some(
    (pair) => pair.name === "StrongEmphasis",
  );
}

/** Property 4 on the planned lines. Returns the probe it left. */
function bolding(start: Probe, plan: Plan, tally: Tally): Probe {
  let probe = start;
  const text = probe.sample.text;
  const lines = textLines(probe.view.state).filter(
    (n) => probe.view.state.doc.line(n).text.trim() !== "",
  );
  for (const n of planned(probe.view.state, lines, plan)) {
    const { view } = probe;
    const line = view.state.doc.line(n);
    const select = () => {
      view.dispatch({
        selection: EditorSelection.range(line.from, line.to),
        userEvent: "select.pointer",
      });
    };
    const plain = noBold(view.state, n);
    select();
    toggleFormat(view, "strong");
    tally.checks += 1;
    if (bytes(view.state) !== text) {
      undoOnce(view);
      if (bytes(view.state) !== text) {
        fail(tally, `Bold then undo on ${where(view.state, line.from)}`);
        probe = rebuilt(probe);
        continue;
      }
    }
    if (!plain) continue;
    select();
    toggleFormat(view, "strong");
    if (bytes(view.state) === text) continue;
    toggleFormat(view, "strong");
    tally.checks += 1;
    if (bytes(view.state) !== text) {
      fail(
        tally,
        `Bold twice on ${JSON.stringify(line.text)} gave ${JSON.stringify(view.state.doc.lineAt(line.from).text)}`,
      );
      probe = rebuilt(probe);
    }
  }
  return probe;
}

/** Properties 1 to 4 on one sample in one mode. */
function runAll(probe: Probe, plan: Plan): Tally {
  const tally = tallyOf();
  moving(probe, plan, tally);
  const after = boundaryEdits(probe, plan, tally);
  bolding(after, plan, tally);
  return tally;
}

interface Run {
  tally: Tally;
  lines: number;
}

/**
 * Each sample's run in one mode, solo or in a room, computed once and kept:
 * the test for that sample reads it, and so does the tally, whichever of
 * them runs first (or alone).
 */
const runs = new Map<string, Run>();

function runOf(sample: Sample, mode: Mode, room: boolean): Run {
  const key = `${sample.name}|${mode}|${String(room)}`;
  let run = runs.get(key);
  if (!run) {
    const probe = probeOf(sample, mode, room);
    const lines = probe.view.state.doc.lines;
    run = { tally: runAll(probe, planFor(sample, mode, room)), lines };
    runs.set(key, run);
    cleanupMarkViews();
  }
  return run;
}

const SOLO: Sample[] = [...FIXTURES, ...CORPUS];

function roomSamples(): Sample[] {
  return [
    LF,
    ...ROOM.map((name) => {
      const sample = CORPUS.find((each) => each.name === name);
      if (!sample) throw new Error(`the corpus has no ${name}`);
      return sample;
    }),
  ];
}

function planFor(sample: Sample, mode: Mode, room: boolean): Plan {
  return {
    full: sample === LF && mode === "hidden" && !room,
    offset: mode === "show" ? 1 : 0,
  };
}

describe.each(MODES)("properties 1 to 4, %s mode, solo", (mode) => {
  it.each(SOLO.map((s) => [s.name, s] as const))(
    "%s",
    { timeout: LONG },
    (_name, sample) => {
      const { tally } = runOf(sample, mode, false);
      expect(tally.failures).toEqual([]);
      expect(tally.checks).toBeGreaterThan(0);
    },
  );
});

describe.each(MODES)("properties 1 to 4, %s mode, in a room", (mode) => {
  it.each(roomSamples().map((s) => [s.name, s] as const))(
    "%s",
    { timeout: LONG },
    (_name, sample) => {
      const { tally } = runOf(sample, mode, true);
      expect(tally.failures).toEqual([]);
      expect(tally.checks).toBeGreaterThan(0);
    },
  );
});

// Property 5.

/** Every element a popover edits in the view, once each, in document order. */
function elementsOf(view: EditorView): (ElementEdit & { at: number })[] {
  const rect = new DOMRect();
  const seen = new Set<string>();
  const out: (ElementEdit & { at: number })[] = [];
  const { doc } = view.state;
  for (let pos = 0; pos <= doc.length; pos++) {
    const edit = elementEditAt(view, pos, rect);
    if (!edit || edit.kind === "new-link") continue;
    const key = `${edit.kind}@${edit.at}`;
    if (seen.has(key)) continue;
    seen.add(key);
    // A code box's actions write a paragraph, never an unchanged value; its
    // header's language menu is what edits the box.
    out.push(
      edit.kind === "code-actions"
        ? { kind: "language", at: edit.at, rect }
        : edit,
    );
  }
  return out;
}

const MENUS = new Set(["task", "callout-kind", "language"]);

/** The popover for `edit` on `view`, as a click or Shift+F10 opens it. */
async function open(view: EditorView, edit: ElementEdit): Promise<HTMLElement> {
  act(() => {
    requestElementEdit(view, edit);
  });
  return screen.findByRole(
    MENUS.has(edit.kind) ? "menu" : "dialog",
    {},
    { timeout: 10_000 },
  );
}

/** The checked items of an open menu, by name. */
function checkedNames(menu: HTMLElement): string[] {
  return [
    ...menu.querySelectorAll<HTMLElement>(
      '[role="menuitemradio"][aria-checked="true"]',
    ),
  ].map((item) => item.textContent);
}

/** What the elements of the dialect file are, to show none is missing. */
function shapesOf(
  view: EditorView,
  edits: (ElementEdit & { at: number })[],
): string[] {
  const { state } = view;
  const shapes = new Set<string>();
  for (const edit of edits) {
    if (edit.kind === "math") {
      shapes.add(`math ${mathAt(state, edit.at)?.form ?? "?"}`);
    } else if (edit.kind === "language") {
      const fence = codeFenceAt(state, edit.at);
      const head = state.doc.lineAt(edit.at).text;
      const word = fence
        ? state.doc.sliceString(fence.word.from, fence.word.to)
        : "";
      shapes.add(`fence ${/^\s*~/.test(head) ? "tilde" : "backtick"}`);
      if (/^ \S/.test(head)) shapes.add("fence indented");
      if (/^ {2}\S/.test(head)) shapes.add("fence in a list");
      if (word === "") shapes.add("fence without a language");
    } else {
      shapes.add(edit.kind);
    }
  }
  return [...shapes].sort();
}

describe.each([
  ...MODES.flatMap((mode) =>
    [false, true].map((room) => ({ mode, room, sample: LF })),
  ),
  ...MODES.map((mode) => ({ mode, room: false, sample: CRLF })),
])(
  "property 5, $sample.name, $mode mode, room $room",
  ({ mode, room, sample }) => {
    it(
      "every element editor committed unchanged writes nothing",
      { timeout: LONG },
      async () => {
        const user = userEvent.setup();
        const { view, changes } = probeOf(sample, mode, room, popoverView);
        const edits = elementsOf(view);
        expect(shapesOf(view, edits)).toEqual(
          [
            "callout-kind",
            "fence backtick",
            "fence in a list",
            "fence indented",
            "fence tilde",
            "fence without a language",
            "link",
            "math code",
            "math dollars",
            "math fence",
            "math inline",
            "status",
            "task",
            "wikilink",
          ].sort(),
        );
        const failures: string[] = [];
        let commits = 0;
        for (const edit of edits) {
          const count = changes();
          const depth = undoSteps(view);
          const check = (what: string) => {
            commits += 1;
            if (
              bytes(view.state) !== sample.text ||
              changes() !== count ||
              undoSteps(view) !== depth
            ) {
              failures.push(
                `${edit.kind} ${what} at ${where(view.state, edit.at)}`,
              );
            }
            expect(openElementEdit(view)).toBeNull();
          };
          const popover = await open(view, edit);
          if (!MENUS.has(edit.kind)) {
            const save = popover.querySelector<HTMLButtonElement>(
              'button[type="submit"]',
            );
            if (save !== null) {
              expect(save).toBeEnabled();
              await user.click(save);
              check("Save");
              continue;
            }
            // A reference link has no Save: Enter in its field commits.
            const field = popover.querySelector<HTMLInputElement>("input");
            if (field === null) throw new Error(`${edit.kind} has no field`);
            field.focus();
            await user.keyboard("{Enter}");
            check("Enter");
            continue;
          }
          const names = checkedNames(popover);
          expect(names.length).toBeGreaterThan(0);
          let menu = popover;
          if (edit.kind === "language") {
            // An empty filter and Enter picks the checked language.
            await user.keyboard("{Enter}");
            check("Enter");
            menu = await open(view, edit);
          }
          for (const [index, name] of names.entries()) {
            if (index > 0) menu = await open(view, edit);
            const item = [
              ...menu.querySelectorAll<HTMLElement>(
                '[role="menuitemradio"][aria-checked="true"]',
              ),
            ].find((each) => each.textContent === name);
            if (!item) throw new Error(`${edit.kind} lost its checked ${name}`);
            await user.click(item);
            check(`pick of the checked ${name}`);
          }
        }
        expect(failures).toEqual([]);
        expect(commits).toBeGreaterThanOrEqual(edits.length);
      },
    );
  },
);

it(
  "counts what it checked, and refuses few deletions",
  { timeout: 4 * LONG },
  () => {
    // Every run, read from the ones the tests above kept or computed here.
    const totals = {
      files: new Set<string>(),
      lines: 0,
      checks: 0,
      deletes: { exact: 0, moved: 0, merged: 0, refused: 0 },
    };
    const all = [
      ...MODES.flatMap((mode) =>
        SOLO.map((sample) => ({ sample, mode, room: false })),
      ),
      ...MODES.flatMap((mode) =>
        roomSamples().map((sample) => ({ sample, mode, room: true })),
      ),
    ];
    for (const { sample, mode, room } of all) {
      const run = runOf(sample, mode, room);
      if (!totals.files.has(sample.name)) {
        totals.files.add(sample.name);
        totals.lines += run.lines;
      }
      totals.checks += run.tally.checks;
      for (const [key, value] of Object.entries(run.tally.deletes)) {
        totals.deletes[key as keyof Tally["deletes"]] += value;
      }
    }
    expect(totals.files.size).toBeGreaterThan(80);
    const { exact, moved, merged, refused } = totals.deletes;
    const judged = exact + moved + merged + refused;
    console.info(
      `fidelity corpus: ${totals.files.size} files, ${totals.lines} lines, ${totals.checks} checks; deletes: ${exact} exact, ${moved} moved, ${merged} merged, ${refused} refused`,
    );
    expect(judged).toBeGreaterThan(500);
    // A check that refused every deletion would pass each case alone.
    expect(refused / judged).toBeLessThanOrEqual(MAX_REFUSED);
  },
);

describe.each(MODES)("property 3 pins, %s mode", (mode) => {
  /** One press in the dialect file: the outcome and the line it leaves. */
  function press(text: string, after: string, key: string) {
    const probe = probeOf(LF, mode, false);
    const at = EVERYTHING.indexOf(text) + text.indexOf(after) + after.length;
    expect(at).toBeGreaterThan(after.length);
    setCaret(probe.view, at);
    probe.outcome();
    keys(probe.view, key);
    const outcome = probe.outcome();
    const line = probe.view.state.doc.lineAt(
      Math.min(at, probe.view.state.doc.length),
    ).text;
    return { outcome, line, bytes: bytes(probe.view.state) };
  }

  it("(a) moves the mark across the space Delete leaves at its edge", () => {
    const { outcome, line } = press("**A bold**", "**", "Delete");
    expect(outcome).toBe("moved");
    expect(line).toContain("Spaced:  **bold** word");
  });

  it("(b) merges two bold words whose space Delete takes", () => {
    // In show mode the closing marks are text: the caret goes past them.
    const after = mode === "show" ? "**one**" : "**one";
    const { outcome, line } = press("**one** **two**", after, "Delete");
    expect(outcome).toBe("merged");
    expect(line).toContain("**onetwo**");
  });

  it("refuses a deletion that would make a new pair where marks are hidden", () => {
    const { outcome, bytes: after } = press("q * a* r", "q * ", "Backspace");
    if (mode === "hidden") {
      expect(outcome).toBe("refused");
      expect(after).toBe(EVERYTHING);
    } else {
      // Where the line shows its marks, a line with no pair keeps today's
      // keys, though its paragraph holds pairs on the lines around it (D11).
      expect(outcome).toBeNull();
      expect(after).toBe(EVERYTHING.replace("q * a* r", "q *a* r"));
    }
  });

  it("refuses a new pair that reaches a line whose marks are hidden, in both modes (D23)", () => {
    // `*a r\nb*` would pair. In show mode the caret's line shows the
    // opener, but the closer's line hides its marks.
    const { outcome, bytes: after } = press(
      "wraps: q * a r\nb* c",
      "q * ",
      "Backspace",
    );
    expect(outcome).toBe("refused");
    expect(after).toBe(EVERYTHING);
  });

  it("refuses a word delete that takes a mark and a space and makes a pair onto a hidden line, in both modes (D28)", () => {
    // The space and the backtick after `q *` go: `*a\` r\nb*` would pair.
    const { outcome, bytes: after } = press(
      "code star that wraps: q * `a` r\nb* c",
      "q *",
      "Mod-Delete",
    );
    expect(outcome).toBe("refused");
    expect(after).toBe(EVERYTHING);
  });

  it("lets a shown mark go in show mode, though its partner's line hides its marks (D25)", () => {
    if (mode === "hidden") return;
    // The caret's line shows the opener of a bold that wraps: Backspace
    // takes one star of it, as typed (D9).
    const { outcome, bytes: after } = press(
      "Wrapped: **a lead-in",
      "Wrapped: **",
      "Backspace",
    );
    expect(outcome).toBeNull();
    expect(after).toBe(
      EVERYTHING.replace("Wrapped: **a lead-in", "Wrapped: *a lead-in"),
    );
  });

  it("(c) refuses across a line break too", () => {
    const { outcome, bytes: after } = press(
      "wraps\nhere.** text",
      "here.**",
      "Delete",
    );
    expect(outcome).toBe("refused");
    expect(after).toBe(EVERYTHING);
  });

  it("(a) moves a mark across a line break pair's space", () => {
    const { outcome, bytes: after } = press(
      "**A spread\nbold**",
      "**",
      "Delete",
    );
    expect(outcome).toBe("moved");
    expect(after).toContain("words and  **spread\nbold** end.");
  });

  it("(b) merges a pair that wraps with the next one", () => {
    const after = mode === "show" ? "more**" : "more";
    const { outcome, bytes: written } = press(
      "**one\nmore** **two**",
      after,
      "Delete",
    );
    expect(outcome).toBe("merged");
    expect(written).toContain("**one\nmoretwo** words");
  });

  it("(c) refuses the space after a closer that follows punctuation", () => {
    const { outcome, bytes: after } = press(
      "**Lead-in.** text",
      "**Lead-in.**",
      "Delete",
    );
    expect(outcome).toBe("refused");
    expect(after).toBe(EVERYTHING);
  });
});
