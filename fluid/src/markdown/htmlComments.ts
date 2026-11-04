/**
 * HTML comments are notes for whoever edits the file (`<!-- sources: ... -->`,
 * a Reflection section's `<!-- r:... -->` id line), never text for a reader.
 * Fluid draws raw HTML as the text it was written as (there is no
 * `rehype-raw`), so a comment would show on the page. These helpers take the
 * comments out of what is drawn; the source keeps them. The editor's live
 * preview hides them off the lines being edited, and shows them as written
 * on those lines (`editor/preview.ts`).
 *
 * Only the raw nodes the parser built are touched: a comment written inside a
 * code span or a fenced block is code text, not a raw node, and stays. Any
 * other raw HTML stays too, drawn as its text.
 */

/**
 * One whole comment. `<!-->` and `<!--->` are comments too (CommonMark reads
 * them so). A `<!--` that never closes is not matched and stays text.
 */
const COMMENT = /<!--(?:-?>|[\s\S]*?-->)/g;

/** Where each whole comment in `text` starts and ends. */
export function htmlCommentRanges(
  text: string,
): { from: number; to: number }[] {
  return Array.from(text.matchAll(COMMENT), (m) => ({
    from: m.index,
    to: m.index + m[0].length,
  }));
}

/**
 * `value` without its comments; what is left of the raw HTML stays as is.
 * The removal runs until nothing changes, because taking one comment out can
 * join the text around it into a new one: `<!-<!-- x -->- y -->` leaves
 * `<!-- y -->` after one pass.
 */
export function withoutHtmlComments(value: string): string {
  let previous: string;
  let current = value;
  do {
    previous = current;
    current = current.replace(COMMENT, "");
  } while (current !== previous);
  return current;
}

interface Node {
  type: string;
  value?: string;
  children?: Node[];
}

function drop(node: Node): void {
  if (!node.children) {
    return;
  }
  node.children = node.children.flatMap((child) => {
    if (child.type === "raw" && (child.value ?? "").includes("<!--")) {
      const rest = withoutHtmlComments(child.value ?? "");
      if (rest.trim() === "") {
        return [];
      }
      return [{ ...child, value: rest }];
    }
    drop(child);
    return [child];
  });
}

/**
 * The rehype step that drops every comment, at any depth (a list, a quote, a
 * table cell, a footnote). A raw node that held nothing but comments goes;
 * one that held more keeps the rest as its text.
 */
export function dropHtmlComments() {
  return (tree: unknown) => {
    drop(tree as Node);
  };
}

/**
 * A snippet's text without its comments. The window the engine cut may end
 * inside one, so an unclosed `<!--` at the very end goes too. A `-->` at the
 * start is left alone: it is as likely a diagram's arrow as a comment's end.
 */
export function stripHtmlComments(text: string): string {
  return withoutHtmlComments(text).replace(/<!--(?![\s\S]*-->)[\s\S]*$/, "");
}
