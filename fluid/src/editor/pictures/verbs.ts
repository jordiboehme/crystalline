/**
 * The picture verbs: placement, size, width, alt text, removal and a move,
 * each one gesture (`dispatchGesture`), so one undo takes exactly it.
 *
 * Every verb re-reads the picture at dispatch (`pictureAt` at `at`, the
 * caret when omitted) rather than trusting what a control last drew, and
 * refuses (false, nothing dispatched) where it finds none. A verb answers
 * true only when the text changed: a refusal, a no-op, a removal a check
 * refused and a gesture `dispatchGesture` dropped all answer false. A picture inside
 * a link is the link's (a press opens the link), and one in a table cell is
 * the grid's to draw and edit: both refuse every verb. The format verbs act
 * only on this page's own attachments; alt, removal and a move work for
 * every kind. Afterwards the picture is selected where it now stands,
 * except after a removal.
 *
 * What a verb writes is the target token or the alt middle and nothing else:
 * the path as written (`./`, a title, parentheses, escapes) is the author's.
 * A removal and a move go through the hidden-mark checks (P7, P8), so a
 * picture between two bold words leaves both bold.
 */

import type { ChangeSet, EditorState, Text } from "@codemirror/state";
import { EditorSelection } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

import { dispatchGesture } from "../gesture";
import type { ImageAlign, ImageFormat } from "../imageFormat";
import { buildImageTarget } from "../imageFormat";
import { judgeBeforeCommand } from "../marks/commandJudge";
import { deletionOutcome, judgeDeletion } from "../marks/deleteCheck";
import { judgedElsewhere } from "../marks/deleting";
import { judgeInsert } from "../marks/insertCheck";
import { PARSE_BUDGET, deadlineIn } from "../marks/parse";
import { blockInsertion } from "../toolbar";
import { altEdit } from "./altText";
import type { Landing } from "./landing";
import type { PictureRef } from "./refs";
import { pictureAt } from "./refs";
import { pictureRemoval } from "./removal";

/** A placement the bar offers; `full` is a size. */
export type PictureAlign = "left" | "center" | "right";

/** A size the bar offers. */
export type PictureSize = "small" | "medium" | "full" | "original";

/** The narrowest and widest share a width may name, in percent. */
const MIN_WIDTH = 10;
const MAX_WIDTH = 100;

/**
 * The picture a verb acts on, read from the text as it is now, or null when
 * there is none or it is not this page's to change.
 */
function target(view: EditorView, at: number | undefined): PictureRef | null {
  const { state } = view;
  if (!state.facet(EditorView.editable) || state.readOnly) {
    return null;
  }
  // A composition still waiting is judged before the text is read (P13).
  judgeBeforeCommand(view);
  const ref = pictureAt(view.state, at ?? view.state.selection.main.head);
  if (ref === null || ref.inLink || ref.inCell) {
    return null;
  }
  return ref;
}

/**
 * Dispatches `spec` as a gesture and answers whether the text changed: false
 * when `dispatchGesture` dropped it or a filter refused it.
 */
function changed(
  view: EditorView,
  spec: Parameters<typeof dispatchGesture>[1],
): boolean {
  const before = view.state.doc;
  return dispatchGesture(view, spec) && view.state.doc !== before;
}

/** The picture as a selection, `delta` longer than it was. */
function selected(ref: PictureRef, delta: number): EditorSelection {
  return EditorSelection.single(ref.from, ref.to + delta);
}

/**
 * Rewrites the target of the attachment at `at` to the format `next` gives,
 * or refuses. A target that already reads so dispatches nothing (false).
 */
function reformat(
  view: EditorView,
  at: number | undefined,
  next: (format: ImageFormat) => ImageFormat,
): boolean {
  const ref = target(view, at);
  if (ref === null || ref.kind !== "attachment" || ref.format === null) {
    return false;
  }
  const token = buildImageTarget(ref.written, next(ref.format));
  const old = view.state.sliceDoc(ref.targetFrom, ref.targetTo);
  if (token === old) {
    return false;
  }
  return changed(view, {
    changes: { from: ref.targetFrom, to: ref.targetTo, insert: token },
    selection: selected(ref, token.length - old.length),
    userEvent: "input.format",
    scrollIntoView: true,
  });
}

