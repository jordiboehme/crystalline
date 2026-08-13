/**
 * "The parse moved on", as a question a decoration layer can ask.
 *
 * Every decoration in this directory is derived from the syntax tree, and the
 * tree is not finished when a buffer is mounted. `@codemirror/language` parses
 * the first 3000 characters of a new state within a 20 millisecond wall-clock
 * budget and keeps whatever was ready when either limit was reached; the rest
 * is parsed afterwards by the `parseWorker` plugin in a `requestIdleCallback`
 * pseudo-thread, which publishes each advance as a transaction carrying no
 * document change, no selection change and no viewport change.
 *
 * A layer that rebuilds only on those three therefore keeps decorations it
 * computed from a tree that has since grown: on a long engram, or on a slow or
 * busy machine, code blocks past the cut-off read in the prose font, syntax
 * marks stay unfolded and wikilinks stay as brackets until the reader happens
 * to type or scroll. Asking this question as well is what closes that window,
 * and it costs one identity comparison per transaction - the tree object is
 * replaced wholesale when it changes, never mutated.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";

/**
 * Both shapes a decoration layer is updated with - a `ViewUpdate` and a
 * `Transaction` - carry the state on each side under these names.
 */
interface Passage {
  readonly startState: EditorState;
  readonly state: EditorState;
}

export function parseAdvanced(passage: Passage): boolean {
  return syntaxTree(passage.startState) !== syntaxTree(passage.state);
}
