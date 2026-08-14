/**
 * A finished parse, for tests that read the syntax tree.
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
 * A `waitFor` or a retry around the assertion would be papering over. The parse
 * these tests need is not asynchronous work they are racing, it is work nobody
 * has asked for yet, and letting the event loop turn would only hand it to the
 * idle worker by accident. Asking for it outright is what makes the assertion
 * mean what it says.
 *
 * Two shapes, because tests come in two:
 *
 * - `parsedState` for a test that builds the state itself, which is most of
 *   them. Parsing before the view is built means every layer sees a whole tree
 *   in its constructor.
 * - `parsedView` for a test that only ever gets a mounted view - a React screen
 *   the test did not construct the buffer for. It is also the app's own path:
 *   advance the parse under a live view, publish it, and let the decoration
 *   layers redraw off `parseAdvanced`.
 */

import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

/**
 * The wall clock this allows the parse, which is a watchdog and not a
 * tolerance.
 *
 * Thirty seconds against fixtures that parse in well under a millisecond is
 * four orders of magnitude of headroom, where the 20 milliseconds this works
 * around is the same order as the work itself - that difference is the whole
 * point, not a bigger number in the same game. It is finite rather than
 * `Infinity` deliberately, and the trade is worth naming: infinity would make a
 * parse that stopped making progress hang the worker forever, and a
 * synchronous hang is the one failure a test runner cannot interrupt or report.
 * A finite deadline turns that same case into the throw below, which names how
 * far the parse got. What it costs is that a machine starved past all
 * plausibility would fail the run rather than survive it - loudly, at this
 * line, which is the outcome to want.
 */
const PARSE_DEADLINE_MS = 30_000;

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

function refuse(state: EditorState): never {
  throw new Error(
    `the parse stopped at ${syntaxTree(state).length} of ${state.doc.length} characters`,
  );
}

export function parsedState(state: EditorState): EditorState {
  if (covered(state)) {
    return state;
  }
  ensureSyntaxTree(state, state.doc.length, PARSE_DEADLINE_MS);
  // The empty update is what publishes the advanced parse into the state
  // field: `ensureSyntaxTree` moves the parse context on, but `syntaxTree`
  // reads the snapshot the field is holding, which only a transaction renews.
  const settled = state.update({}).state;
  return covered(settled) ? settled : refuse(settled);
}

/**
 * The same for a view that is already mounted, which is the shape the
 * `parseWorker` itself uses: advance, then publish through a transaction that
 * changes no document, no selection and no viewport. Every tree-driven layer
 * has to notice that transaction on its own, which is what `parseAdvanced`
 * is for.
 */
export function parsedView<V extends EditorView>(view: V): V {
  if (covered(view.state)) {
    return view;
  }
  ensureSyntaxTree(view.state, view.state.doc.length, PARSE_DEADLINE_MS);
  view.dispatch({});
  return covered(view.state) ? view : refuse(view.state);
}
