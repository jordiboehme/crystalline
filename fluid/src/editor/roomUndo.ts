/**
 * The room's `Y.UndoManager`, for code that has to close its capture window:
 * a grid gesture that adds a row calls `stopCapturing()` on both sides, so
 * the row is its own undo step and not joined to the typing around it.
 * y-codemirror keeps its own facet for the manager private, so each screen
 * provides it here too, next to `yCollab`, with the same instance. Null
 * outside a room.
 */

import { Facet } from "@codemirror/state";
import type * as Y from "yjs";

export const roomUndo = Facet.define<
  Y.UndoManager | null,
  Y.UndoManager | null
>({
  combine: (values) => values[0] ?? null,
});
