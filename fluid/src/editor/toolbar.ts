/**
 * The formatting commands the floating toolbar and its shortcuts run. Every
 * command is an ordinary transaction on the live view - never setState, never
 * a buffer swap - so the same code is collab-safe by construction: in a room
 * the yCollab binding sees a local edit and writes the shared text once.
 *
 * Multi-line insertions join with `state.lineBreak`: the change-set splits
 * inserted text by the state's own separator facet, so a literal "\n" inside
 * an insertion into a CRLF solo document would land as line CONTENT rather
 * than as a break. The skeletons are therefore line arrays, joined at the
 * moment of insertion with whatever separator the buffer actually uses.
 *
 * Every command dispatches exactly once and tags itself `userEvent: "input"`
 * rather than `"input.type"`: the history joins adjacent events only for
 * `input.type` and `delete`, so a plain `input` is one undo step per action
 * and never merges into the typing around it.
 */

import type { EditorState, Extension, Line } from "@codemirror/state";
import { EditorSelection, Prec } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { keymap } from "@codemirror/view";

import { frontmatterHidden } from "./frontmatterFold";
import { frontmatterRegion } from "./frontmatterRegion";

/**
 * Keep a command off a frontmatter block that is currently folded away.
 *
 * The fold is atomic, but atomicity constrains cursor MOTION only - it is read
 * by the movement commands and by pointer selection, never by a dispatched
 * change - so nothing stops a toolbar verb from writing into text nobody can
 * see. And the caret starts there: `CmEditor` sets no initial selection, so a
 * freshly opened engram has its caret at position 0, inside the block. A
 * heading run before the author has clicked anywhere would turn the opening
 * fence into `## ---`, and a table would land between the fence and the first
 * key where the chip keeps it invisible - a button that looks like it did
 * nothing while the file stops parsing.
 *
 * Two cases, deliberately answered differently:
 *
 * - A bare caret in the region is not a decision, it is where the buffer put
 *   it. The whole selection is REPLACED by one cursor on the first line after
 *   the block, because clicking a format button means "put this in my
 *   document" and the document starts there.
 * - A selection that spans the block was made on purpose - select-all then
 *   bold is the ordinary way in - so the command REFUSES and returns false
 *   rather than quietly formatting something else. Returning false leaves the
 *   key unhandled, which is the honest answer for a command that declined.
 *
 * The question asked is the fold's own live state, never the mode: in Raw mode
 * the field is not installed, after the chip is clicked its set is empty, and
 * the MANIFEST surface never carries it. In all three the frontmatter is text
 * on screen like any other and formatting it is legitimate.
 */
function clearOfFoldedFrontmatter(view: EditorView): boolean {
  const { state } = view;
  if (!frontmatterHidden(state)) {
    return true;
  }
  const region = frontmatterRegion(state.doc);
  if (region === null) {
    return true;
  }
  const touching = state.selection.ranges.filter(
    (range) => range.from <= region.to && range.to >= region.from,
  );
  if (touching.length === 0) {
    return true;
  }
  if (touching.some((range) => !range.empty)) {
    return false;
  }
  const fence = state.doc.lineAt(region.to);
  if (fence.number >= state.doc.lines) {
    // Frontmatter and nothing else: there is no body line to move to, so
    // there is nowhere honest to put the edit.
    return false;
  }
  view.dispatch({
    selection: EditorSelection.cursor(state.doc.line(fence.number + 1).from),
  });
  return true;
}

/**
 * Whether the text on both sides of `from`..`to` is this command's own pair.
 *
 * The run of marker characters has to be EXACTLY as long as the marker. A
 * neighbour that is part of a longer run belongs to a stronger emphasis:
 * double-clicking the word in `**hello**` and pressing italic sees a `*` on
 * each side, and a sniff that stopped there would strip the bold off a person
 * who asked to add italic. Longer runs are therefore left alone and the
 * command nests instead - `***hello***` - which is what was asked for.
 *
 * Every marker this is used with is one character repeated, which is what
 * makes "one longer" spellable as the marker plus that character.
 *
 * The price of that rule, accepted rather than overlooked: inside `***hello***`
 * neither italic nor bold unwraps. The run on each side is three characters
 * long, no exact-length match is found, and the command nests instead, so the
 * text becomes `****hello****` - additive junk markup rather than the removal
 * that was asked for. Attributing that third character to one of the two
 * emphases is a guess this refuses to make, because guessing wrong strips the
 * emphasis a writer did not touch, and the cost of refusing is one undo.
 */
function wrappedIn(
  state: EditorState,
  from: number,
  to: number,
  marker: string,
): boolean {
  const char = marker.slice(0, 1);
  const longer = char + marker;
  const before = state.sliceDoc(Math.max(0, from - longer.length), from);
  const after = state.sliceDoc(
    to,
    Math.min(state.doc.length, to + longer.length),
  );
  return (
    before.endsWith(marker) &&
    !before.endsWith(longer) &&
    after.startsWith(marker) &&
    !after.startsWith(longer)
  );
}

/**
 * Wrap or unwrap every selection range in `marker` - bold, italic and inline
 * code are the same operation with a different string.
 *
 * An empty range is the useful cursor case rather than a no-op: it leaves the
 * cursor between a fresh pair of markers, ready to type into, and toggling
 * again from that same spot removes the pair it just made.
 */
