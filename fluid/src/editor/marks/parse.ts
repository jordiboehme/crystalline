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
 * A key that deletes holds more than one deadline: the key's own parse and
 * the deletion check each have one, and in a cell so does the guard of an
 * unread row. Only a press that is refused for want of a parse spends them all.
 */

import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
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
 * so a press wants the whole document (D35). After an edit that is one step,
 * as the rest of the tree is reused; on a page whose first parse is still
 * running it is not, and the press does not wait for that: it reads what the
 * page draws.
 */
const REST_OF_PAGE_MS = 10;

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

/**
 * Parses `state` to `upto` and, with what `deadline` allows, on to the end of
 * the document: true when the parse context got anywhere new. The tree stays
 * in the parse context the state shares with the view.
 */
function advance(
  state: EditorState,
  upto: number,
  deadline: Deadline,
): boolean {
  const end = state.doc.length;
  const held = syntaxTree(state).length;
  let advanced = false;
  if (held < upto) {
    const began = Date.now();
    advanced =
      ensureSyntaxTree(state, upto, Math.max(0, deadline.left)) !== null;
    deadline.left -= Math.max(0, Date.now() - began);
    if (!advanced) return false;
  }
  if (held < end && deadline.rest > 0) {
    const began = Date.now();
    const whole = ensureSyntaxTree(
      state,
      end,
      Math.max(0, Math.min(deadline.rest, deadline.left)),
    );
    const spent = Math.max(0, Date.now() - began);
    deadline.rest -= spent;
    deadline.left -= spent;
    advanced ||= whole !== null;
  }
  return advanced;
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
  if (!advance(state, upto, deadline)) {
    return syntaxTree(state).length >= upto ? state : null;
  }
  const settled = state.update({}).state;
  return syntaxTree(settled).length >= upto ? settled : null;
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
  if (advance(view.state, upto, deadline)) {
    view.dispatch({});
  }
  return syntaxTree(view.state).length >= upto;
}
