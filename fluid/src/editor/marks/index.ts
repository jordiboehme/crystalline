/**
 * The hidden-mark layer: the marks the model finds (`hiddenMarks.ts`) drawn
 * away, as atomic ranges a caret steps over, with the caret and delete rules
 * that go with them (`caret.ts`, `deleting.ts`). Installing it is also what
 * separates the preview from Raw for `marksHiddenOnLine`.
 *
 * It sits in the preview compartment after the prose layers, not inside
 * `livePreview()`; the table cell editor gets it after its prose layers
 * through `cellPreviewLayers`, with the cell's content as its segment.
 */

import type { EditorState, Extension } from "@codemirror/state";

import { elementMenuKey } from "../elementEdit";
import { hiddenMarkDrawing, linkClicks } from "../preview";
import { blockDrawing, listEnter, taskKeymap } from "./blocks";
import { caretKeymap, caretNormalizer } from "./caret";
import { compositionCheck } from "./composition";
import { checkedDeletion } from "./deleteCheck";
import { deletionKeymap, headingQuestion, keepPartnerMarks } from "./deleting";
import { formatting } from "./formatting";
import { checkedInsert } from "./insertCheck";
import { hiddenMarkLayerOn } from "./mode";

export type { TaskState } from "./blocks";
export { listEnter, taskChanges } from "./blocks";
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
export {
  deleteVisible,
  deletionKeymap,
  judgedElsewhere,
  keepPartnerMarks,
} from "./deleting";
export type { DeleteOutcome } from "./deleteCheck";
export { CANNOT_JOIN, checkedDeletion, deletionOutcome } from "./deleteCheck";
export type { Format, Pending } from "./formatting";
export {
  FORMAT_MARKER,
  formatCell,
  formatInput,
  formatKeymap,
  formatStateListener,
  formatsAt,
  pendingFormat,
  toggleFormat,
} from "./formatting";

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
    // A press on a link's words, and Shift+F10 on the element at the caret,
    // open its popover.
    linkClicks,
    elementMenuKey,
    // Bullets, numbers, task boxes, quote bars and rules in place of their marks.
    blockDrawing,
    listEnter,
    taskKeymap,
    caretKeymap,
    deletionKeymap,
    // The format waiting for typing and the typing rules around formats.
    formatting,
    // An IME composition, judged with the same rules when it ends.
    compositionCheck,
    // Filters run last-listed first: a range deletion is rewritten to keep
    // its partner marks, then checked against the text it leaves (repaired
    // or refused), and an insert against the pairs around it (I2), before
    // the caret it leaves is normalized.
    caretNormalizer,
    checkedDeletion,
    checkedInsert,
    keepPartnerMarks,
    ...(options.confirmHeadingChange
      ? [headingQuestion.of(options.confirmHeadingChange)]
      : []),
  ];
}
