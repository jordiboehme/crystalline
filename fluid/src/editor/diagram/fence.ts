/**
 * The mermaid fences of a document, with the positions the diagram layer
 * edits through. Which fences count is not decided here: `mermaidFence`
 * (`elementVerbs.ts`, built on `fenceInfo`) is the one rule the preview, the
 * code box and the reading page share, so a closed fence with the info word
 * `mermaid` and a non-blank body, tildes and info text included. An unclosed
 * or empty fence is never one of these.
 *
 * Positions are document positions; the body is read with
 * `state.doc.sliceString`, so it is an LF string in a CRLF document too, and
 * `bodyFrom` plus a body offset is a document position (a line break is one
 * position whatever the separator).
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";
import type { SyntaxNode } from "@lezer/common";

import { mermaidFence } from "../elementVerbs";
import { fenceInfo } from "../fenceInfo";

export interface MermaidFence {
  /** The `FencedCode` node's range, opening and closing marks included. */
  from: number;
  to: number;
  /** The body between the two fence lines, as `body` was read. */
  bodyFrom: number;
  bodyTo: number;
  body: string;
  /** False inside a blockquote or a list item, whose body carries prefixes. */
  topLevel: boolean;
}

/** Every mermaid fence of the parsed tree, in document order. */
export function mermaidFences(state: EditorState): MermaidFence[] {
  const fences: MermaidFence[] = [];
  syntaxTree(state).iterate({
    enter: (node) => {
      if (node.name === "FencedCode") {
        const fence = fenceOf(state, node.node);
        if (fence !== null) {
          fences.push(fence);
        }
        return false;
      }
      return undefined;
    },
  });
  return fences;
}

/** The mermaid fence whose `from..to` holds `pos` (ends included), or null. */
export function fenceAt(state: EditorState, pos: number): MermaidFence | null {
  let found: MermaidFence | null = null;
  syntaxTree(state).iterate({
    from: pos,
    to: pos,
    enter: (node) => {
      if (found !== null) {
        return false;
      }
      if (node.name === "FencedCode") {
        const fence = fenceOf(state, node.node);
        if (fence !== null && fence.from <= pos && pos <= fence.to) {
          found = fence;
        }
        return false;
      }
      return undefined;
    },
  });
  return found;
}

function fenceOf(state: EditorState, node: SyntaxNode): MermaidFence | null {
  if (mermaidFence(state, node) === null) {
    return null;
  }
  const info = fenceInfo(state.doc, node);
  if (info?.close == null) {
    return null;
  }
  const doc = state.doc;
  const bodyFrom = doc.line(info.open + 1).from;
  const bodyTo = doc.line(info.close - 1).to;
  return {
    from: node.from,
    to: node.to,
    bodyFrom,
    bodyTo,
    body: doc.sliceString(bodyFrom, bodyTo),
    topLevel: !inContainer(node),
  };
}

function inContainer(node: SyntaxNode): boolean {
  for (let up = node.parent; up !== null; up = up.parent) {
    if (up.name === "Blockquote" || up.name === "ListItem") {
      return true;
    }
  }
  return false;
}
