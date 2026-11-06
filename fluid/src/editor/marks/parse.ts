/**
 * The one parse the format commands, the typing rules and the deletion check
 * wait for (D35). A press gets one parse budget, and every parse it reads
 * (the start state's, then each trial's) takes only what is left of it, so
 * one press never parses longer than its bound in total, however many trials
 * it reads.
 *
 * The bounds, per press:
 *
 * - typing: 50 ms. On a timeout the insert goes in as typed, unverified, so
 *   nothing is refused;
 * - a format command on a selection: 500 ms;
 * - a selection deleted or cut: 500 ms, single gestures;
 * - Backspace, Delete and their word and line forms: 150 ms, as they repeat.
 *
 * Past its bound a command or a deletion is refused with `PAGE_LOADING`,
 * never written unchecked: a delete that touches no pair can still make one.
 *
 * The filters that judge one edit share one deadline (`pressDeadline`). A key
 * that deletes holds two: its own parse before it reads the page, and that
 * one. Only a press that is refused for want of a parse spends them both.
 */

import { Language, ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import type { EditorState, StateField } from "@codemirror/state";
import type { Tree } from "@lezer/common";
import type { EditorView } from "@codemirror/view";

/** What a press refused for want of a parse says. */
export const PAGE_LOADING =
  "This page is still loading. Try again in a moment.";

/** The bounds of one press, in milliseconds. */
export const PARSE_BUDGET = {
  typing: 50,
  command: 500,
  gesture: 500,
  key: 150,
} as const;

/**
 * The parse time past the lines a press asks about, per press. A line's
 * reference links read their definitions, which can stand anywhere below it,
 * so a press on such a block wants the whole document (D35). After an edit
 * that is one step, as the rest of the tree is reused; on a page whose first
 * parse is still running it is not, and the press does not wait for that: it
 * reads what the page draws. The next edit drops what this parse did not
 * finish, so a press on any other block spends none of it (`readsBelow`).
 */
const REST_OF_PAGE_MS = 10;

/** The lines of a block looked at either way; a longer one counts as asking. */
const BLOCK_LINES = 200;

/**
 * Brackets that can be a reference: a pair not followed by `(` or `]`, which
 * leaves out an inline link, a wikilink's and an image's address. A task box
 * at a line's start is taken out first, so a list does not ask on every line.
 */
const REFERENCE = /\[[^\]]*\](?![(\]])/;
const TASK_BOX = /^\s*(?:[-*+]|\d+[.)])\s+\[.\]/gm;

/**
 * Whether the block around `upto` holds text whose meaning stands below it:
 * a reference link and its definition. Nothing else a press reads does, so
 * this is what the rest of the page is parsed for.
 */
function readsBelow(state: EditorState, upto: number): boolean {
  const { doc } = state;
  const at = doc.lineAt(Math.min(upto, doc.length)).number;
  const lines: string[] = [];
  for (const step of [-1, 1]) {
    for (let n = step < 0 ? at : at + 1, seen = 0; ; n += step, seen += 1) {
      if (n < 1 || n > doc.lines) break;
      if (seen >= BLOCK_LINES) return true;
      const { text } = doc.line(n);
      if (text.trim() === "") break;
      lines.push(text);
    }
    if (step < 0) lines.reverse();
  }
  return REFERENCE.test(lines.join("\n").replace(TASK_BOX, ""));
}

/**
 * What is left of one press's parse time. Only time spent parsing counts
 * (D52): the scans, the reads and a pause of the page between them never
 * use it up, so a press on a page that is parsed already is never refused.
 * `rest` is the part of it that may go past the lines asked about. Typing
 * has none, so a letter never waits for lines it does not touch.
 */
export interface Deadline {
  left: number;
  rest: number;
}

/** The parse time of a press that starts now with `budget` milliseconds. */
export function deadlineIn(budget: number): Deadline {
  return {
    left: budget,
    rest: budget > PARSE_BUDGET.typing ? REST_OF_PAGE_MS : 0,
  };
}

const pressDeadlines = new WeakMap<EditorState, Deadline>();

/**
 * The one deadline of the filters that judge one edit, which all start from
 * the same state: the guard of a cell's row, `keepPartnerMarks` and the deletion
 * check. A refused edit still makes a new state, so the next press gets its own.
 */
export function pressDeadline(state: EditorState, budget: number): Deadline {
  let deadline = pressDeadlines.get(state);
  if (!deadline) {
    deadline = deadlineIn(budget);
    pressDeadlines.set(state, deadline);
  }
  return deadline;
}

/** The bound of one press that writes an edit of `event` (D35). */
export function budgetOf(event: string): number {
  return event === "input.type" ||
    event === "delete.backward" ||
    event === "delete.forward"
    ? PARSE_BUDGET.key
    : PARSE_BUDGET.gesture;
}

