/**
 * Where a dropped file or a moved picture lands: below the whole block under
 * the pointer, never inside it.
 *
 * A block is a top-level child of the document - a paragraph, a table, a
 * fence, a whole list or quote - so a picture never splits a paragraph whose
 * marks run over several lines, a table the grid draws, or a container its
 * unprefixed blank-line lead would end. The block then goes in the way
 * `insertBlock` puts one below a line, and `Landing.caret` is the caret that
 * makes today's upload flow do exactly that.
 *
 * Eager and small: no React, nothing of a lazy chunk. The tables the grid
 * draws past the parse (D58) are read through `drawnGridTables`, which lives
 * in the eager `grid/draws.ts` for this reason.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Line } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";

import { frontmatterHidden } from "../frontmatterFold";
import { frontmatterRegion } from "../frontmatterRegion";
import { drawnGridTables } from "../grid/draws";
import { closesFence, fenceMarker } from "../imageFormat";
import { isIdLineAt } from "../hiddenIdLines";

/** Where a block dropped at a position goes. */
export interface Landing {
  /** The 1-based line the block goes after. */
  line: number;
  /** That line's end: where the drop line is drawn and a moved picture is written. */
  pos: number;
  /**
   * Where the upload drop handler puts the caret before today's upload flow
   * runs, so `insertBlock` writes the file at `pos`.
   */
  caret: number;
}

function blank(line: Line): boolean {
  return line.text.trim() === "";
}

/** The last line of a table the grid draws over `line`, or null. */
function drawnTableEnd(state: EditorState, line: Line): Line | null {
  for (const read of state.facet(drawnGridTables)) {
    const table = read(state).find(
      (t) => t.from <= line.to && t.to >= line.from && t.to <= state.doc.length,
    );
    if (table) return state.doc.lineAt(table.to);
  }
  return null;
}

/**
 * The closing line of the fence `line` opens or sits in, read from the text
 * (the core's fence rule, as `eachLiveLine` applies it), or null when `line`
 * is outside every fence. An unclosed fence runs to the last line.
 */
function textFenceEnd(state: EditorState, line: Line): Line | null {
  const { doc } = state;
  let open: { char: string; count: number } | null = null;
  for (let number = 1; number <= doc.lines; number += 1) {
    const text = doc.line(number).text;
    if (open === null) {
      open = fenceMarker(text);
      if (open === null && number >= line.number) return null;
    } else if (closesFence(open, text)) {
      if (number >= line.number) return doc.line(number);
      open = null;
    }
  }
  return open === null ? null : doc.line(doc.lines);
}

/** A list item's first line: a bullet or a number, then a space or nothing. */
const LIST_ITEM = /^ {0,3}(?:[-+*]|\d{1,9}[.)])(?:[ \t]|$)/;

/** An indented line: below a list item, more of that item. */
const INDENTED = /^(?: {2}|\t)/;

/** The last line of the run of non-blank lines `line` is in, before a fence. */
function runEnd(state: EditorState, line: Line): Line {
  let last = line;
  while (last.number < state.doc.lines) {
    const next = state.doc.line(last.number + 1);
    if (blank(next) || fenceMarker(next.text) !== null) break;
    last = next;
  }
  return last;
}

/** The first line of the run of non-blank lines `line` is in, after a fence. */
function runStart(state: EditorState, line: Line): Line {
  let first = line;
  while (first.number > 1) {
    const above = state.doc.line(first.number - 1);
    if (blank(above) || fenceMarker(above.text) !== null) break;
    first = above;
  }
  return first;
}

/**
 * Whether the run of non-blank lines `line` is in belongs to a list: it
 * starts with a list item, or it is indented and the run above it (past the
 * blank lines between) belongs to one.
 */
function inTextList(state: EditorState, line: Line): boolean {
  for (let run = runStart(state, line); ;) {
    if (LIST_ITEM.test(run.text)) return true;
    if (!INDENTED.test(run.text) || fenceMarker(run.text) !== null) {
      return false;
    }
    let above = run.number - 1;
    while (above >= 1 && blank(state.doc.line(above))) above -= 1;
    if (above < 1 || above === run.number - 1) return false;
    run = runStart(state, state.doc.line(above));
  }
}

/**
 * The last line of the block holding `line`, read from the text alone where
 * the tree does not reach: a fence to its closing line (blank lines inside it
 * included), a list to its last item (the blank lines between the items of
 * a loose list included, P19), anything else to the line before the next
 * blank line or the next fence. Null for a blank line outside a fence.
 */
