/**
 * The room's `Y.UndoManager`, for code that has to close its capture window:
 * a grid gesture that adds a row calls `stopCapturing()` on both sides, so
 * the row is its own undo step and not joined to the typing around it.
 * y-codemirror keeps its own facet for the manager private, so each screen
 * provides it here too, next to `yCollab`, with the same instance. Null
 * outside a room.
 */

import { isolateHistory } from "@codemirror/commands";
import type { TransactionSpec } from "@codemirror/state";
import { Annotation, Facet } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type * as Y from "yjs";

export const roomUndo = Facet.define<
  Y.UndoManager | null,
  Y.UndoManager | null
>({
  combine: (values) => values[0] ?? null,
});

/**
 * Dispatches `spec` as one undo step of its own: `isolateHistory` keeps
 * CodeMirror's history from joining it to the events around it, and in a room
 * the manager's capture window closes on both sides. Inside a room's stack the
 * annotation does nothing, and the screens' room buffers have no history for
 * it to act on.
 */
export function dispatchIsolated(
  view: EditorView,
  spec: TransactionSpec,
): void {
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
}