/**
 * The parse a state holds. `treeLen` is how far it finished, which is the one
 * thing that says what is read: a cut tree's `length` overstates it (`advance`),
 * and `ensureSyntaxTree` calls a page with a fence whose language is still
 * loading never done (no fragment spans the fence), so it would parse again
 * on each key. Not public API in @codemirror/language 6.13.1; a test pins it
 * (`pressParse.test.ts`), as other tests pin the same version's clock (D26).
 */
interface HeldParse {
  context: { treeLen: number; tree: Tree };
}
const languageState = (Language as unknown as { state: StateField<HeldParse> })
  .state;

/** The tree `state`'s parse finished to `upto`, with no parse time spent, or null. */
function readTo(state: EditorState, upto: number): Tree | null {
  const held = state.field(languageState, false)?.context;
  return held && held.treeLen >= upto ? held.tree : null;
}

/** Whether `state` is read to `upto`. Spends nothing, so a filter can ask. */
export function isReadTo(state: EditorState, upto: number): boolean {
  return readTo(state, upto) !== null;
}

/**
 * The parse steps a press takes before its clock counts. A step is one block,
 * so a line a few blocks away is read at any load: a pause of the page must not
 * cost a press on a small page its format (D52), and a long way off is still a
 * long way off.
 */
const FREE_STEPS = 16;

/** `ensureSyntaxTree` on the press's clock, when the parse is not there yet. */
function timed(
  state: EditorState,
  upto: number,
  deadline: Deadline,
  most: number,
): Tree | null {
  const done = readTo(state, upto);
  if (done) return done;
  const began = Date.now();
  const tree = ensureSyntaxTree(state, upto, Math.max(0, most));
  const spent = Math.max(0, Date.now() - began);
  deadline.left -= spent;
  deadline.rest -= spent;
  return tree;
}

/** The first `FREE_STEPS` steps towards `upto`, or null when they fall short. */
function freeSteps(state: EditorState, upto: number): Tree | null {
  for (let step = 0; step < FREE_STEPS; step += 1) {
    const tree = ensureSyntaxTree(state, upto, 0);
    if (tree) return tree;
  }
  return null;
}

/**
 * Parses `state` to `upto` and, where the block asks and `deadline` allows, on
 * to the end of the document: the tree that reaches `upto`, or null when the
 * time ran out first. The work stays in the parse context the state shares
 * with the view.
 *
 * This asks the parse context and never `tree.length`, which overstates a cut
 * parse: a tree cut inside a quote or a list is as long as the blocks it closed,
 * with the lines past the cut unread. A trial cut like that read the next
 * line's markers as absent and refused every place for Enter, so `**` went
 * onto the new line.
 */
function advance(
  state: EditorState,
  upto: number,
  deadline: Deadline,
): Tree | null {
  const reached =
    readTo(state, upto) ??
    freeSteps(state, upto) ??
    timed(state, upto, deadline, deadline.left);
  if (!reached) return null;
  if (deadline.rest <= 0 || !readsBelow(state, upto)) return reached;
  const whole = timed(
    state,
    state.doc.length,
    deadline,
    Math.min(deadline.rest, deadline.left),
  );
  return whole ?? reached;
}

/**
 * `state` parsed at least to `upto`, published as a state of its own when
 * the parse had to go further, or null when the parse time left in
 * `deadline` did not reach that far: the syntax tree stops short of the
 * lines asked about, and only then does a press say `PAGE_LOADING`. The
 * parse work is kept in the parse context the state shares with the view,
 * so the next press does not do it again.
 */
export function parsedTo(
  state: EditorState,
  upto: number,
  deadline: Deadline,
): EditorState | null {
  const tree = advance(state, upto, deadline);
  if (!tree) return null;
  return tree === syntaxTree(state) ? state : state.update({}).state;
}

/**
 * `view`'s own tree parsed to `upto` at the start of a press, published into
 * the view when the parse went further: true when it reaches. A reparse after
 * a document change stops after 20 ms and keeps what it finished, so a pause
 * of the page, a slow phone or a long engram leave the view's tree short until
 * the background parse catches up. Whatever a press reads from the view then
 * (the drawn atoms, a cell's "not read yet" gate) has to see the parse too,
 * which a state of its own (`parsedTo`) does not give it. Never inside an
 * update.
 */
export function viewParsedTo(
  view: EditorView,
  upto: number,
  deadline: Deadline,
): boolean {
  const tree = advance(view.state, upto, deadline);
  if (!tree) return false;
  if (tree !== syntaxTree(view.state)) view.dispatch({});
  return true;
}
