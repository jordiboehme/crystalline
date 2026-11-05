/**
 * One gesture, one undo step: a picture's size, placement, alt text, removal
 * or move is written as a step of its own, solo and in a room, even when it
 * comes right after typing.
 *
 * Solo, `isolateHistory` keeps CodeMirror's history from joining it to the
 * events around it. In a room the `Y.UndoManager` joins whatever lands in
 * its capture window, so its window is closed on both sides of the dispatch
 * (`roomUndo` hands the screen's manager in).
 *
 * A composition still waiting to be judged is judged first (D60, P13): a
 * gesture comes from outside the editor's DOM, so the capture listeners
 * that judge one before a press never saw it.
 */

import { isolateHistory } from "@codemirror/commands";
import type { TransactionSpec } from "@codemirror/state";
import { Annotation } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

import { judgeBeforeCommand } from "./marks/commandJudge";
import { roomUndo } from "./roomUndo";

/**
 * Dispatches `spec` as one undo step of its own, and says whether it did.
 * A caller reads the text after its own `judgeBeforeCommand` call; if the
 * judge here still changed the text, the spec's positions are stale and
 * nothing is dispatched (false).
 */
export function dispatchGesture(
  view: EditorView,
  spec: TransactionSpec & { userEvent: string },
): boolean {
  const before = view.state.doc;
  judgeBeforeCommand(view);
  if (view.state.doc !== before) {
    return false;
  }
  const given = spec.annotations;
  const annotations: readonly Annotation<unknown>[] =
    given === undefined ? [] : given instanceof Annotation ? [given] : given;
  const room = view.state.facet(roomUndo);
  room?.stopCapturing();
  view.dispatch({
    ...spec,
    annotations: [...annotations, isolateHistory.of("full")],
  });
  room?.stopCapturing();
  return true;
}
