/**
 * A code block's face, in preview mode.
 *
 * The scroller is proportional now (see `editorTheme`), and prose is what that
 * is for; a code block is not prose. The obvious mechanism for keeping it mono
 * is the `tags.monospace` highlight rule, and it is not enough: `@lezer/markdown`
 * maps `CodeText` to that tag, but the moment a fence names a language
 * (```json, ```rust, ```ts) the nested language parser mounts over the body and
 * `@lezer/highlight` recurses into the mount with `inheritedClass` reset to "",
 * so the mono class never reaches the tokens the inner parser produced. The
 * body of every language-tagged fence would read in the reading font - the
 * common case, not the edge.
 *
 * So the face is decided by structure rather than by token: one line
 * decoration per line of every `FencedCode` and `CodeBlock` node, which is
 * true whatever parser owns the inside of it. It covers the fence delimiters
 * and the info string as well as the body, so a fence shows ONE face rather
 * than mono text between sans backticks.
 *
 * A `StateField` rather than a `ViewPlugin`, matching `fencePreviews`: the
 * decorations come out of the syntax tree, which the state already has, and
 * nothing here needs a view to compute them.
 *
 * This belongs to the preview layer and is installed only from
 * `previewConfig`'s preview branch. Raw mode and the MANIFEST editor are
 * already fully mono through `RAW_MONO`, so there is nothing for it to do
 * there.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension, Range } from "@codemirror/state";
import { StateField } from "@codemirror/state";
import type { DecorationSet } from "@codemirror/view";
import { Decoration, EditorView } from "@codemirror/view";

import { parseAdvanced } from "./parseProgress";

/** The class one line of code wears. */
const FENCE_LINE = Decoration.line({ class: "cm-fence-mono" });

/** The block nodes whose lines are code however they are written. */
const CODE_NODES = new Set(["FencedCode", "CodeBlock"]);

function buildFenceLines(state: EditorState): DecorationSet {
  const doc = state.doc;
  // Line numbers rather than positions, deduplicated: a decoration set may
  // not carry the same line twice, and the tree can hand back nested or
  // adjacent code nodes.
  const lines = new Set<number>();
  syntaxTree(state).iterate({
    enter: (node) => {
      if (!CODE_NODES.has(node.name)) {
        return;
      }
      // `node.to` sits either on the last character of the closing fence or,
      // for a block that ends in a line break, on the start of the line
      // after it. Stepping back one position lands inside the block's own
      // last line in both cases.
      const endPos = node.to > node.from ? node.to - 1 : node.to;
      const first = doc.lineAt(node.from).number;
      const last = doc.lineAt(endPos).number;
      for (let number = first; number <= last; number += 1) {
        lines.add(number);
      }
    },
  });
  const ranges: Range<Decoration>[] = Array.from(lines)
    .sort((a, b) => a - b)
    .map((number) => FENCE_LINE.range(doc.line(number).from));
  return Decoration.set(ranges);
}

/**
 * The mono rule, written against BOTH classes on purpose.
 *
 * `.cm-line` already carries a rule from `editorTheme` (its padding), and two
 * rules of equal specificity on one element are decided by the order they
 * were mounted in - the trap `RAW_MONO` documents. `.cm-line.cm-fence-mono`
 * is one class more specific than any plain `.cm-line` rule, so this wins on
 * specificity and stays correct however the modules end up ordered.
 */
const fenceMonoTheme = EditorView.baseTheme({
  ".cm-line.cm-fence-mono": {
    fontFamily: "var(--font-mono, ui-monospace, monospace)",
  },
});

/** Mono for every line of every code block. */
export function fenceMono(): Extension {
  const field = StateField.define<DecorationSet>({
    create: (state) => buildFenceLines(state),
    update: (value, tr) =>
      tr.docChanged || parseAdvanced(tr) ? buildFenceLines(tr.state) : value,
    provide: (f) => EditorView.decorations.from(f),
  });
  return [field, fenceMonoTheme];
}
