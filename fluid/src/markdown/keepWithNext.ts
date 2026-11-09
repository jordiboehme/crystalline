/**
 * On paper a heading is never the last line of a page. Each top-level
 * heading is wrapped with what comes after it in a `<div data-keep>`, which
 * the print sheet lays out as a box that may not break (`index.css`). On
 * screen the wrapper is `display: contents`, so it draws no box and moves
 * nothing. There are three ways to keep a heading, by what follows it:
 *
 * - A diagram or a picture of its own (`data-keep="unit"`): it never breaks
 *   and prints no taller than a page less room for its heading, so the pair
 *   is one inline block on paper, which a browser moves whole to the next
 *   page. WebKit printing left a heading behind with a plain block here.
 * - A block that never breaks on paper (code, a quote, a callout, a short
 *   table), or one short enough to keep, about half a page
 *   (`data-keep=""`): the pair is one block that may not break. A block that
 *   never breaks leaves the space at the foot of a page either way, so its
 *   heading goes with it whatever its length. The height is an estimate from
 *   the text, so a pair can come out taller than a page; the browser then
 *   breaks it like any other block, and nothing is cut off. It is not an
 *   inline block: that holds together only when it fits on a page, which
 *   nothing caps here.
 * - A block too tall to keep and that may break (a long paragraph, a long
 *   list, a long table, a paragraph with a picture whose height is not known
 *   yet, or the next heading) (`data-keep="lead"`): keeping it with its
 *   heading could leave a page half empty. The wrapper holds the heading
 *   alone and carries how many lines of the block must print with it
 *   (`--keep-lines`): the print sheet reserves that room under the heading.
 *
 * It also marks a table short enough to keep on one page and a list item
 * short enough to keep whole (`data-short`); a longer one breaks between its
 * rows or lines.
 *
 * Only the document's own top level is grouped; a heading inside a callout
 * or a list stays where it is. It runs last in the chain, after the plugins
 * that read headings and their neighbours.
 */

interface Node {
  type: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  children?: Node[];
  value?: string;
}

/** The attribute the wrapper carries; the print sheet keys on it. */
export const KEEP_ATTR = "data-keep";

/**
 * Its value on a heading kept with a diagram or a picture, a pair whose
 * height the print sheet caps and which prints as one inline block.
 */
export const KEEP_UNIT = "unit";

/**
 * Its value on a heading kept with only the first lines of what follows:
 * the wrapper holds the heading alone and reserves room under it on paper.
 */
export const KEEP_LEAD = "lead";

/** The custom property that carries a lead's room, in lines of text. */
export const LEAD_VAR = "--keep-lines";

/** Lines a paragraph keeps at the foot of a page (`orphans` on paper). */
const ORPHAN_LINES = 3;

/** Lines a heading takes with its space around it, roughly. */
const HEADING_LINES = 3;

/** The most room a lead reserves: never more than a short block. */
const MAX_LEAD_LINES = 8;

/** About half an A4 page of body text, in lines. */
const HALF_PAGE_LINES = 24;

/** Characters on one printed line of body text, roughly. */
const LINE_CHARS = 95;

/** Characters on one printed line of code, which is set wider. */
const CODE_CHARS = 80;

/** Rows a table may have and still be kept on one page whole. */
const SHORT_TABLE_ROWS = 12;

/** Lines a list item may take and still be kept whole. */
const SHORT_ITEM_LINES = 6;

const HEADING = /^h[1-6]$/;

const LIST = /^[uo]l$/;

function textOf(node: Node): string {
  return node.type === "text"
    ? (node.value ?? "")
    : (node.children ?? []).map(textOf).join("");
}

function elements(node: Node, tag: string): Node[] {
  const found: Node[] = [];
  const walk = (n: Node) => {
    if (n.type === "element" && n.tagName === tag) {
      found.push(n);
    }
    (n.children ?? []).forEach(walk);
  };
  walk(node);
  return found;
}

/** Lines `chars` characters take, at `width` to a line; at least one. */
function wrapped(chars: number, width: number): number {
  return Math.max(1, Math.ceil(chars / width));
}

/** Lines a run of inline content takes: each hard break starts a line. */
function inlineLines(nodes: Node[]): number {
  let lines = 0;
  let run = "";
  for (const node of nodes) {
    if (node.type === "element" && node.tagName === "br") {
      lines += wrapped(run.length, LINE_CHARS);
      run = "";
    } else {
      run += textOf(node);
    }
  }
  return lines + wrapped(run.trim().length, LINE_CHARS);
}

