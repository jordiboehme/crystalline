/**
 * The display formulas of a document, read the way the reading page reads
 * them: a `math` fence and a `$$` block, outside code and the frontmatter.
 * Both may stand in a quote: their lines are read without the quote's marks,
 * and a `$$` closes only at the depth it opened, inside the same quote. On a
 * page read one level-2 section at a time (a Reflection page) a section
 * heading ends an open `$$`.
 *
 * Shared by the preview, which draws them, and the formula popover's verb,
 * which rewrites a formula's lines, so both agree on where a block is.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Text } from "@codemirror/state";
import { Facet } from "@codemirror/state";

import { frontmatterRegion } from "./frontmatterRegion";
import { CODE_CONTEXTS } from "./prose";

/** Whether this buffer's page is read one level-2 section at a time. */
export const mathSections = Facet.define<boolean, boolean>({
  combine: (values) => values.some(Boolean),
});

/** The source and the span of one display formula. */
export interface MathBlock {
  from: number;
  to: number;
  tex: string;
  /** A `math` fence or a `$$` block. */
  form: "fence" | "dollars";
  /** The document lines of its opening and closing marks. */
  open: number;
  close: number;
  /** How many quote marks its lines carry. */
  depth: number;
}

/** A line's quote marks: how many, and how long the run of them is. */
const QUOTE_RUN = /^[ \t]*((?:>[ \t]?)*)/;

export function quoteDepth(text: string): number {
  return (QUOTE_RUN.exec(text)?.[1] ?? "").split(">").length - 1;
}

/** The first `depth` quote marks of `text` as written (with a space after each). */
export function quotePrefix(text: string, depth: number): string {
  let rest = text;
  let prefix = "";
  for (let mark = 0; mark < depth; mark += 1) {
    const found = /^[ \t]*>[ \t]?/.exec(rest);
    if (found === null) {
      break;
    }
    prefix += found[0];
    rest = rest.slice(found[0].length);
  }
  return prefix;
}

/** `text` without its first `depth` quote marks (and a space after each). */
export function unquoted(text: string, depth: number): string {
  return text.slice(quotePrefix(text, depth).length);
}

/**
 * Whether the character at `pos` is escaped: an odd run of backslashes
 * stands right before it (two backslashes and a dollar sign are an escaped
 * backslash and a dollar sign), as the reading page tests it.
 */
export function escapedAt(doc: Text, pos: number): boolean {
  let run = 0;
  while (
    pos - run - 1 >= 0 &&
    doc.sliceString(pos - run - 1, pos - run) === "\\"
  ) {
    run += 1;
  }
  return run % 2 === 1;
}

/**
 * An inline formula's text as the page reads it: one space taken from each
 * end when both ends have one and it is not all spaces, the rule CommonMark
 * keeps for a code span and the math parser for `$$...$$`.
 */
export function unpadded(text: string): string {
  return text.length >= 2 &&
    text.startsWith(" ") &&
    text.endsWith(" ") &&
    text.trim() !== ""
    ? text.slice(1, -1)
    : text;
}

/** A page section's heading, which ends anything a section holds. */
export const SECTION_HEADING = /^##(?:[ \t]|$)/;

/** Every display formula of `state`, in document order of their ends. */
export function mathBlocks(
  state: EditorState,
  sections = state.facet(mathSections),
): MathBlock[] {
  const doc = state.doc;
  const blocks: MathBlock[] = [];
  const code: { from: number; to: number }[] = [];
  syntaxTree(state).iterate({
    enter: (node) => {
      if (!CODE_CONTEXTS.has(node.name) || node.name === "InlineCode") {
        return;
      }
      code.push({ from: node.from, to: node.to });
      if (node.name !== "FencedCode") {
        return false;
      }
      const first = doc.lineAt(node.from);
      const last = doc.lineAt(node.to);
      const depth = quoteDepth(first.text);
      const lines: string[] = [];
      for (let number = first.number; number <= last.number; number += 1) {
        lines.push(unquoted(doc.line(number).text, depth));
      }
      // A fence that does not start its line (one in a list item) stays
      // source: its block would have to start mid-line.
      const fence = /^```math[ \t]*\n([\s\S]*?)\n?```[ \t]*$/.exec(
        lines.join("\n"),
      );
      // On a page read section by section, a fence that runs over a section
      // heading is not one block on the page: draw none.
      const crosses =
        sections &&
        lines.some(
          (text, index) =>
            index > 0 &&
            quoteDepth(doc.line(first.number + index).text) === 0 &&
            SECTION_HEADING.test(text),
        );
      if (fence !== null && !crosses && last.number > first.number) {
        blocks.push({
          from: first.from,
          to: last.to,
          tex: fence[1] ?? "",
          form: "fence",
          open: first.number,
          close: last.number,
          depth,
        });
      }
      return false;
    },
  });
  const fmEnd = frontmatterRegion(doc)?.to ?? -1;
  const inCode = (pos: number) =>
    pos <= fmEnd || code.some((range) => pos >= range.from && pos <= range.to);
  let open: { number: number; depth: number } | null = null;
  for (let number = 1; number <= doc.lines; number += 1) {
    const line = doc.line(number);
    if (inCode(line.from)) {
      continue;
    }
    const depth = quoteDepth(line.text);
    const rest = unquoted(line.text, depth);
    if (open !== null && depth < open.depth) {
      // The quote the block opened in has ended.
      open = null;
    }
    if (
      open !== null &&
      sections &&
      depth === 0 &&
      SECTION_HEADING.test(rest)
    ) {
      open = null;
      continue;
    }
    if (rest.trim() !== "$$") {
      continue;
    }
    if (open !== null && depth > open.depth) {
      // A deeper line is the formula's content, as the reader reads it.
      continue;
    }
    if (open === null) {
      open = { number, depth };
      continue;
    }
    const tex: string[] = [];
    for (let inner = open.number + 1; inner < number; inner += 1) {
      tex.push(unquoted(doc.line(inner).text, depth));
    }
    blocks.push({
      from: doc.line(open.number).from,
      to: line.to,
      tex: tex.join("\n"),
      form: "dollars",
      open: open.number,
      close: number,
      depth,
    });
    open = null;
  }
  return blocks;
}
