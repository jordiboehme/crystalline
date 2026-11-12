/**
 * Math in the forms that cannot be mistaken for prose: `$$...$$`, a
 * ```` ```math ```` fence, and GitHub's inline form `` $`...`$ ``.
 *
 * A single `$` is never math (`singleDollarTextMath: false`), so a price, a
 * shell variable or a pair of dollar amounts stays text. The inline form
 * reaches remark as inline code between two dollar signs; the pass below
 * turns that into the same `inlineMath` node remark-math makes for `$$x$$`,
 * and a `math` fence into the same `math` node as a `$$` block.
 *
 * The positions stay true: the text on each side loses only the dollar sign
 * and keeps its own offsets, so source spans still read the source.
 */

import remarkMath from "remark-math";

import type { Root } from "./callouts";

interface Point {
  line: number;
  column: number;
  offset?: number;
}

interface MdNode {
  type: string;
  value?: string;
  lang?: string | null;
  children?: MdNode[];
  position?: { start: Point; end: Point };
  data?: Record<string, unknown>;
}

/**
 * What remark-math's own parser attaches to its nodes, so the hast step
 * turns ours into the same elements: an inline `code.language-math.math-inline`
 * and a block `pre > code.language-math.math-display`.
 */
function inlineData(value: string): Record<string, unknown> {
  return {
    hName: "code",
    hProperties: { className: ["language-math", "math-inline"] },
    hChildren: [{ type: "text", value }],
  };
}

function blockData(value: string): Record<string, unknown> {
  return {
    hName: "pre",
    hChildren: [
      {
        type: "element",
        tagName: "code",
        properties: { className: ["language-math", "math-display"] },
        children: [{ type: "text", value }],
      },
    ],
  };
}

/** remark-math with the single-dollar form off. */
export const remarkMathOptions = { singleDollarTextMath: false } as const;
export { remarkMath };

/**
 * Whether the character at `at` is escaped: an odd run of backslashes
 * stands right before it (`\\$` is an escaped backslash and a dollar).
 */
function escaped(source: string, at: number): boolean {
  let run = 0;
  while (at - run - 1 >= 0 && source[at - run - 1] === "\\") {
    run += 1;
  }
  return run % 2 === 1;
}

function walk(node: MdNode, source: string): void {
  const kids = node.children;
  if (!kids) {
    return;
  }
  for (let i = 0; i < kids.length; i += 1) {
    const kid: MdNode = kids[i]!;
    if (kid.type === "code" && kid.lang === "math") {
      kids[i] = { ...kid, type: "math", data: blockData(kid.value ?? "") };
      continue;
    }
    if (kid.type === "inlineCode") {
      const before = kids[i - 1];
      const after = kids[i + 1];
      const start = kid.position?.start.offset;
      const end = kid.position?.end.offset;
      if (
        before?.type === "text" &&
        after?.type === "text" &&
        start !== undefined &&
        end !== undefined &&
        before.value?.endsWith("$") === true &&
        after.value?.startsWith("$") === true &&
        source[start - 1] === "$" &&
        !escaped(source, start - 1) &&
        source[end] === "$" &&
        before.position?.end.offset === start &&
        after.position?.start.offset === end
      ) {
        const left = before.position;
        const right = after.position;
        const rest = (after.value ?? "").slice(1);
        const opens: Point = {
          line: left.end.line,
          column: left.end.column - 1,
          offset: start - 1,
        };
        const closes: Point = {
          line: right.start.line,
          column: right.start.column + 1,
          offset: end + 1,
        };
        before.value = (before.value ?? "").slice(0, -1);
        before.position = { start: left.start, end: opens };
        after.value = rest;
        after.position = { start: closes, end: right.end };
        kids[i] = {
          type: "inlineMath",
          value: kid.value ?? "",
          data: inlineData(kid.value ?? ""),
          position: { start: opens, end: closes },
        };
        continue;
      }
    }
    walk(kid, source);
  }
  // A text node that held only the dollar sign is empty now.
  node.children = kids.filter((k) => !(k.type === "text" && k.value === ""));
}

/** The remark pass for the GitHub forms; runs after `remarkMath`. */
export function remarkGithubMath(): (
  tree: Root,
  file?: { value?: unknown },
) => void {
  return (tree, file) => {
    walk(tree, typeof file?.value === "string" ? file.value : "");
  };
}