/** Whether a node is a block of its own rather than inline content. */
function isBlock(node: Node): boolean {
  return (
    node.type === "element" &&
    /^(p|h[1-6]|ul|ol|li|pre|table|blockquote|div|details|summary|figure|hr|section|aside|dl)$/.test(
      node.tagName ?? "",
    )
  );
}

/** Lines the children of a container take, block by block. */
function contentLines(node: Node): number {
  const kids = (node.children ?? []).filter(
    (kid) => kid.type === "element" || textOf(kid).trim() !== "",
  );
  if (!kids.some(isBlock)) {
    return inlineLines(kids);
  }
  return kids.reduce(
    (sum, kid) => sum + (isBlock(kid) ? blockLines(kid) : inlineLines([kid])),
    0,
  );
}

/** Lines one list item takes, its nested lists included. */
function itemLines(item: Node): number {
  return contentLines(item);
}

/** Lines a block takes on paper, its space after included. */
function blockLines(node: Node): number {
  switch (node.tagName) {
    case "table":
      // A row is as tall as its longest cell, set in its share of the line.
      return elements(node, "tr").reduce((sum, row) => {
        const cells = (row.children ?? []).filter((c) => c.type === "element");
        const width = LINE_CHARS / Math.max(1, cells.length);
        const tallest = Math.max(
          1,
          ...cells.map((cell) => wrapped(textOf(cell).length, width)),
        );
        return sum + tallest * 1.2 + 0.4;
      }, 1);
    case "ul":
    case "ol":
      return (
        (node.children ?? [])
          .filter((kid) => kid.type === "element" && kid.tagName === "li")
          .reduce((sum, item) => sum + itemLines(item), 0) + 1
      );
    case "pre": {
      // A diagram draws taller than its source reads.
      const factor = /language-mermaid/.test(
        JSON.stringify(node.children?.[0]?.properties ?? {}),
      )
        ? 2.5
        : 1;
      const lines = textOf(node)
        .replace(/\n$/, "")
        .split("\n")
        .reduce((sum, line) => sum + wrapped(line.length, CODE_CHARS), 0);
      return lines * factor + 1;
    }
    case "hr":
      return 1;
    default:
      return contentLines(node) + 1;
  }
}

/**
 * How many printed lines a block takes, roughly, or null when it cannot be
 * told before it is drawn (a picture). A quote or a callout counts what it
 * holds; a hard break, a wrapped code line and a table cell that wraps each
 * count as the lines they make.
 */
export function linesOf(node: Node): number | null {
  if (elements(node, "img").length > 0) {
    return null;
  }
  return blockLines(node);
}

/** Whether a block is a mermaid fence, which prints as one diagram. */
function isDiagram(node: Node): boolean {
  const code = node.children?.find((kid) => kid.type === "element");
  const classes = code?.properties?.className;
  return (
    node.tagName === "pre" &&
    code?.tagName === "code" &&
    Array.isArray(classes) &&
    classes.includes("language-mermaid")
  );
}

/** The element children of a node, past whitespace between them. */
function solid(node: Node): Node[] {
  return (node.children ?? []).filter(
    (kid) => kid.type !== "text" || textOf(kid).trim() !== "",
  );
}

/**
 * Whether a block is a picture of its own: a paragraph holding one image and
 * nothing else, or one link around one image. It prints no taller than a
 * page less room for its heading (`index.css`), like a diagram. A picture in
 * a sentence is part of the text and is not one.
 */
export function isPicture(node: Node): boolean {
  if (node.tagName !== "p") {
    return false;
  }
  const kids = solid(node);
  if (kids.length !== 1 || kids[0]!.type !== "element") {
    return false;
  }
  const only = kids[0]!;
  if (only.tagName === "img") {
    return true;
  }
  const inner = solid(only);
  return (
    only.tagName === "a" &&
    inner.length === 1 &&
    inner[0]!.type === "element" &&
    inner[0]!.tagName === "img"
  );
}

/**
 * A node between two blocks that draws nothing: a line break or a comment.
 * `dropHtmlComments` has taken the comments out before this step; a raw
 * comment is still read as nothing, so the pairing never depends on it.
 */
function between(node: Node): boolean {
  return (
    (node.type === "text" && textOf(node).trim() === "") ||
    node.type === "comment" ||
    (node.type === "raw" && (node.value ?? "").trim().startsWith("<!--"))
  );
}

