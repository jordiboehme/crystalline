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
import { EditorView, keymap } from "@codemirror/view";

import { frontmatterHidden } from "./frontmatterFold";
import { frontmatterRegion } from "./frontmatterRegion";
import { isIdLineAt, joinSections, stepOffIdLines } from "./hiddenIdLines";
import { roomUndo } from "./roomUndo";

/**
 * Keep a command off a frontmatter block that is currently hidden.
 *
 * The fold is atomic, but atomicity constrains cursor MOTION only - it is read
 * by the movement commands and by pointer selection, never by a dispatched
 * change - so nothing stops a toolbar verb from writing into text nobody can
 * see. And the caret starts there: `CmEditor` sets no initial selection, so a
 * freshly opened engram has its caret at position 0, inside the block. A
 * heading run before the author has clicked anywhere would turn the opening
 * fence into `## ---`, and a table would land between the fence and the first
 * key where nobody can see it - a button that looks like it did nothing while
 * the file stops parsing. (The fold's own guard keeps typing out of the
 * block; a command is a dispatch of its own and is answered here first.)
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
 * the field is not installed, a document whose block lost its closing fence
 * has an empty set, and the MANIFEST surface never carries it. In all three
 * the frontmatter is text on screen like any other and formatting it is
 * legitimate.
 *
 * Exported because it is the shared guard for the assist verbs too, not only
 * for the formatting commands in this file: `tableVerbs` opens every one of
 * its dispatches with it, so a table edit and a bold both refuse the same way
 * over a hidden block rather than each having their own idea of the rule.
 */
