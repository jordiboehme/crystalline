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

import type { Extension, Line } from "@codemirror/state";
import { EditorSelection, Prec } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { keymap } from "@codemirror/view";

/**
 * Wrap or unwrap every selection range in `marker` - bold, italic and inline
 * code are the same operation with a different string.
 *
 * An empty range is the useful cursor case rather than a no-op: it leaves the
 * cursor between a fresh pair of markers, ready to type into, and toggling
 * again from that same spot removes the pair it just made.
 */
export function toggleInline(view: EditorView, marker: string): boolean {
  const changes = view.state.changeByRange((range) => {
    const { from, to } = range;
    const before = view.state.sliceDoc(Math.max(0, from - marker.length), from);
    const after = view.state.sliceDoc(
      to,
      Math.min(view.state.doc.length, to + marker.length),
    );
    if (before === marker && after === marker) {
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
