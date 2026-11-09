/**
 * Fidelity checks for diagram ops: an op may change the bytes it names and
 * nothing else. The property suites run every op through these, so each has
 * a failing case in `editor/diagram/edits.test.ts`.
 */

import {
  type TextEdit,
  applyEdits,
  sortedEdits,
} from "../editor/diagram/edits";

/**
 * Whether `after` is `applyEdits(before, edits)` and every byte of `before`
 * outside the edited ranges sits unchanged in `after`, in its place. A bad
 * or overlapping edit list is a failure, not a throw.
 */
export function onlyEditedBytes(
  before: string,
  after: string,
  edits: readonly TextEdit[],
): boolean {
  let sorted: TextEdit[];
  try {
    sorted = sortedEdits(edits, before.length);
  } catch {
    return false;
  }
  if (after !== applyEdits(before, sorted)) {
    return false;
  }
  let at = 0;
  let shift = 0;
  for (const edit of [
    ...sorted,
    { from: before.length, to: before.length, insert: "" },
  ]) {
    const kept = before.slice(at, edit.from);
    if (after.slice(at + shift, at + shift + kept.length) !== kept) {
      return false;
    }
    shift += edit.insert.length - (edit.to - edit.from);
    at = edit.to;
  }
  return true;
}

/**
 * The lines of `before` that no edit touches but that `after` does not hold
 * whole at their shifted place (empty means fine). An edit touches a line
 * when it starts at or before the line's end and ends at or after its start,
 * so an insert at either edge of a line counts as touching it.
 */
export function untouchedLinesEqual(
  before: string,
  after: string,
  edits: readonly TextEdit[],
): string[] {
  const sorted = sortedEdits(edits, before.length);
  const differ: string[] = [];
  let start = 0;
  for (const line of before.split("\n")) {
    const end = start + line.length;
    const touched = sorted.some((e) => e.from <= end && e.to >= start);
    if (!touched) {
      let shift = 0;
      for (const e of sorted) {
        if (e.to < start) {
          shift += e.insert.length - (e.to - e.from);
        }
      }
      const at = start + shift;
      const whole =
        after.slice(at, at + line.length) === line &&
        (at === 0 || after[at - 1] === "\n") &&
        (at + line.length === after.length || after[at + line.length] === "\n");
      if (!whole) {
        differ.push(line);
      }
    }
    start = end + 1;
  }
  return differ;
}
