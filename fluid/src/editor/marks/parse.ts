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
 */

import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";

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
 * What is left of one press's parse time. Only time spent parsing counts
 * (D52): the scans, the reads and a pause of the page between them never
 * use it up, so a press on a page that is parsed already is never refused.
 */
export interface Deadline {
  left: number;
}

/** The parse time of a press that starts now with `budget` milliseconds. */
export function deadlineIn(budget: number): Deadline {
  return { left: budget };
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
  if (syntaxTree(state).length >= upto) {
    return state;
  }
  const began = Date.now();
  ensureSyntaxTree(state, upto, Math.max(0, deadline.left));
  deadline.left -= Math.max(0, Date.now() - began);
  const settled = state.update({}).state;
  return syntaxTree(settled).length >= upto ? settled : null;
}
