/**
 * Where the frontmatter block sits, read off the document's own lines.
 *
 * The markdown parser has no frontmatter notion and would read the fences as
 * thematic breaks, so every editor layer that must leave the block alone asks
 * this one question instead. Same shape as the display regex in MarkdownBody
 * (`FRONTMATTER`), deliberately: the editor and the reader have to agree on
 * where the metadata ends, or a heading would render in one and not the other.
 *
 * An opening `---` on the very first line, closed by the next `---` line. A
 * trailing carriage return is tolerated on either fence because a document
 * whose separator is "\n" may still carry a stray "\r" inside a line, and the
 * closing fence tolerates trailing blanks the way the display regex does.
 */

import type { Text } from "@codemirror/state";

const OPEN_FENCE = /^---\r?$/;
const CLOSE_FENCE = /^---[ \t]*\r?$/;

export function frontmatterRegion(
  doc: Text,
): { from: number; to: number } | null {
  if (doc.lines < 2 || !OPEN_FENCE.test(doc.line(1).text)) {
    return null;
  }
  for (let number = 2; number <= doc.lines; number += 1) {
    const line = doc.line(number);
    if (CLOSE_FENCE.test(line.text)) {
      // Delimiter lines included, the closing break excluded: the range is
      // the block, not the blank line somebody put after it.
      return { from: 0, to: line.to };
    }
  }
  return null;
}
