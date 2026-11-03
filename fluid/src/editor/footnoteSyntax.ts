/**
 * Where the footnotes of a document are, read by the parser the reading page
 * uses: remark-parse with the GFM, status label and math syntax extensions
 * `MarkdownBody` installs (its other plugins only transform the tree, which a
 * parse never runs). A footnote reference, a definition, what is code and what
 * is a URL are the parser's own answers, and the numbers follow the rule of
 * mdast-util-to-hast, which draws the page's notes: references in the body in
 * order, then the references inside the notes in the order the notes are
 * numbered; a label is matched without regard to case; of two definitions of a
 * label the first is the note; a definition nobody refers to is not drawn.
 */

import type { Text } from "@codemirror/state";
import remarkParse from "remark-parse";
import { unified } from "unified";

import { markdownSyntax } from "../markdown/syntax";
import { frontmatterRegion } from "./frontmatterRegion";

/** A drawn reference. */
export interface Ref {
  from: number;
  to: number;
  key: string;
}

/** A drawn note: the span of `[^label]:` and its space is [from, textFrom). */
export interface Note {
  key: string;
  from: number;
  textFrom: number;
}

export interface Notes {
  refs: Ref[];
  notes: Note[];
  numbers: Map<string, number>;
}

interface Position {
  start: { offset?: number | undefined };
  end: { offset?: number | undefined };
}

interface MdNode {
  type: string;
  identifier?: string;
  label?: string | null;
  position?: Position;
  children?: MdNode[];
}

const processor = unified().use(remarkParse).use(markdownSyntax);

/** `text` with its frontmatter block blanked, so offsets stay the document's. */
function body(text: string, end: number): string {
  if (end < 0) {
    return text;
  }
  return text.slice(0, end).replace(/[^\n]/g, " ") + text.slice(end);
}

/** Every footnote reference and definition of `text`, numbered. */
export function readNotes(doc: Text): Notes {
  const text = doc.toString();
  const region = frontmatterRegion(doc);
  const tree = processor.parse(
    body(text, region ? Math.min(region.to, text.length) : -1),
  ) as unknown as MdNode;

  const refs: { node: MdNode; from: number; to: number }[] = [];
  const defs: { node: MdNode; from: number; to: number }[] = [];
  const walk = (node: MdNode, inNote: MdNode | null) => {
    const from = node.position?.start.offset;
    const to = node.position?.end.offset;
    if (from === undefined || to === undefined) {
      return;
    }
    if (node.type === "footnoteDefinition") {
      defs.push({ node, from, to });
      inNote = node;
    } else if (node.type === "footnoteReference") {
      refs.push({ node, from, to });
      return;
    }
    for (const child of node.children ?? []) {
      walk(child, inNote);
    }
  };
  walk(tree, null);

  const noted = new Map<string, MdNode>();
  for (const def of defs) {
    const key = def.node.identifier ?? "";
    if (!noted.has(key)) {
      noted.set(key, def.node);
    }
  }

  const numbers = new Map<string, number>();
  const queue: string[] = [];
  const counted: typeof refs = [];
  const visit = (node: MdNode) => {
    const parts: { node: MdNode }[] = [];
    const collect = (child: MdNode) => {
      if (child.type === "footnoteReference") {
        parts.push({ node: child });
        return;
      }
      if (child.type === "footnoteDefinition") {
        return;
      }
      child.children?.forEach(collect);
    };
    collect(node);
    for (const { node: ref } of parts) {
      const key = ref.identifier ?? "";
      const found = refs.find((entry) => entry.node === ref);
      if (found) {
        counted.push(found);
      }
      if (!numbers.has(key)) {
        numbers.set(key, numbers.size + 1);
        queue.push(key);
      }
    }
  };
  visit(tree);
  for (let index = 0; index < queue.length; index += 1) {
    const def = noted.get(queue[index] ?? "");
    if (def) {
      for (const child of def.children ?? []) {
        visit(child);
      }
    }
  }

  const drawnNotes: Note[] = [];
  for (const [key, node] of noted) {
    const from = node.position?.start.offset;
    if (from === undefined || !numbers.has(key)) {
      continue;
    }
    // `[^label]:` and the one space after it.
    const colon = from + 4 + (node.label ?? key).length;
    drawnNotes.push({
      key,
      from,
      textFrom: text[colon] === " " ? colon + 1 : colon,
    });
  }
  return {
    refs: counted
      .map(({ node, from, to }) => ({
        from,
        to,
        key: node.identifier ?? "",
      }))
      .sort((a, b) => a.from - b.from),
    notes: drawnNotes.sort((a, b) => a.from - b.from),
    numbers,
  };
}
