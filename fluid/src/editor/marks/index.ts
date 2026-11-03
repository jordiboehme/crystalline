/**
 * The hidden-mark layer: the marks the model finds (`hiddenMarks.ts`) drawn
 * away, as atomic ranges a caret steps over. Installing it is also what
 * separates the preview from Raw for `marksHiddenOnLine`.
 *
 * It sits in the preview compartment after the prose layers, not inside
 * `livePreview()`, so the table cell editor, which reads the prose layers,
 * gets it only once it is given it on purpose.
 */

import type { Extension } from "@codemirror/state";

import { hiddenMarkDrawing } from "../preview";
import { hiddenMarkLayerOn } from "./mode";

export type { Mark, MarkKind, MarkRun } from "./hiddenMarks";
export { marksOnLine, runAt, runsOnLine } from "./hiddenMarks";

export function hiddenMarkLayer(): Extension {
  return [hiddenMarkLayerOn.of(true), hiddenMarkDrawing];
}
