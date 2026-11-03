/**
 * The hidden-mark model: which source ranges of a line are Markdown marks a
 * reader never sees (`**`, a heading's `## `, a link's `](...)`, a comment).
 *
 * Pure and state only: read off the syntax tree of the one line asked about,
 * with no view and no viewport, so the drawing, the caret rules and the
 * formatting commands all ask the same question and get the same answer.
 * Positions are document positions, never indexes into the file's string.
 *
 * A line whose marks show (`marksHiddenOnLine` false: Raw, or show mode on a
 * touched line) has none, except an HTML comment in a Reflection, which is
 * never shown there. The frontmatter block, code and math hold no marks.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Text } from "@codemirror/state";
import type { SyntaxNode, Tree } from "@lezer/common";

import { htmlCommentRanges } from "../../markdown/htmlComments";
import { isIdLine } from "../../reflectionText";
import { OBSERVATION_LINE } from "../crystallineLines";
import { frontmatterRegion } from "../frontmatterRegion";
import { hidesIdLines } from "../hiddenIdLines";
import { marksHiddenOnLine } from "./mode";

export type MarkKind =
  | "strong"
  | "em"
  | "strike"
  | "code"
  | "link"
  | "heading"
  | "list"
  | "task"
  | "quote"
  | "comment"
  | "category";

export interface Mark {
  from: number;
  to: number;
  kind: MarkKind;
  side: "open" | "close" | "prefix" | "whole";
  /** The index of the paired mark in the same line's array, if it is there. */
  partner: number | null;
}

/** Touching marks taken together: what a caret steps over in one go. */
export interface MarkRun {
  from: number;
  to: number;
  marks: Mark[];
}

/** The subtrees that hold no marks at all. */
const NO_MARKS = new Set([
  "FencedCode",
  "CodeBlock",
  "CodeText",
  "InlineMath",
  "StatusLabel",
  "LinkReference",
]);

/** The inline pairs whose first and last child are the marks. */
const PAIRS = new Map<string, MarkKind>([
  ["Emphasis", "em"],
  ["StrongEmphasis", "strong"],
  ["Strikethrough", "strike"],
  ["InlineCode", "code"],
]);

/** The not-applicable box after a list mark, which the parser reads as text. */
const NOT_APPLICABLE = /^\[~\](?=[ \t]|$)/;

/** A link label as CommonMark matches it to a definition. */
function labelKey(label: string): string {
  return label.trim().replace(/\s+/g, " ").toLowerCase();
}

/** Every link definition's label, once per parse tree. */
const definitionCache = new WeakMap<Tree, Set<string>>();

function definitions(state: EditorState): Set<string> {
  const tree = syntaxTree(state);
  const cached = definitionCache.get(tree);
  if (cached) {
    return cached;
  }
  const labels = new Set<string>();
  tree.iterate({
    enter: (node) => {
      if (node.name === "LinkReference") {
        const label = node.node.getChild("LinkLabel");
        if (label) {
          labels.add(
            labelKey(state.doc.sliceString(label.from + 1, label.to - 1)),
          );
        }
        return false;
      }
      // Definitions are blocks: nothing inline or in code holds one.
      if (
        node.name === "Paragraph" ||
        node.name.startsWith("ATXHeading") ||
        node.name.startsWith("SetextHeading") ||
        NO_MARKS.has(node.name) ||
        node.name === "Table"
      ) {
        return false;
      }
    },
  });
  definitionCache.set(tree, labels);
  return labels;
}

/** Where the frontmatter block ends, once per document; -1 without one. */
const frontmatterCache = new WeakMap<Text, number>();

function frontmatterEnd(doc: Text): number {
  let end = frontmatterCache.get(doc);
  if (end === undefined) {
    end = frontmatterRegion(doc)?.to ?? -1;
    frontmatterCache.set(doc, end);
  }
  return end;
}

/** A mark before its partner index is known: partners are linked by object. */
interface Draft {
  from: number;
  to: number;
  kind: MarkKind;
  side: Mark["side"];
  pair: Draft | null;
}

function isSpace(char: string): boolean {
  return char === " " || char === "\t";
}

/**
 * The marks of line `line` (1-based), sorted by position. Empty when the
 * line's marks show, apart from a Reflection's comments. With `always`, the
 * marks the line has whether they show or not (Raw, show mode on the caret's
 * line, a table cell): what the format commands read a line's formats from.
 */