/** Whether a list item may be kept whole on paper: no list in it, and short. */
function shortItem(item: Node): boolean {
  const nested = (item.children ?? []).some(
    (kid) =>
      kid.type === "element" &&
      (LIST.test(kid.tagName ?? "") ||
        elements(kid, "ul").length + elements(kid, "ol").length > 0),
  );
  return !nested && itemLines(item) <= SHORT_ITEM_LINES;
}

/** Whether a block never breaks on paper (`index.css`), whatever its height. */
function unbreakable(node: Node): boolean {
  return (
    /^(pre|blockquote|aside|details|figure)$/.test(node.tagName ?? "") ||
    (node.tagName === "table" &&
      elements(node, "tr").length <= SHORT_TABLE_ROWS)
  );
}

/**
 * Lines of a block that must print on its heading's page: the first lines
 * of a paragraph, the first item of a list, the head and first row of a
 * table, and for a heading its own lines and the first lines under it.
 */
function leadLines(node: Node): number {
  const tag = node.tagName ?? "";
  if (HEADING.test(tag)) {
    return HEADING_LINES + ORPHAN_LINES;
  }
  if (LIST.test(tag)) {
    const first = (node.children ?? []).find(
      (kid) => kid.type === "element" && kid.tagName === "li",
    );
    return first !== undefined && shortItem(first)
      ? itemLines(first)
      : ORPHAN_LINES;
  }
  if (tag === "table") {
    const rows = elements(node, "tr").slice(0, 2);
    return blockLines({ type: "element", tagName: "table", children: rows });
  }
  return ORPHAN_LINES;
}

/** How a heading is kept with the block after it. */
type Keep =
  { kind: "unit" } | { kind: "pair" } | { kind: "lead"; lines: number };

function keepOf(next: Node): Keep {
  if (isDiagram(next) || isPicture(next)) {
    return { kind: "unit" };
  }
  const lines = HEADING.test(next.tagName ?? "") ? null : linesOf(next);
  if (unbreakable(next) || (lines !== null && lines <= HALF_PAGE_LINES)) {
    return { kind: "pair" };
  }
  return { kind: "lead", lines: Math.min(leadLines(next), MAX_LEAD_LINES) };
}

/** `kids` with each heading wrapped with what follows it (see the top). */
function grouped(kids: Node[]): Node[] {
  const out: Node[] = [];
  for (let i = 0; i < kids.length; i += 1) {
    const kid = kids[i]!;
    if (kid.type !== "element" || !HEADING.test(kid.tagName ?? "")) {
      out.push(kid);
      continue;
    }
    // The next element, past the line breaks between blocks and a hidden
    // comment (a Reflection section's id line).
    let j = i + 1;
    while (j < kids.length && between(kids[j]!)) {
      j += 1;
    }
    const next = kids[j];
    if (next === undefined || next.type !== "element") {
      out.push(kid);
      continue;
    }
    const keep = keepOf(next);
    if (keep.kind === "lead") {
      out.push({
        type: "element",
        tagName: "div",
        properties: {
          [KEEP_ATTR]: KEEP_LEAD,
          style: `${LEAD_VAR}: ${String(keep.lines)}`,
        },
        children: [kid],
      });
      continue;
    }
    out.push({
      type: "element",
      tagName: "div",
      properties: { [KEEP_ATTR]: keep.kind === "unit" ? KEEP_UNIT : "" },
      children: kids.slice(i, j + 1),
    });
    i = j;
  }
  return out;
}

export function keepWithNext() {
  return (tree: unknown) => {
    const root = tree as Node;
    // A short table prints on one page; a long one breaks between rows.
    for (const table of elements(root, "table")) {
      if (elements(table, "tr").length <= SHORT_TABLE_ROWS) {
        table.properties = { ...table.properties, dataShort: "" };
      }
    }
    // A short item prints whole; a long one, or one holding a list, breaks
    // between its lines like any paragraph.
    for (const item of elements(root, "li")) {
      if (shortItem(item)) {
        item.properties = { ...item.properties, dataShort: "" };
      }
    }
    root.children = grouped(root.children ?? []);
    // The notes at the end are a section with a heading of their own, which
    // is kept with the first notes the same way.
    for (const kid of root.children) {
      if (kid.tagName === "section" && kid.properties?.["dataFootnotes"]) {
        kid.children = grouped(kid.children ?? []);
      }
    }
  };
}