/**
 * Places the picture and keeps its width; a full picture takes the
 * placement with no width.
 */
export function setPictureAlign(
  view: EditorView,
  align: PictureAlign,
  at?: number,
): boolean {
  return reformat(view, at, (format) =>
    format.align === "full" || format.width === undefined
      ? { align }
      : { align, width: format.width },
  );
}

/**
 * Sizes the picture: `small` and `medium` as a quarter and a half of the
 * column, keeping a float and turning `full` into centered; `full` the whole
 * column with no width; `original` its own size, `full` turned into
 * centered.
 */
export function setPictureSize(
  view: EditorView,
  size: PictureSize,
  at?: number,
): boolean {
  return reformat(view, at, (format) => {
    const align: ImageAlign = format.align === "full" ? "center" : format.align;
    switch (size) {
      case "small":
        return { align, width: "25%" };
      case "medium":
        return { align, width: "50%" };
      case "full":
        return { align: "full" };
      case "original":
        return { align };
    }
  });
}

/** Sizes the picture to `percent` of the column, a whole number from 10 to 100. */
export function setPictureWidth(
  view: EditorView,
  percent: number,
  at?: number,
): boolean {
  const share = Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, Math.round(percent)));
  return reformat(view, at, (format) => ({
    align: format.align === "full" ? "center" : format.align,
    width: `${share}%`,
  }));
}

/**
 * Changes the alt text to read `next`: one change to the part that differs,
 * escaped where it has to be (`altEdit`). The same words dispatch nothing
 * (false). With `select` false the selection is left where it is (mapped
 * through the change): the bar writes a typed alt text when it closes,
 * after the selection may have moved on.
 */
export function setPictureAlt(
  view: EditorView,
  next: string,
  at?: number,
  select = true,
): boolean {
  const ref = target(view, at);
  if (ref === null) {
    return false;
  }
  const edit = altEdit(view.state.sliceDoc(ref.altFrom, ref.altTo), next);
  if (edit === null) {
    return false;
  }
  return changed(view, {
    changes: {
      from: ref.altFrom + edit.from,
      to: ref.altFrom + edit.to,
      insert: edit.insert,
    },
    ...(select
      ? {
          selection: selected(ref, edit.insert.length - (edit.to - edit.from)),
        }
      : {}),
    userEvent: "input.alt",
    scrollIntoView: true,
  });
}

/**
 * Removes the picture, with its line when it stands alone on one
 * (`pictureRemoval`). Tagged `delete.picture`, which the hidden-mark checks
 * judge like any deletion (P7): a pair the removal would break is repaired
 * or the removal refused (false, with the check's own announcement), as a
 * selection deleted there would be.
 */
export function removePicture(view: EditorView, at?: number): boolean {
  const ref = target(view, at);
  if (ref === null) {
    return false;
  }
  const range = pictureRemoval(view.state, ref);
  return changed(view, {
    changes: range,
    selection: EditorSelection.cursor(range.from),
    userEvent: "delete.picture",
    scrollIntoView: true,
  });
}

/**
 * Whether two texts hold the same blocks in the same order: equal once each
 * run of blank lines reads as one. A move that changes only how many blank
 * lines stand between two blocks puts the picture where it already was; the
 * extra blank line `insertBlock` leaves below a block (P17) is such a change.
 */
function sameBlocks(a: Text, b: Text): boolean {
  if (a.eq(b)) {
    return true;
  }
  const read = (text: Text): string[] => {
    const out: string[] = [];
    for (const line of text.iterLines()) {
      const blank = line.trim() === "";
      if (blank && out.at(-1) === "") continue;
      out.push(blank ? "" : line);
    }
    return out;
  };
  const left = read(a);
  const right = read(b);
  return left.length === right.length && left.every((l, i) => l === right[i]);
}