export function marksOnLine(
  state: EditorState,
  line: number,
  options: { always?: boolean } = {},
): Mark[] {
  const doc = state.doc;
  const at = doc.line(line);
  if (at.from <= frontmatterEnd(doc)) {
    return [];
  }
  const hidden = options.always === true || marksHiddenOnLine(state, line);
  // A Reflection never shows a comment, in either mode.
  const comments = hidden || hidesIdLines(state);
  if (!comments) {
    return [];
  }
  const drafts: Draft[] = [];
  const onLine = (from: number, to: number) =>
    from >= at.from && to <= at.to && from < to;
  const add = (
    from: number,
    to: number,
    kind: MarkKind,
    side: Mark["side"],
  ): Draft | null => {
    if (!onLine(from, to)) {
      return null;
    }
    const draft: Draft = { from, to, kind, side, pair: null };
    drafts.push(draft);
    return draft;
  };
  const pair = (open: Draft | null, close: Draft | null) => {
    if (open && close) {
      open.pair = close;
      close.pair = open;
    }
  };
  const char = (pos: number) => doc.sliceString(pos, pos + 1);
  // A `>` with the one optional space after it.
  const quote = (from: number, to: number) => {
    add(from, isSpace(char(to)) ? to + 1 : to, "quote", "prefix");
  };
  let inCode = false;
  // Where an observation's category or a task box already took a `[`, so the
  // parser's link over the same brackets is not read twice.
  const taken = new Set<number>();
  let defined: Set<string> | null = null;
  const isDefined = (label: string) => {
    defined ??= definitions(state);
    return defined.has(labelKey(label));
  };

  const category = hidden ? OBSERVATION_LINE.exec(at.text) : null;

  const link = (node: SyntaxNode) => {
    const open = node.firstChild;
    if (!open || open.name !== "LinkMark" || taken.has(open.from)) {
      return;
    }
    const image = node.name === "Image";
    let close: SyntaxNode | null = open.nextSibling;
    while (close && !(close.name === "LinkMark" && char(close.from) === "]")) {
      close = close.nextSibling;
    }
    if (!close) {
      return;
    }
    const text = doc.sliceString(open.to, close.from);
    // A footnote reference is Task 10's, never a link.
    if (!image && text.startsWith("^")) {
      return;
    }
    // `[[Target]]`: the wikilink chip's, never a link.
    if (char(node.from - 1) === "[" && char(node.to) === "]") {
      return;
    }
    const after = close.nextSibling;
    let end = close.to;
    if (after?.name === "LinkMark" && char(after.from) === "(") {
      end = node.to;
    } else {
      // A reference: full (`][ref]`), collapsed (`][]`) or shortcut (`]`).
      let label = text;
      if (after?.name === "LinkLabel") {
        end = after.to;
        const written = doc.sliceString(after.from + 1, after.to - 1);
        if (written.trim() !== "") {
          label = written;
        }
      }
      if (!isDefined(label)) {
        return;
      }
    }
    pair(
      add(open.from, open.to, "link", "open"),
      add(close.from, end, "link", "close"),
    );
  };

  syntaxTree(state).iterate({
    from: at.from,
    to: at.to,
    enter: (ref) => {
      const name = ref.name;
      if (name === "Comment" || name === "CommentBlock") {
        if (!isIdLine(at.text)) {
          const text = doc.sliceString(ref.from, ref.to);
          for (const comment of htmlCommentRanges(text)) {
            add(
              Math.max(ref.from + comment.from, at.from),
              Math.min(ref.from + comment.to, at.to),
              "comment",
              "whole",
            );
          }
        }
        return false;
      }
      if (!hidden) {
        return;
      }
      if (NO_MARKS.has(name)) {
        if (name === "FencedCode" || name === "CodeBlock") {
          if (ref.from <= at.from && ref.to >= at.from) {
            inCode = true;
          }
          // In a quote or a callout the parser hangs each code line's `>`
          // under the block: those are the quote's marks, not code.
          for (const mark of ref.node.getChildren("QuoteMark")) {
            quote(mark.from, mark.to);
          }
        }
        return false;
      }
      const node = ref.node;
      const pairKind = PAIRS.get(name);
      if (pairKind !== undefined) {
        // `$`...`$` is inline math, drawn by `elementPreviews.ts`.
        const math =
          name === "InlineCode" &&
          char(ref.from - 1) === "$" &&
          doc.sliceString(ref.from - 2, ref.from - 1) !== "\\" &&
          char(ref.to) === "$";
        const first = node.firstChild;
        const last = node.lastChild;
        if (!math && first && last && first.from !== last.from) {
          pair(
            add(first.from, first.to, pairKind, "open"),
            add(last.from, last.to, pairKind, "close"),
          );
        }
        // Inside code nothing is a mark; inside emphasis, more may be.
        return name === "InlineCode" ? false : undefined;
      }
      if (name === "Link" || name === "Image") {
        link(node);
        return;
      }
      if (name === "Autolink") {
        const first = node.firstChild;
        const last = node.lastChild;
        if (first?.name === "LinkMark" && last?.name === "LinkMark") {
          pair(
            add(first.from, first.to, "link", "open"),
            add(last.from, last.to, "link", "close"),
          );
        }
        return false;
      }
      if (name.startsWith("ATXHeading")) {
        const open = node.firstChild;
        const close = node.lastChild;
        // Only with the space after it: a `#` run alone is a heading being typed.
        const prefix =
          open?.name === "HeaderMark" && isSpace(char(open.to))
            ? add(open.from, open.to + 1, "heading", "prefix")
            : null;
        if (
          prefix &&
          close &&
          close.from !== prefix.from &&
          close.name === "HeaderMark"
        ) {
          pair(prefix, add(close.from, close.to, "heading", "close"));
        }
        return;
      }
      if (name === "ListMark") {
        if (isSpace(char(ref.to))) {
          add(ref.from, ref.to + 1, "list", "prefix");
          const box = ref.to + 1;
          if (NOT_APPLICABLE.test(doc.sliceString(box, box + 4))) {
            taken.add(box);
            add(
              box,
              isSpace(char(box + 3)) ? box + 4 : box + 3,
              "task",
              "prefix",
            );
          }
        }
        return;
      }
      if (name === "TaskMarker") {
        taken.add(ref.from);
        add(
          ref.from,
          isSpace(char(ref.to)) ? ref.to + 1 : ref.to,
          "task",
          "prefix",
        );
        return;
      }
      if (name === "QuoteMark") {
        quote(ref.from, ref.to);
        return;
      }
    },
  });

  if (category && !inCode) {
    const open = at.from + at.text.indexOf("[");
    const close = open + (category[1] ?? "").length + 1;
    taken.add(open);
    // The link the parser read over these brackets, if any, is dropped.
    for (let index = drafts.length - 1; index >= 0; index -= 1) {
      const draft = drafts[index];
      if (draft?.kind === "link" && draft.from === open) {
        drafts.splice(index, 1);
        if (draft.pair) {
          drafts.splice(drafts.indexOf(draft.pair), 1);
        }
      }
    }
    pair(
      add(open, open + 1, "category", "open"),
      add(close, close + 1, "category", "close"),
    );
  }

  drafts.sort((a, b) => a.from - b.from || a.to - b.to);
  return drafts.map((draft) => ({
    from: draft.from,
    to: draft.to,
    kind: draft.kind,
    side: draft.side,
    partner: draft.pair ? drafts.indexOf(draft.pair) : null,
  }));
}

/** The marks of line `line` grouped into runs of touching marks. */
export function runsOnLine(state: EditorState, line: number): MarkRun[] {
  return runsOf(marksOnLine(state, line));
}

/** Sorted marks grouped into runs of touching marks. */
export function runsOf(marks: readonly Mark[]): MarkRun[] {
  const runs: MarkRun[] = [];
  for (const mark of marks) {
    const last = runs.at(-1);
    if (last && last.to >= mark.from) {
      last.to = Math.max(last.to, mark.to);
      last.marks.push(mark);
    } else {
      runs.push({ from: mark.from, to: mark.to, marks: [mark] });
    }
  }
  return runs;
}

/** The run with `from <= pos <= to` on `pos`'s line, or null. */
export function runAt(state: EditorState, pos: number): MarkRun | null {
  const line = state.doc.lineAt(pos).number;
  return (
    runsOnLine(state, line).find((run) => run.from <= pos && pos <= run.to) ??
    null
  );
}
