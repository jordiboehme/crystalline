/**
 * A state whose whole document is parsed, for tests that read the syntax tree.
 *
 * What this compensates for is a wall-clock budget inside
 * `@codemirror/language`: `LanguageState.init` gives the first parse of a new
 * state 20 milliseconds of `Date.now()` and, when that runs out, calls
 * `takeTree()` and keeps whatever was finished. The rest of the document is
 * parsed later by the `parseWorker` plugin in a `requestIdleCallback`
 * pseudo-thread, and a selection-only transaction does not advance it either
 * (`LanguageState.apply` returns the same value when nothing changed). A test
 * that builds a state and asserts on the same tick therefore asserts against
 * however much of the tree happened to fit in those 20 milliseconds - which on
 * a loaded machine is a coin toss, and measured as one: the same 69-character
 * fixture came out parsed to 21, 32, 55 or all 69 characters across repeated
 * runs, and every short tree was a failed assertion.
 *
 * The budget is wall clock rather than work, so the honest fix is to remove it
 * rather than to widen it: `Number.POSITIVE_INFINITY` below says "parse all of
 * it" where a bigger number would only say "be luckier". A `waitFor` or a retry
 * around the assertion would be papering over - the parse these tests need is
 * not asynchronous work they are racing, it is work nobody has asked for yet,
 * and letting the event loop turn would only hand it to the idle worker by
 * accident. Asking for it outright is what makes the assertion mean what it
 * says.
 *
 * Ask for it BEFORE the view is built. A decoration plugin reads the tree in
 * its constructor, and the plugins here rebuild on a document, selection or
 * viewport change rather than on parse progress, so a tree that grows after
 * mounting does not redraw anything by itself.
 */

import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";

/** No budget at all, which is the point. See the note above. */
const NO_BUDGET = Number.POSITIVE_INFINITY;

/**
 * How far the tree reaches, which is the only thing the callers depend on.
 *
 * Deliberately not `syntaxTreeAvailable`: that also demands a fragment set
 * spanning the document, and a fence whose nested language is still being
 * imported leaves the parse covering every character with no such fragment.
 * The decorations read node positions, so covering every character is the bar.
 */
function covered(state: EditorState): boolean {
  return syntaxTree(state).length >= state.doc.length;
}

export function parsedState(state: EditorState): EditorState {
  if (covered(state)) {
    return state;
  }
  ensureSyntaxTree(state, state.doc.length, NO_BUDGET);
  // The empty update is what publishes the advanced parse into the state
  // field: `ensureSyntaxTree` moves the parse context on, but `syntaxTree`
  // reads the snapshot the field is holding, which only a transaction renews.
  const settled = state.update({}).state;
  if (!covered(settled)) {
    throw new Error(
      `the parse stopped at ${syntaxTree(settled).length} of ${settled.doc.length} characters`,
    );
  }
  return settled;
}
