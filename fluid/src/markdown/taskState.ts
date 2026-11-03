/**
 * Task list items, read for the reading page: where each box is in the
 * source, and the third state `[~]`, not applicable.
 *
 * GitHub's task list syntax knows two states, and remark-gfm already reads
 * them (`checked` on the list item). This pass adds two things on top.
 *
 * - Every box remark-gfm found gets `data-task-at`: the offset of its `[`
 *   in the text the parser was handed, found on the item's own first line
 *   and only where the three characters there are `[ ]`, `[x]` or `[X]`.
 *   Searching rather than adding a bullet width keeps `10.`, `-   `, a
 *   nested item and an item inside a quote right. An item whose box is not
 *   where it should be gets no offset and its box does not tick.
 * - An item whose first words are `[~]` and a space or tab is marked
 *   `data-task="na"` and loses the marker. The text node that held it starts
 *   where its remaining words start, so source spans on a Reflection page
 *   still read the source at their offsets. Every other node keeps its
 *   place and its position.
 *
 * The core reads the same four markers as a task, never as an observation
 * category. No React here: what a box looks like is `TaskBox.tsx`.
 */

/** The small part of the mdast shape this plugin reads and writes. */
interface Point {
  line: number;
  column: number;
  offset?: number;
}

interface MdNode {
  type: string;
  value?: string;
  checked?: boolean | null;
  children?: MdNode[];
  position?: { start: Point; end: Point };
  data?: Record<string, unknown>;
}

/** The root of an mdast tree, as far as this plugin needs it. */
export interface Root extends MdNode {
  type: "root";
}

/** The three boxes that tick. */
const TICKABLE = /^\[[ xX]\]$/;

/** `[~]` and the whitespace after it, at the start of an item's words. */
export const NOT_APPLICABLE = /^\[~\][ \t]+/;

/** The offset of the item's box in `source`, or null when it is not on the item's first line. */
function boxOffset(item: MdNode, source: string): number | null {
  const start = item.position?.start.offset;
  if (start === undefined) {
    return null;
  }
  const at = source.indexOf("[", start);
  const lineEnd = source.indexOf("\n", start);
  if (at < 0 || (lineEnd >= 0 && at > lineEnd)) {
    return null;
  }
  return TICKABLE.test(source.slice(at, at + 3)) ? at : null;
}

/** Mark the item not applicable when its first words are `[~] `. */
function markNotApplicable(item: MdNode): void {
  const paragraph = item.children?.[0];
  const text = paragraph?.type === "paragraph" ? paragraph.children?.[0] : null;
  if (text?.type !== "text" || typeof text.value !== "string") {
    return;
  }
  const marker = NOT_APPLICABLE.exec(text.value);
  if (marker === null) {
    return;
  }
  const cut = marker[0].length;
  text.value = text.value.slice(cut);
  const start = text.position?.start;
  if (start !== undefined) {
    text.position = {
      start: {
        line: start.line,
        column: start.column + cut,
        ...(start.offset === undefined ? {} : { offset: start.offset + cut }),
      },
      end: text.position!.end,
    };
  }
  if (text.value === "" && paragraph?.children) {
    paragraph.children = paragraph.children.slice(1);
  }
  setProperty(item, "data-task", "na");
}

function setProperty(node: MdNode, key: string, value: string): void {
  const data = node.data ?? {};
  const properties =
    (data.hProperties as Record<string, unknown> | undefined) ?? {};
  node.data = { ...data, hProperties: { ...properties, [key]: value } };
}

function walk(node: MdNode, source: string): void {
  if (node.type === "listItem") {
    if (typeof node.checked === "boolean") {
      const at = boxOffset(node, source);
      if (at !== null) {
        setProperty(node, "data-task-at", String(at));
      }
    } else {
      markNotApplicable(node);
    }
  }
  for (const child of node.children ?? []) {
    walk(child, source);
  }
}

/** The remark plugin: every task item in the tree, nested ones included. */
export function remarkTaskState(): (
  tree: Root,
  file?: { value?: unknown },
) => void {
  // The source is the file's text; without it no box gets an offset, and so
  // no box ticks.
  return (tree, file) => {
    walk(tree, typeof file?.value === "string" ? file.value : "");
  };
}
