/**
 * The hidden-mark layer: the marks the model finds (`hiddenMarks.ts`) drawn
 * away, as atomic ranges a caret steps over, with the caret and delete rules
 * that go with them (`caret.ts`, `deleting.ts`). Installing it is also what
 * separates the preview from Raw for `marksHiddenOnLine`.
 *
 * It sits in the preview compartment after the prose layers, not inside
 * `livePreview()`, so the table cell editor, which reads the prose layers,
 * gets it only once it is given it on purpose.
 */

import type { EditorState, Extension } from "@codemirror/state";

import { hiddenMarkDrawing } from "../preview";
import { caretKeymap, caretNormalizer } from "./caret";
import { deletionKeymap, headingQuestion, keepPartnerMarks } from "./deleting";
import { hiddenMarkLayerOn } from "./mode";

export type { Mark, MarkKind, MarkRun } from "./hiddenMarks";
export { marksOnLine, runAt, runsOnLine } from "./hiddenMarks";
export type { Segment } from "./caret";
export {
  canonical,
  caretKeymap,
  caretNormalizer,
  crossEdge,
  markSegment,
  snapRange,
  stepVisible,
} from "./caret";
export { deleteVisible, deletionKeymap, keepPartnerMarks } from "./deleting";

export interface HiddenMarkOptions {
  /**
   * The adapter's question before a heading change, asked when Backspace
   * would take a heading's prefix away (a Reflection's section headings).
   */
  confirmHeadingChange?: (
    state: EditorState,
    line: number,
    nextLevel: number,
  ) => string | null;
}

export function hiddenMarkLayer(options: HiddenMarkOptions = {}): Extension {
  return [
    hiddenMarkLayerOn.of(true),
    hiddenMarkDrawing,
    caretKeymap,
    deletionKeymap,
    // Filters run last-listed first: a range deletion is rewritten to keep
    // its partner marks before the caret it leaves is normalized.
    caretNormalizer,
    keepPartnerMarks,
    ...(options.confirmHeadingChange
      ? [headingQuestion.of(options.confirmHeadingChange)]
      : []),
  ];
}