/**
 * The removal of `ref` and its insertion below `landing`'s line, as one
 * change of `state`, with where the picture then begins; `said` when the
 * removal's check refuses it (P21), with the sentence a refused delete
 * gives; null when the move would leave every block where it is or lands
 * inside what it takes away.
 */
function moveChanges(
  state: EditorState,
  ref: PictureRef,
  landing: Landing,
): { changes: ChangeSet; start: number } | { said: string } | null {
  const { doc } = state;
  if (landing.line < 1 || landing.line > doc.lines) {
    return null;
  }
  const line = doc.line(landing.line);
  const range = pictureRemoval(state, ref);
  // The landing's end inside the range, or its whole line taken with it.
  if (
    (range.from < line.to && line.to < range.to) ||
    (range.from <= line.from && line.to <= range.to)
  ) {
    return null;
  }
  const deadline = deadlineIn(PARSE_BUDGET.gesture);
  // The removal first, judged as a deletion is: the pairs around the
  // picture keep reading as they did (P7).
  const plain = state.changes(range);
  const judged = judgeDeletion(state, plain, deadline);
  let removal: ChangeSet;
  switch (judged.outcome) {
    case "refused":
      return { said: judged.said };
    case "unchecked":
      removal = plain;
      break;
    case "exact":
      removal = judged.changes ?? plain;
      break;
    default:
      removal = judged.changes;
  }
  const mid = state.update({ changes: removal, filter: false }).state;
  const picture = state.sliceDoc(ref.from, ref.to);
  const after = mid.doc.lineAt(removal.mapPos(line.to, -1));
  const { change, start } = blockInsertion(mid, after, [picture]);
  let insert = mid.changes(change);
  let begins = start;
  // Then the insert at the landing, judged by Part 2's insert judge (P8).
  // The text is read back from the change so a CRLF break counts as the one
  // position it is in the document.
  let text = "";
  insert.iterChanges((_fromA, _toA, _fromB, _toB, inserted) => {
    text = inserted.toString();
  });
  const moved = judgeInsert(
    mid,
    [],
    { from: change.from, to: change.from, text },
    deadline,
  );
  if (moved !== null) {
    insert = moved.changes;
    // The block ends in one line break after the picture.
    begins = moved.end - 1 - (ref.to - ref.from);
  }
  const changes = removal.compose(insert);
  if (sameBlocks(doc, changes.apply(doc))) {
    return null;
  }
  return { changes, start: begins };
}

/**
 * Moves the picture at `from` below the block `landing` names
 * (`dropLanding`), in one gesture tagged `move.drop`, and selects it where
 * it lands. Refused when it would land inside what it takes away or would
 * leave the picture where it stands, and when the check refuses the removal,
 * which then says why (P21).
 *
 * The move judges its own removal and insert with the hidden-mark judges
 * and is marked `judgedElsewhere`, so the filters leave it where the drop
 * line said (P8).
 */
export function movePicture(
  view: EditorView,
  from: number,
  landing: Landing,
): boolean {
  // Looked up one in from its start, as `selectedPicture` does: at a
  // junction of two pictures `from` is also the end of the left one.
  const ref = target(view, from + 1);
  if (ref === null || ref.from !== from) {
    return false;
  }
  const move = moveChanges(view.state, ref, landing);
  if (move === null) {
    return false;
  }
  if ("said" in move) {
    view.dispatch({
      effects: [
        EditorView.announce.of(move.said),
        deletionOutcome.of("refused"),
      ],
    });
    return false;
  }
  return changed(view, {
    changes: move.changes,
    selection: EditorSelection.single(
      move.start,
      move.start + (ref.to - ref.from),
    ),
    userEvent: "move.drop",
    annotations: judgedElsewhere.of(true),
    scrollIntoView: true,
  });
}
