/**
 * The one-walk mark scan (`scanMarks`, and `marksOnLine` through it) gives
 * every line exactly the marks the line-by-line read gave before it: the
 * drawing, the caret rules and the deletion check all read them. The
 * reference below is that read as it stood, kept verbatim apart from two
 * module helpers it reaches through their exported forms. It runs over the
 * dialect fixture (LF, CRLF, a Reflection, Raw) and a few corpus files, with
 * marks hidden, shown on a caret's lines and read `always`, line by line,
 * per paragraph and over the whole page in one scan.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
import { EditorSelection } from "@codemirror/state";
import type { SyntaxNode } from "@lezer/common";
import { afterEach, describe, expect, it } from "vitest";

import { htmlCommentRanges } from "../../markdown/htmlComments";
import { isIdLine } from "../../reflectionText";
import { cleanupMarkViews, markView } from "../../test/markKit";
import { repoMarkdown } from "../../test/repoMarkdown";
import { observationCategory } from "../crystallineLines";
import { inMathRegion, mathRegions } from "../mathBlocks";
import { frontmatterRegion } from "../frontmatterRegion";
import { hidesIdLines, pageHidesComments } from "../hiddenIdLines";
import { partnerAcrossLines } from "./deleting";
import type { Mark, MarkKind } from "./hiddenMarks";
import {
  linkDefinition,
  marksOnLine,
  scanMarks,
  treeWalkWork,
} from "./hiddenMarks";
import { marksHiddenOnLine } from "./mode";

afterEach(cleanupMarkViews);

const NO_MARKS = new Set([
  "FencedCode",
  "CodeBlock",
  "CodeText",
  "InlineMath",
  "StatusLabel",
  "LinkReference",
]);

const PAIRS = new Map<string, MarkKind>([
  ["Emphasis", "em"],
  ["StrongEmphasis", "strong"],
  ["Strikethrough", "strike"],
  ["InlineCode", "code"],
]);

const NOT_APPLICABLE = /^\[~\](?=[ \t]|$)/;

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

function referenceMarksOnLine(
  state: EditorState,
  line: number,
  options: { always?: boolean } = {},
): Mark[] {
  const doc = state.doc;
  const at = doc.line(line);
  if (at.from <= (frontmatterRegion(doc)?.to ?? -1)) {
    return [];
  }
  const hidden = options.always === true || marksHiddenOnLine(state, line);
  // A Reflection never shows a comment, in either mode, in a table cell too.
  const comments =
    hidden || hidesIdLines(state) || state.facet(pageHidesComments);
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
  const isDefined = (label: string) =>
    linkDefinition(state, label) !== undefined;

  // `$$` math the page draws: only its quote marks are read (D41).
  const inMath = inMathRegion(mathRegions(state), at.from);
  const category = hidden && !inMath ? observationCategory(at.text) : null;

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
    const after = close.nextSibling;
    // `[[Target]]`: the wikilink chip's, never a link (a bare bracket link
    // only, D39).
    if (
      !image &&
      after === null &&
      char(node.from - 1) === "[" &&
      char(node.to) === "]"
    ) {
      return;
    }
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
      if (inMath && name !== "QuoteMark") {
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
    const open = at.from + category.from;
    const close = at.from + category.to - 1;
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

const DIALECT = import.meta.glob<string>("../fixtures/dialect/*.md", {
  query: "?raw",
  import: "default",
  eager: true,
});

const EVERYTHING = Object.values(DIALECT)[0] ?? "";

/** A few corpus files with code, quotes, tables, lists and wrapped prose. */
const CORPUS = new Set([
  "README.md",
  "docs/README.md",
  "docs/deployment.md",
  "release-notes/v0.19.3.md",
  "release-notes/v0.19.4.md",
]);

interface Page {
  name: string;
  text: string;
  adapter?: "engram" | "reflection";
  raw?: boolean;
  /** Put the caret on every line in turn, not on four. */
  everyLine?: boolean;
}

const PAGES: Page[] = [
  { name: "everything.md", text: EVERYTHING },
  { name: "everything.md (CRLF)", text: EVERYTHING.replaceAll("\n", "\r\n") },
  {
    name: "everything.md (Reflection)",
    text: EVERYTHING,
    adapter: "reflection",
  },
  {
    name: "everything.md (Reflection, Raw)",
    text: EVERYTHING,
    adapter: "reflection",
    raw: true,
  },
  {
    // The shapes a line-by-line read and one walk could part on: a code
    // block in a quote (its `>` hang under the block), a comment over
    // lines, pairs and links that wrap, tasks, observations, a heading.
    name: "wrapped shapes",
    text: [
      "---",
      "title: t",
      "---",
      "",
      "# A heading with **bold** #",
      "",
      "> A quote with code:",
      ">",
      "> ```js",
      "> const a = `x` + **y**; <!-- c -->",
      "> - [idea] not an observation",
      "> ```",
      ">",
      ">     an indented *code* line",
      "",
      "Text <!-- a comment",
      "over **two** lines --> and **a bold",
      "that wraps** and [a link",
      "that wraps](u) and `code",
      "that wraps` and ~~strike",
      "that wraps~~ end.",
      "",
      "- [ ] a task with *em*",
      "- [~] not applicable with [ref]",
      "- [idea] an observation with `code`",
      "  - [x] done <https://example.com>",
      "",
      "```",
      "- [idea] not one either",
      "```",
      "",
      "[ref]: https://example.com",
    ].join("\n"),
    everyLine: true,
  },
  ...repoMarkdown()
    .filter(([name]) => CORPUS.has(name))
    .map(([name, text]) => ({ name, text })),
];