function textBlockEnd(state: EditorState, line: Line): Line | null {
  const fence = textFenceEnd(state, line);
  if (fence !== null) return fence;
  if (blank(line)) return null;
  let last = runEnd(state, line);
  if (!inTextList(state, line)) return last;
  // A loose list goes on past a blank line while the next text is another
  // item or more of the last one.
  const { doc } = state;
  for (;;) {
    let next = last.number + 1;
    if (next > doc.lines || !blank(doc.line(next))) return last;
    while (next <= doc.lines && blank(doc.line(next))) next += 1;
    if (next > doc.lines) return last;
    const text = doc.line(next).text;
    if (
      fenceMarker(text) !== null ||
      !(LIST_ITEM.test(text) || INDENTED.test(text))
    ) {
      return last;
    }
    last = runEnd(state, doc.line(next));
  }
}

/**
 * The last line of the top-level block holding `line`, or null when no
 * block holds it (a blank line between blocks). Where the tree stops short
 * of the block, its end is read from the text ({@link textBlockEnd}).
 */
function treeBlockEnd(state: EditorState, line: Line): Line | null {
  const tree = syntaxTree(state);
  if (tree.length < line.to) {
    return textBlockEnd(state, line);
  }
  let node = tree.resolveInner(line.from, 1);
  while (node.parent !== null && node.parent.parent !== null) {
    node = node.parent;
  }
  if (node.parent === null || node.to < line.from || node.from > line.to) {
    return null;
  }
  const last = state.doc.lineAt(node.to);
  // The last block a short tree holds may go on past it: a list the parse
  // stopped inside ends at its last parsed item, a little before the tree.
  return node.nextSibling === null && tree.length < state.doc.length
    ? (textBlockEnd(state, last) ?? last)
    : last;
}

/** The last line of the block `line` belongs to, never above line `floor`. */
function blockEnd(state: EditorState, line: Line, floor: number): Line {
  const drawn = drawnTableEnd(state, line);
  if (drawn !== null) return drawn;
  const end = treeBlockEnd(state, line);
  if (end !== null) return end;
  // A blank line right below a block: written into it, the picture would
  // join that block, so it goes below the block instead.
  if (blank(line) && line.number - 1 >= floor) {
    const above = state.doc.line(line.number - 1);
    if (!blank(above)) {
      return drawnTableEnd(state, above) ?? treeBlockEnd(state, above) ?? above;
    }
  }
  return line;
}

/**
 * Where a block dropped at `pos` lands, or null when there is no place for
 * it (a folded frontmatter block with no body line).
 */
export function dropLanding(state: EditorState, pos: number): Landing | null {
  const { doc } = state;
  let line = doc.lineAt(pos);
  let floor = 1;
  // The folded frontmatter is no place to write: the first body line is, as
  // `clearOfFoldedFrontmatter` answers a caret in it.
  if (frontmatterHidden(state)) {
    const region = frontmatterRegion(doc);
    if (region !== null) {
      const fence = doc.lineAt(region.to);
      floor = fence.number + 1;
      if (pos <= region.to) {
        if (fence.number >= doc.lines) return null;
        line = doc.line(fence.number + 1);
      }
    }
  }
  let last = blockEnd(state, line, floor);
  // A Reflection heading keeps its id line below it: the block goes below
  // the id, which a position on the hidden id line itself also means.
  if (isIdLineAt(state, last.number + 1)) {
    last = doc.line(last.number + 1);
  }
  if (last.number > 1 && isIdLineAt(state, last.number)) {
    // From the heading's end the input filters write below the id line;
    // from the id line's own end `insertBlock` would first step the caret to
    // the line below, one line too far.
    return {
      line: last.number,
      pos: last.to,
      caret: doc.line(last.number - 1).to,
    };
  }
  return { line: last.number, pos: last.to, caret: last.to };
}

/** Where a drag or drop event points: its client coordinates. */
interface Pointed {
  clientX: number;
  clientY: number;
}

/**
 * The document position a drag or drop event points at: the closest one, as
 * CodeMirror's own drop asks, so a drop below the last line still names it.
 * Null for an event with no coordinates (a synthetic one).
 */
export function dropPosition(view: EditorView, event: Pointed): number | null {
  if (!Number.isFinite(event.clientX) || !Number.isFinite(event.clientY)) {
    return null;
  }
  return view.posAtCoords({ x: event.clientX, y: event.clientY }, false);
}

/**
 * Where a block dropped at the event's point lands. The one reading of the
 * pointer the drop line, a file drop and a picture move all share, so the
 * line is drawn exactly where the block then goes.
 */
export function landingAt(view: EditorView, event: Pointed): Landing | null {
  const at = dropPosition(view, event);
  return at === null ? null : dropLanding(view.state, at);
}
