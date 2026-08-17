/**
 * Whether a position is prose a completion may fire in.
 *
 * The decoration layers each leave code contexts and the frontmatter block
 * alone (`wikilinkChips`, `crystallineLines`); a completion popup is the
 * same recognition affordance in another form, so the completion sources
 * ask this one question before matching anything. Inside a fence a `- item`
 * is YAML and a `# note` is a shell comment, and inside the frontmatter
 * block a bullet is a tags entry - none of them want a vocabulary popup.
 *
 * The code check walks the outer syntax tree at the position: a fenced
 * block with a language mounts a whole nested tree, and the block's own
 * node is what has to be seen whether or not that inner parse has landed.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState } from "@codemirror/state";

import { frontmatterRegion } from "./frontmatterRegion";

/** The subtrees a match is code inside rather than prose in. */
export const CODE_CONTEXTS = new Set([
  "InlineCode",
  "FencedCode",
  "CodeBlock",
  "CodeText",
]);

export function inCompletableProse(state: EditorState, pos: number): boolean {
  const frontmatter = frontmatterRegion(state.doc);
  if (frontmatter && pos <= frontmatter.to) {
    return false;
  }
  let inCode = false;
  syntaxTree(state).iterate({
    from: pos,
    to: pos,
    enter: (node) => {
      if (CODE_CONTEXTS.has(node.name)) {
        inCode = true;
        return false;
      }
    },
  });
  return !inCode;
}
