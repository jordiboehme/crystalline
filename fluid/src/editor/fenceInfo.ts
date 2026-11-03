/**
 * A fence's info word, read the one way every editor layer reads it and the
 * way CommonMark (and so the reading page) reads it: after any spaces that
 * follow the opening marks, up to the next space or the line end. The code
 * box, the mermaid preview and the math fence all ask here, so a fence the
 * page draws as a diagram or a formula is one in the editor too.
 */

import type { Text } from "@codemirror/state";
import type { SyntaxNode } from "@lezer/common";

export interface FenceInfo {
  /** The opening fence's backticks or tildes. */
  marks: { from: number; to: number };
  /** The info word; empty (at the marks' end) when there is none. */
  word: { from: number; to: number };
  /** The word's text. */
  name: string;
  /** Whether more info text follows the word on the opening line. */
  more: boolean;
  /**
   * The document lines of the opening and closing marks, or a null close
   * when the fence is not closed.
   */
  open: number;
  close: number | null;
}

/** The info of the `FencedCode` node `node`, or null without opening marks. */
export function fenceInfo(doc: Text, node: SyntaxNode): FenceInfo | null {
  const first = node.firstChild;
  if (first?.name !== "CodeMark") {
    return null;
  }
  const openLine = doc.lineAt(first.from);
  const after = doc.sliceString(first.to, openLine.to);
  const lead = /^[ \t]*/.exec(after)?.[0].length ?? 0;
  const length = /^[^ \t]*/.exec(after.slice(lead))?.[0].length ?? 0;
  const word = { from: first.to + lead, to: first.to + lead + length };
  const last = node.lastChild;
  const close =
    last !== null && last !== first && last.name === "CodeMark"
      ? doc.lineAt(last.from).number
      : null;
  return {
    marks: { from: first.from, to: first.to },
    word,
    name: doc.sliceString(word.from, word.to),
    more: after.slice(lead + length).trim() !== "",
    open: openLine.number,
    close: close !== null && close > openLine.number ? close : null,
  };
}

/** The text between a closed fence's two fence lines, as written. */
export function fenceBody(doc: Text, info: FenceInfo): string {
  if (info.close === null || info.close === info.open + 1) {
    return "";
  }
  return doc.sliceString(
    doc.line(info.open + 1).from,
    doc.line(info.close - 1).to,
  );
}