export function toggleInline(view: EditorView, marker: string): boolean {
  if (!clearOfFoldedFrontmatter(view)) {
    return false;
  }
  const changes = view.state.changeByRange((range) => {
    const { from, to } = range;
    if (wrappedIn(view.state, from, to, marker)) {
      return {
        changes: [
          { from: from - marker.length, to: from, insert: "" },
          { from: to, to: to + marker.length, insert: "" },
        ],
        range: EditorSelection.range(from - marker.length, to - marker.length),
      };
    }
    return {
      changes: [
        { from, insert: marker },
        { from: to, insert: marker },
      ],
      range: EditorSelection.range(from + marker.length, to + marker.length),
    };
  });
  view.dispatch(changes, { userEvent: "input" });
  view.focus();
  return true;
}

const HEADING = /^(#{1,6})[ \t]+/;

/**
 * Heading level on the MAIN selection's line only - deliberate scope, not an
 * oversight: a heading is a line property, and cycling it under every cursor
 * of a multi-cursor selection is more surprise than help. The inline
 * wrappers and the wiki link are the multi-range commands.
 */
export function cycleHeading(view: EditorView, level: number): boolean {
  if (!clearOfFoldedFrontmatter(view)) {
    return false;
  }
  // Read AFTER the guard: it may have moved the caret out of the block.
  const { state } = view;
  const line = state.doc.lineAt(state.selection.main.head);
  const match = HEADING.exec(line.text);
  const mark = `${"#".repeat(level)} `;
  // The level the line wears now, and how much of it the mark occupies. Zero
  // is "no heading here", which no `#{1,6}` match can otherwise produce.
  const worn = match?.[1]?.length ?? 0;
  const width = match?.[0]?.length ?? 0;
  const changes =
    worn === 0
      ? [{ from: line.from, insert: mark }]
      : worn === level
        ? [{ from: line.from, to: line.from + width, insert: "" }]
        : [{ from: line.from, to: line.from + width, insert: mark }];
  view.dispatch({ changes, userEvent: "input" });
  view.focus();
  return true;
}

/**
 * Put `prefix` on the front of every touched line, or take it off all of them
 * when every one already carries it - the list and task-list buttons.
 *
 * Lines are collected by NUMBER rather than by walking positions: a CRLF
 * buffer's line break is two characters wide, so stepping `line.to + 1` would
 * be reasoning about the middle of a break.
 */
export function toggleLinePrefix(view: EditorView, prefix: string): boolean {
  if (!clearOfFoldedFrontmatter(view)) {
    return false;
  }
  const { state } = view;
  const lines: Line[] = [];
  const seen = new Set<number>();
  for (const range of state.selection.ranges) {
    const first = state.doc.lineAt(range.from).number;
    const last = state.doc.lineAt(range.to).number;
    for (let number = first; number <= last; number++) {
      const line = state.doc.line(number);
      if (!seen.has(line.from)) {
        seen.add(line.from);
        lines.push(line);
      }
    }
  }
  const removing = lines.every((line) => line.text.startsWith(prefix));
  const changes = lines.flatMap((line) =>
    removing
      ? [{ from: line.from, to: line.from + prefix.length, insert: "" }]
      : line.text.startsWith(prefix)
        ? []
        : [{ from: line.from, insert: prefix }],
  );
  if (changes.length === 0) {
    return false;
  }
  view.dispatch({ changes, userEvent: "input" });
  view.focus();
  return true;
}

/** `[[...]]` around the selection, cursor inside for the target's name. */
export function insertWikilink(view: EditorView): boolean {
  if (!clearOfFoldedFrontmatter(view)) {
    return false;
  }
  const changes = view.state.changeByRange((range) => ({
    changes: [
      { from: range.from, insert: "[[" },
      { from: range.to, insert: "]]" },
    ],
    range: EditorSelection.range(range.from + 2, range.to + 2),
  }));
  view.dispatch(changes, { userEvent: "input" });
  view.focus();
  return true;
}

/** The starter table: a header, its rule and one empty row to fill in. */
export const TABLE_SKELETON = [
  "| Column | Column |",
  "| --- | --- |",
  "|  |  |",
] as const;

/** The starter diagram, in the flavor the read view already renders. */
export const MERMAID_SKELETON = [
  "```mermaid",
  "flowchart TD",
  "    A[First step] --> B[Next step]",
  "```",
] as const;

/**
 * Drop a block below the line the cursor is on, never into the middle of it:
 * a table or a fence has to start its own line to parse at all.
 *
 * A blank line goes in front of it unless the cursor line is already empty,
 * so the block is separated from the prose above it, and the cursor lands at
 * the end of the block's first line - the header row, the fence's own line -
 * which is where editing what was just inserted starts.
 */
export function insertBlock(
  view: EditorView,
  lines: readonly string[],
): boolean {
  if (!clearOfFoldedFrontmatter(view)) {
    return false;
  }
  // Read AFTER the guard: it may have moved the caret out of the block.
  const { state } = view;
  const separator = state.lineBreak;
  const line = state.doc.lineAt(state.selection.main.head);
  const lead = line.length === 0 ? "" : separator + separator;
  const block = `${lead}${lines.join(separator)}${separator}`;
  view.dispatch({
    changes: { from: line.to, insert: block },
    selection: { anchor: line.to + lead.length + (lines[0]?.length ?? 0) },
    userEvent: "input",
  });
  view.focus();
  return true;
}

/**
 * Mod-b and Mod-i, the two everybody's fingers already know. Prec.high is
 * load-bearing: defaultKeymap already binds Mod-i (selectParentSyntax, with
 * preventDefault), so at default precedence italic would silently never run
 * regardless of where this keymap sits in the extension array. Nothing else
 * is bound here: Mod-s (save), Mod-z (undo) and Mod-f (search) are owned
 * elsewhere.
 */
export const formattingKeymap: Extension = Prec.high(
  keymap.of([
    { key: "Mod-b", run: (view) => toggleInline(view, "**") },
    { key: "Mod-i", run: (view) => toggleInline(view, "*") },
  ]),
);
