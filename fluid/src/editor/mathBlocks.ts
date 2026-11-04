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
import type { SyntaxNode, Tree } from "@lezer/common";

import { fenceInfo } from "./fenceInfo";
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

/**
 * The formula a `FencedCode` node holds when it is a `math` fence the page
 * draws as one block, or null. Shared with the code box, which leaves to
 * this layer exactly the fences it draws.
 */
export function mathFence(
  state: EditorState,
  node: SyntaxNode,
  sections = state.facet(mathSections),
): MathBlock | null {
  const doc = state.doc;
  // The info word as the page reads it (`fenceInfo`): "``` math", a tilde
  // fence and info after the word are math there too.
  const info = fenceInfo(doc, node);
  if (info === null || info.name !== "math" || info.close === null) {
    return null;
  }
  const first = doc.line(info.open);
  const last = doc.line(info.close);
  const depth = quoteDepth(first.text);
  const quoted: string[] = [];
  for (let number = first.number; number <= last.number; number += 1) {
    quoted.push(unquoted(doc.line(number).text, depth));
  }
  // The fence's own indentation (a list item's): taken from every line, as
  // CommonMark takes it from a fence's content lines.
  const marks = doc.sliceString(info.marks.from, info.marks.to);
  const indent = /^[ \t]*/.exec(quoted[0] ?? "")?.[0].length ?? 0;
  const lines = quoted.map((text) => {
    const lead = /^[ \t]*/.exec(text)?.[0].length ?? 0;
    return text.slice(Math.min(lead, indent));
  });
  // A fence that follows a list marker on its line stays source: its
  // block would have to start mid-line.
  const fence = (lines[0] ?? "").startsWith(marks)
    ? [lines.join("\n"), lines.slice(1, -1).join("\n")]
    : null;
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
  if (fence === null || crosses || last.number <= first.number) {
    return null;
  }
  return {
    from: first.from,
    to: last.to,
    tex: fence[1] ?? "",
    form: "fence",
    open: first.number,
    close: last.number,
    depth,
  };
}

/** Every display formula of `state`, in document order of their ends. */
export function mathBlocks(
  state: EditorState,
  sections = state.facet(mathSections),
): MathBlock[] {
  return readMath(state, sections).blocks;
}

/** A stretch of the document the page reads as formula source. */
export interface MathRegion {
  from: number;
  to: number;
}

/** Whether `doc` holds `$$` anywhere, read chunk by chunk. */
function holdsDollars(doc: Text): boolean {
  let last = "";
  for (const iter = doc.iter(); !iter.next().done;) {
    const chunk = iter.value;
    if ((last + chunk).includes("$$")) {
      return true;
    }
    last = chunk.slice(-1);
  }
  return false;
}

const regionCache = new WeakMap<Tree, Map<boolean, MathRegion[]>>();

/**
 * The stretches of `$$` math the page reads as formula source, which hold
 * no Markdown marks (D41): every closed `$$` block, and an unclosed `$$`
 * from its line to where the page ends it (the document's end, the end of
 * the quote it opened in, or on a page read section by section the next
 * section heading). Math fences are code to the parser already. Read once
 * per parse tree.
 */
export function mathRegions(
  state: EditorState,
  sections = state.facet(mathSections),
): MathRegion[] {
  const tree = syntaxTree(state);
  let cached = regionCache.get(tree);
  const hit = cached?.get(sections);
  if (hit) {
    return hit;
  }
  const regions = holdsDollars(state.doc)
    ? readMath(state, sections).regions
    : [];
  if (!cached) {
    cached = new Map();
    regionCache.set(tree, cached);
  }
  cached.set(sections, regions);
  return regions;
}

/** Whether `pos` lies in one of `regions`. */
export function inMathRegion(
  regions: readonly MathRegion[],
  pos: number,
): boolean {
  return regions.some((region) => region.from <= pos && pos <= region.to);
}

function readMath(
  state: EditorState,
  sections: boolean,
): { blocks: MathBlock[]; regions: MathRegion[] } {
  const doc = state.doc;
  const blocks: MathBlock[] = [];
  const regions: MathRegion[] = [];
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
      const block = mathFence(state, node.node, sections);
      if (block !== null) {
        blocks.push(block);
      }
      return false;
    },
  });
  const fmEnd = frontmatterRegion(doc)?.to ?? -1;
  const inCode = (pos: number) =>
    pos <= fmEnd || code.some((range) => pos >= range.from && pos <= range.to);
  let open: { number: number; depth: number } | null = null;
  // An unclosed `$$` the page ends before line `number`.
  const unclosed = (number: number) => {
    if (open !== null && number - 1 >= open.number) {
      regions.push({
        from: doc.line(open.number).from,
        to: doc.line(number - 1).to,
      });
    }
    open = null;
  };
  for (let number = 1; number <= doc.lines; number += 1) {
    const line = doc.line(number);
    if (inCode(line.from)) {
      continue;
    }
    const depth = quoteDepth(line.text);
    const rest = unquoted(line.text, depth);
    if (open !== null && depth < open.depth) {
      // The quote the block opened in has ended.
      unclosed(number);
    }
    if (
      open !== null &&
      sections &&
      depth === 0 &&
      SECTION_HEADING.test(rest)
    ) {
      unclosed(number);
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
    regions.push({ from: doc.line(open.number).from, to: line.to });
    open = null;
  }
  // Still open at the end: the page reads the rest as the formula.
  unclosed(doc.lines + 1);
  return { blocks, regions };
}