const SHAPES = PAGES.find((page) => page.name === "wrapped shapes");
if (SHAPES) {
  PAGES.push({
    ...SHAPES,
    name: "wrapped shapes (Reflection)",
    adapter: "reflection",
  });
}

const KINDS = new Set<MarkKind>(["strong", "em", "strike", "code", "link"]);

/** The paragraphs and other top blocks, as line ranges. */
function blocks(state: EditorState): { first: number; last: number }[] {
  const out: { first: number; last: number }[] = [];
  const top = syntaxTree(state).topNode;
  for (let node = top.firstChild; node; node = node.nextSibling) {
    out.push({
      first: state.doc.lineAt(node.from).number,
      last: state.doc.lineAt(Math.min(node.to, state.doc.length)).number,
    });
  }
  return out;
}

/** Every way of asking, against the reference, on one state. */
function compare(state: EditorState, where: string): number {
  const lines = state.doc.lines;
  let checked = 0;
  for (const always of [false, true]) {
    const options = { always };
    const whole = scanMarks(state, 1, lines, options);
    for (let n = 1; n <= lines; n += 1) {
      const want = referenceMarksOnLine(state, n, options);
      const at = `${where}, line ${String(n)}, always ${String(always)}`;
      expect(marksOnLine(state, n, options), at).toEqual(want);
      expect(whole.lines[n - 1], `${at}, whole page`).toEqual(want);
      checked += want.length;
    }
    for (const { first, last } of blocks(state)) {
      const scan = scanMarks(state, first, last, options);
      for (let n = first; n <= last; n += 1) {
        expect(
          scan.lines[n - first],
          `${where}, line ${String(n)}, always ${String(always)}, block ${String(first)}-${String(last)}`,
        ).toEqual(referenceMarksOnLine(state, n, options));
      }
      // An opening mark finds its closer as the tree gives it.
      if (!always) continue;
      for (const marks of scan.lines) {
        for (const mark of marks) {
          if (mark.side !== "open" || !KINDS.has(mark.kind)) continue;
          expect(
            scan.closers.get(mark.from) ?? null,
            `${where}, the closer of the mark at ${String(mark.from)}`,
          ).toEqual(partnerAcrossLines(state, mark));
        }
      }
    }
  }
  return checked;
}

describe("the one-walk mark scan", () => {
  it.each(PAGES)("gives every line the marks it had, $name", (page) => {
    const view = markView(page.text, {
      mode: "show",
      ...(page.adapter ? { adapter: page.adapter } : {}),
      ...(page.raw ? { raw: true } : {}),
    });
    let checked = compare(view.state, "no caret");
    const lines = view.state.doc.lines;
    // Show mode: the caret's lines show their marks, the rest hide them.
    const carets = page.everyLine
      ? Array.from({ length: lines }, (_, i) => i + 1)
      : [2, Math.floor(lines / 3), Math.floor(lines / 2), lines];
    for (const n of carets) {
      const line = view.state.doc.line(Math.max(1, Math.min(n, lines)));
      view.dispatch({ selection: EditorSelection.cursor(line.from) });
      checked += compare(view.state, `caret on line ${String(line.number)}`);
    }
    // A selection over several lines shows all of them.
    const from = view.state.doc.line(Math.max(1, Math.floor(lines / 4))).from;
    const to = view.state.doc.line(Math.max(1, Math.floor(lines / 4) + 6)).to;
    view.dispatch({ selection: EditorSelection.range(from, to) });
    checked += compare(view.state, "a selection");
    if (!page.raw) expect(checked).toBeGreaterThan(0);
  });

  it("reads a long hard-wrapped paragraph in one walk", () => {
    const line = "word **bold** and *em* with `code` and [link](u) end";
    const steps = (count: number) => {
      const text = Array.from({ length: count }, () => line).join("\n");
      const view = markView(text, { mode: "hidden" });
      treeWalkWork.steps = 0;
      const scan = scanMarks(view.state, 1, count, { always: true });
      expect(scan.lines.flat()).toHaveLength(count * 8);
      return treeWalkWork.steps;
    };
    // Five times the lines: about five times the steps, not twenty-five.
    const small = steps(40);
    const large = steps(200);
    expect(large / small).toBeLessThanOrEqual(5 * 1.25);
  });
});