export function clearOfFoldedFrontmatter(view: EditorView): boolean {
  // First off a Reflection's hidden id line, where a caret can stand at
  // either end: a verb run from there would write into the id.
  stepOffIdLines(view);
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
 *
 * `joinSections` is the confirmed form of a change the document adapter
 * asked about first (`confirmHeadingChange`): a section heading of a
 * Reflection that stops being one takes its hidden id line with it, in the
 * same transaction, so one undo puts both back. Without it the id line stays
 * where it is. In a Reflection every heading change goes through the
 * adapter's `confirmHeadingChange` first; the format bar does that.
 */
export function cycleHeading(
  view: EditorView,
  level: number,
  options: { joinSections?: boolean } = {},
): boolean {
  return writeHeading(view, (worn) => (worn === level ? 0 : level), options);
}

/**
 * Sets the main selection's line to exactly `level` `#` (0 is no heading).
 * It never toggles - picking the level the line has changes nothing - and
 * writes only the `#` run and its space: from `## x` to level 3 inserts one
 * `#`, to 0 deletes `## `. The text style menu runs it.
 *
 * It is a gesture of its own, one undo step in a room too, and a page that is
 * not writable takes nothing. `joinSections` is `cycleHeading`'s: the bar asks
 * the document adapter first and passes it once the person has confirmed.
 */
export function setHeadingLevel(
  view: EditorView,
  level: 0 | 1 | 2 | 3 | 4,
  options: { joinSections?: boolean } = {},
): boolean {
  if (!view.state.facet(EditorView.editable) || view.state.readOnly) {
    return false;
  }
  const room = view.state.facet(roomUndo);
  room?.stopCapturing();
  const done = writeHeading(view, () => level, options);
  room?.stopCapturing();
  return done;
}

function writeHeading(
  view: EditorView,
  target: (worn: number) => number,
  options: { joinSections?: boolean },
): boolean {
  if (!clearOfFoldedFrontmatter(view)) {
    return false;
  }
  // Read AFTER the guard: it may have moved the caret out of the block.
  const { state } = view;
  const line = state.doc.lineAt(state.selection.main.head);
  const match = HEADING.exec(line.text);
  // The level the line wears now, and how much of it the mark occupies. Zero
  // is "no heading here", which no `#{1,6}` match can otherwise produce.
  const worn = match?.[1]?.length ?? 0;
  const width = match?.[0]?.length ?? 0;
  const level = target(worn);
  const mark = level === 0 ? "" : `${"#".repeat(level)} `;
  const changes: { from: number; to?: number; insert: string }[] = [];
  if (worn !== level) {
    changes.push({ from: line.from, to: line.from + width, insert: mark });
  }
  const joining =
    options.joinSections === true && isIdLineAt(state, line.number + 1);
  if (joining) {
    // The id line and the line break in front of it.
    changes.push({
      from: line.to,
      to: state.doc.line(line.number + 1).to,
      insert: "",
    });
  }
  if (changes.length > 0) {
    view.dispatch({
      changes,
      userEvent: "input",
      ...(joining ? { annotations: joinSections.of(true) } : {}),
    });
  }
  view.focus();
  return true;
}

/**
 * What a numbered list looks like once it exists.
 *
 * The button inserts a literal `1. ` on every line, which is correct markdown
 * - the renderer numbers the items in sequence and an author who reorders
 * them gets the new order for free - but it is not what the second line then
 * READS as, here or after another editor has renumbered the source. Toggling
 * off has to recognise whatever number is actually there, or the button
 * unnumbers the first line and numbers the rest.
 */
export const ORDERED_ITEM = /^\d+\. /;

/**
 * How much of `text` is already this command's prefix: its length when the
 * line carries it, zero when it does not.
 *
 * `pattern` is for the prefixes that are a shape rather than a string. The
 * literal comparison is the rule for every other one, because `- ` and `> `
 * mean themselves and a line that starts `-- ` is not a list item.
 */
function wornPrefix(text: string, prefix: string, pattern?: RegExp): number {
  if (pattern) {
    return pattern.exec(text)?.[0].length ?? 0;
  }
  return text.startsWith(prefix) ? prefix.length : 0;
}

/**
 * Put `prefix` on the front of every touched line, or take it off all of them
 * when every one already carries it - the list, numbered-list, task-list and
 * quote buttons.
 *
 * Lines are collected by NUMBER rather than by walking positions: a CRLF
 * buffer's line break is two characters wide, so stepping `line.to + 1` would
 * be reasoning about the middle of a break.
 */
export function toggleLinePrefix(
  view: EditorView,
  prefix: string,
  pattern?: RegExp,
): boolean {
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
      // A Reflection's hidden id line, and the section heading above it,
      // stay as they are: a prefix on either parts the id from its heading,
      // and the section would vanish into the one above without a word.
      if (isIdLineAt(state, number) || isIdLineAt(state, number + 1)) {
        continue;
      }
      if (!seen.has(line.from)) {
        seen.add(line.from);
        lines.push(line);
      }
    }
  }
  // The width is read once per line and carried, because removal strips what
  // the line actually wears - `10. ` is a character wider than `1. `.
  const worn = lines.map((line) => ({
    line,
    width: wornPrefix(line.text, prefix, pattern),
  }));
  const removing = worn.every((entry) => entry.width > 0);
  const changes = worn.flatMap(({ line, width }) =>
    removing
      ? [{ from: line.from, to: line.from + width, insert: "" }]
      : width > 0
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

/** The placeholders a link arrives with, one of them always selected. */
const LINK_TEXT = "text";
const LINK_URL = "url";

/**
 * `[text](url)` - the link to somewhere that is not an engram.
 *
 * Which half is selected follows from what the author already gave. From a
 * bare cursor nothing is written yet, so the whole link is placeholder and
 * the words come first: `text` is selected and the first keystroke replaces
 * it. From a selection the words exist and are kept - selecting them again
 * would mean the next keystroke DELETES what was just linked - so the caret
 * lands on `url`, the only part still missing.
 */
export function insertMarkdownLink(view: EditorView): boolean {
  if (!clearOfFoldedFrontmatter(view)) {
    return false;
  }
  const tail = `](${LINK_URL})`;
  const changes = view.state.changeByRange((range) => {
    if (range.empty) {
      return {
        changes: [{ from: range.from, insert: `[${LINK_TEXT}${tail}` }],
        range: EditorSelection.range(
          range.from + 1,
          range.from + 1 + LINK_TEXT.length,
        ),
      };
    }
    // Where the address lands: past the bracket that went in front of the
    // words, then past the `](` this tail opens with.
    const url = range.to + "[".length + "](".length;
    return {
      changes: [
        { from: range.from, insert: "[" },
        { from: range.to, insert: tail },
      ],
      range: EditorSelection.range(url, url + LINK_URL.length),
    };
  });
  view.dispatch(changes, { userEvent: "input" });
  view.focus();
  return true;
}

/**
 * A GFM table of the asked-for size: a header of `Column` placeholders, its
 * rule, and the empty rows below it.
 *
 * `rows` counts the header, because that is what the size picker's grid shows
 * - its top row IS the header row - so a 2x2 pick means a header and one row
 * to type in, and the emitted array is one line longer than `rows` for the
 * rule. One column by one row is a header and a rule with no data row, which
 * is still a table; the grid's corner cell would be a lie otherwise.
 *
 * Both counts are clamped to a whole number of at least one, and a size that
 * is not a number at all is one too: `Math.max(1, NaN)` is `NaN`, which would
 * emit a zero-column table with an empty rule row - markdown that does not
 * parse, from a function whose contract says it always returns a table.
 */
export function tableSkeleton(columns: number, rows: number): string[] {
  const atLeastOne = (value: number) =>
    Number.isFinite(value) ? Math.max(1, Math.trunc(value)) : 1;
  const wide = atLeastOne(columns);
  const tall = atLeastOne(rows);
  const row = (cell: string) =>
    `| ${Array.from({ length: wide }, () => cell).join(" | ")} |`;
  return [
    row("Column"),
    row("---"),
    ...Array.from({ length: tall - 1 }, () => row("")),
  ];
}

/**
 * The starter table: a header, its rule and one empty row to fill in.
 *
 * The size picker generates its own sizes now, so nothing in the app reads
 * this - it stays as the historical shape the picker's default has to keep
 * reproducing, and the byte-identity test next door is what says so.
 */
export const TABLE_SKELETON: readonly string[] = tableSkeleton(2, 2);

/** A span of the document, in positions of the state it is read against. */
export interface TextRange {
  from: number;
  to: number;
}

/**
 * Where the caret lands inside a freshly inserted block: an index into the
 * inserted line array plus a character range within that line.
 *
 * Line-local and character-counted, deliberately: a caller building a block
 * knows its own text and nothing about the buffer it will land in, and
 * `insertBlock` turns this into document positions at insertion time, where
 * the separator is finally known.
 */
export interface BlockSelection {
  line: number;
  from: number;
  to: number;
}

/**
 * Find the FIRST occurrence of `token` across a block's lines, or null when
 * the block does not contain it.
 *
 * First occurrence rather than exactly-once by design: a state diagram
 * necessarily repeats a state name, and every block that carries a token is
 * written so its first mention is the one a person edits. Where a token does
 * recur, typing over the selection leaves the later mention behind and it is
 * the person's to follow up.
 */
export function selectToken(
  lines: readonly string[],
  token: string,
): BlockSelection | null {
  for (const [line, text] of lines.entries()) {
    const from = text.indexOf(token);
    if (from !== -1) {
      return { line, from, to: from + token.length };
    }
  }
  return null;
}

/**
 * The starter code block: a bare fence, a line to write on and the close.
 *
 * The fence names no language on purpose. `insertBlock` leaves the caret at
 * the end of the block's first line, which on a bare fence is exactly the
 * language slot, so the first keystroke after the button names the language
 * and one Down arrow reaches the body. A fence that guessed a language would
 * have to be deleted before it could be corrected.
 */
export const CODE_SKELETON = ["```", "", "```"] as const;

/**
 * The starter diagram, in the flavor the read view already renders.
 *
 * The type picker builds its own fence for each of the sixteen starters now,
 * so nothing in the app reads this - it stays as the historical shape the
 * menu's first item has to keep reproducing, and the byte-identity test in
 * `mermaidStarters.test.ts` is what says so.
 */
export const MERMAID_SKELETON = [
  "```mermaid",
  "flowchart TD",
  "    A[First step] --> B[Next step]",
  "```",
] as const;

/**
 * The text `insertBlock` writes below `line` and where the block's first line
 * then begins, for a caller that puts a block below a line itself (a picture
 * moved to a drop landing, a dropped file): the same lead, separator and
 * position arithmetic, so the two can never place a block differently.
 * `start` is in positions of the document after the change. Below the last
 * block, where only line breaks follow, the block takes no line break of its
 * own, so the page does not end in a blank line (P40).
 */
export function blockInsertion(
  state: EditorState,
  line: Line,
  lines: readonly string[],
): { change: { from: number; insert: string }; start: number } {
  const placed = placeBlock(state, line, lines, 0);
  const rest = state.doc.sliceString(line.to);
  if (rest !== "" && rest.trim() === "") {
    const separator = state.lineBreak;
    const { insert } = placed.change;
    return {
      ...placed,
      change: {
        from: placed.change.from,
        insert: insert.slice(0, insert.length - separator.length),
      },
    };
  }
  return placed;
}

/**
 * {@link blockInsertion} with `removed` characters of `line` taken away in
 * the same transaction (the slash menu's `/query`), which the line is read
 * without.
 */
function placeBlock(
  state: EditorState,
  line: Line,
  lines: readonly string[],
  removed: number,
): { change: { from: number; insert: string }; start: number } {
  const separator = state.lineBreak;
  const lead = line.length - removed === 0 ? "" : separator + separator;
  const block = `${lead}${lines.join(separator)}${separator}`;
  // Where the block's first line begins: past the cursor's line and past the
  // lead's breaks, which are two positions when the lead is there at all.
  const start = line.to - removed + (lead === "" ? 0 : 2);
  return { change: { from: line.to, insert: block }, start };
}

/**
 * Drop a block below the line the cursor is on, never into the middle of it:
 * a table or a fence has to start its own line to parse at all.
 *
 * A blank line goes in front of it unless the cursor line is already empty,
 * so the block is separated from the prose above it. Where the caret lands is
 * the caller's to say: a `select` descriptor puts it on the first word worth
 * replacing, so the next keystroke is already content, and without one it
 * lands at the end of the block's first line - the header row, the fence's
 * own line - which is where editing an unlabelled block starts.
 *
 * The mapping counts POSITIONS, never characters of the separator. A line
 * break is one document position however many characters it is written with:
 * the change set splits the inserted text on the state's own separator and
 * rebuilds it as lines, so a CRLF buffer stores `\r\n` as one break costing
 * one position. A caret computed from `separator.length` reads two positions
 * too far per line in a CRLF document - far enough to leave the caret in the
 * rule row of a freshly inserted table - while every LF test stays green.
 *
 * A `select` must describe the very `lines` being inserted: its `line` indexes
 * that array and its `from`/`to` are offsets within that line. Nothing here
 * validates it, because there is nothing honest to fall back to - a descriptor
 * off the end maps past the document and `dispatch` raises rather than editing
 * (atomically, so no half-insert lands). Build the two together, as
 * `selectToken` and `mermaidFence` do, and the pair cannot disagree.
 *
 * `replace` is a range on the cursor's line that goes in the same
 * transaction: the `/query` the slash menu was opened with. The line is read
 * as it will be without it, so a slash typed on an empty line leaves no blank
 * line in front of the block, and one undo puts the typed text back.
 */
export function insertBlock(
  view: EditorView,
  lines: readonly string[],
  select?: BlockSelection | null,
  replace?: TextRange,
): boolean {
  if (!clearOfFoldedFrontmatter(view)) {
    return false;
  }
  // Read AFTER the guard: it may have moved the caret out of the block.
  const { state } = view;
  const line = state.doc.lineAt(state.selection.main.head);
  const removed = replace === undefined ? 0 : replace.to - replace.from;
  const { change, start } = placeBlock(state, line, lines, removed);
  const startOf = (index: number) =>
    start +
    lines
      .slice(0, index)
      .reduce((positions, text) => positions + text.length + 1, 0);
  const selection =
    select == null
      ? { anchor: start + (lines[0]?.length ?? 0) }
      : {
          anchor: startOf(select.line) + select.from,
          head: startOf(select.line) + select.to,
        };
  view.dispatch({
    changes: [
      ...(replace === undefined ? [] : [{ ...replace, insert: "" }]),
      change,
    ],
    selection,
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
