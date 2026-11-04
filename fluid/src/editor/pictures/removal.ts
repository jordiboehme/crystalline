/**
 * What a picture's delete or move takes away.
 *
 * A picture alone on its line takes the line with it, so no empty line is
 * left where it stood: the line and the break after it (the break before it
 * on the last line), and one of two blank lines that would otherwise meet.
 * A picture inside prose takes its span and nothing else. A Reflection's id
 * line, and the breaks on either side of it, are never part of the range.
 */

import type { EditorState, Line } from "@codemirror/state";

import { isIdLineAt } from "../hiddenIdLines";
import type { PictureRef } from "./refs";

/** A document range, in positions. */
interface Range {
  from: number;
  to: number;
}

/** Whether `range` takes a character of an id line or one of its breaks. */
function reachesIdLine(state: EditorState, range: Range): boolean {
  const { doc } = state;
  const first = doc.lineAt(Math.max(0, range.from - 1)).number;
  const last = doc.lineAt(Math.min(doc.length, range.to + 1)).number;
  for (let number = first; number <= last; number += 1) {
    if (!isIdLineAt(state, number)) continue;
    const line = doc.line(number);
    // The break before the line is the character at `from - 1`, the one
    // after it the character at `to`.
    if (range.from < line.to + 1 && range.to > line.from - 1) return true;
  }
  return false;
}

function empty(line: Line | null): boolean {
  return line !== null && line.text.trim() === "";
}

/** The range a delete or move of `ref` removes. */
export function pictureRemoval(
  state: EditorState,
  ref: PictureRef,
): { from: number; to: number } {
  const { doc } = state;
  const bare = { from: ref.from, to: ref.to };
  const line = doc.lineAt(ref.from);
  if (line.text.trim() !== doc.sliceString(ref.from, ref.to)) {
    return bare;
  }
  const above = line.number > 1 ? doc.line(line.number - 1) : null;
  const below = line.number < doc.lines ? doc.line(line.number + 1) : null;
  const candidates: Range[] = [];
  if (below !== null) {
    if (empty(above) && empty(below) && above !== null) {
      // "para, blank, picture, blank, next" keeps one blank line.
      candidates.push({ from: above.from, to: line.to + 1 });
    }
    candidates.push({ from: line.from, to: line.to + 1 });
  } else if (above !== null) {
    candidates.push({ from: line.from - 1, to: line.to });
  }
  candidates.push({ from: line.from, to: line.to });
  for (const range of candidates) {
    if (!reachesIdLine(state, range)) return range;
  }
  return bare;
}
