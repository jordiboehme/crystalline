/**
 * The text edits every diagram op compiles to, and the two ways they leave
 * the compiler: applied to the fence body as a string (`applyEdits`, which
 * the verify step and the tests use) or turned into document changes
 * (`toChanges`, which the dispatch uses). Both read the edits in the same
 * order, so what is verified is what is written.
 *
 * Positions are body-relative and the body is an LF string, as
 * `state.doc.sliceString` reads it. A CodeMirror position counts a line break
 * as one, whatever the document's separator, so a body offset plus the
 * body's start is a document position in an LF and a CRLF document alike;
 * only the inserted text needs the document's line break.
 */

import type { ChangeSpec } from "@codemirror/state";

/** A range of the fence body. */
export interface Span {
  from: number;
  to: number;
}

/** One replacement in the fence body; `"\n"` in `insert` is a line break. */
export interface TextEdit {
  from: number;
  to: number;
  insert: string;
}

/** Why an op made no change. */
export type RefusalCode =
  | "stale"
  | "gone"
  | "invalid"
  | "group"
  | "chain"
  | "unsupported"
  | "unchecked"
  | "breaksFence"
  | "needsLineBelow"
  | "arrowChanged"
  | "emptiesBlock"
  | "leavesActive";

/**
 * What an op compiler returns: the edits (and what they create, of a type each
 * kind of diagram names) or a refusal.
 */
export type OpResult<Created = unknown> =
  { edits: TextEdit[]; created?: Created } | { refused: RefusalCode };

/** The notice shown for each refusal. None mentions Raw or text. */
export const NOTICES: Record<RefusalCode, string> = {
  stale: "The diagram just changed. Try again.",
  gone: "That part of the diagram is no longer there.",
  invalid: "That change would break the diagram, so nothing was changed.",
  group:
    "This arrow is drawn together with others, so it cannot be changed on its own.",
  chain: "This arrow is part of a chain, so it cannot be reversed on its own.",
  unsupported:
    "This diagram uses something the editor does not know yet, so nothing was changed.",
  unchecked:
    "The diagram could not be checked, so nothing was changed. Try again.",
  breaksFence: "That would break a diagram here, so nothing was changed.",
  needsLineBelow:
    "A diagram at the end of a page needs a line below it, so nothing was changed.",
  arrowChanged:
    "This arrow changed, so the label was not written. Press Escape and open it again.",
  emptiesBlock:
    "That would leave a block with nothing in it, so nothing was changed.",
  leavesActive: "That would leave an activation open, so nothing was changed.",
};

/**
 * `edits` in source order, checked: each range lies in `0..length` and runs
 * forward, and no two overlap (touching is fine). The order is by `from`,
 * then `to`, so an insert at `p` goes before a range that starts at `p`;
 * inserts at one point keep the order they were given in.
 */
export function sortedEdits(
  edits: readonly TextEdit[],
  length: number,
): TextEdit[] {
  const sorted = edits
    .map((edit, i) => ({ edit, i }))
    .sort(
      (a, b) => a.edit.from - b.edit.from || a.edit.to - b.edit.to || a.i - b.i,
    )
    .map(({ edit }) => edit);
  let end = 0;
  for (const edit of sorted) {
    if (
      !Number.isInteger(edit.from) ||
      !Number.isInteger(edit.to) ||
      edit.from < 0 ||
      edit.to < edit.from ||
      edit.to > length
    ) {
      throw new RangeError(
        `edit ${edit.from}..${edit.to} is outside the body (0..${length})`,
      );
    }
    if (edit.from < end) {
      throw new RangeError(`edit ${edit.from}..${edit.to} overlaps another`);
    }
    end = edit.to;
  }
  return sorted;
}

/** `body` with `edits` applied; throws on an overlap or a bad range. */
export function applyEdits(body: string, edits: readonly TextEdit[]): string {
  let out = "";
  let at = 0;
  for (const edit of sortedEdits(edits, body.length)) {
    out += body.slice(at, edit.from) + edit.insert;
    at = edit.to;
  }
  return out + body.slice(at);
}

/**
 * `edits` as changes to a document whose body starts at `bodyFrom` and whose
 * line break is `lineBreak` (`state.lineBreak`): each `"\n"` of an insert
 * becomes that break, the CRLF rule `mermaidStarters.ts` follows. The ranges
 * are checked as `applyEdits` checks them, except against the body's end,
 * which a caller holding only `bodyFrom` does not know.
 *
 * `body`, for a fence, whose body always has a line break after it: an edit can
 * only name the body, so deleting the last line takes the break before it, and
 * the line before would end in the last line's break, which in a mixed document
 * is another one (R68). Such a deletion moves on by one position, to take the
 * last line's own break. The LF text is the same, so what was verified is what
 * is written.
 */
export function toChanges(
  bodyFrom: number,
  edits: readonly TextEdit[],
  lineBreak: string,
  body?: string,
): ChangeSpec[] {
  return sortedEdits(edits, Number.MAX_SAFE_INTEGER).map((edit) => {
    const shift =
      body !== undefined &&
      edit.insert === "" &&
      edit.to === body.length &&
      edit.from < edit.to &&
      body[edit.from] === "\n"
        ? 1
        : 0;
    return {
      from: bodyFrom + edit.from + shift,
      to: bodyFrom + edit.to + shift,
      insert:
        lineBreak === "\n"
          ? edit.insert
          : edit.insert.replaceAll("\n", lineBreak),
    };
  });
}
