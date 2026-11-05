/**
 * The fence-local line scanner every diagram parser starts from: it tells
 * the lines mermaid removes before parsing (frontmatter, directives,
 * comments) from the header, the accessibility lines and the statements, and
 * splits a statement line into its `;`-separated statements.
 *
 * The rules mirror mermaid 12.2.0's own preprocessing, read from its source
 * (research/2026-11-05-mermaid-12.2-flowchart-ids.md):
 *
 * - Frontmatter (`matchFrontMatter`): only from the body's first line, `---`
 *   with an optional indent and trailing whitespace, closed by the same
 *   indent and `---` on a later line that is not the body's last.
 * - Directives (`directiveRegex`): `%%{` up to the next `}%%`, across lines.
 * - Comments (`cleanupComments`, `/^\s*%%(?!{)[^\n]+/`): only a whole line
 *   that is `%%` followed by at least one character. A bare `%%` line is a
 *   node to mermaid and a trailing `%%` after a statement is a parse error;
 *   both stay statements here and `hasStrayComment` finds them.
 *   `cleanupComments` is a text rule that runs before the lexer, so a `%%`
 *   line inside an `accDescr { }` block is a comment too.
 * - `encodeEntities`: on a line holding `style` (or `classDef`) followed by
 *   `:`, `#` and a `;`, the line's last `;` is removed before the parse, and
 *   an entity `#name;` is hidden from the lexer; neither splits here.
 * - The flowchart lexer's `.*direction\s+(TB|BT|RL|LR|TD)[^\n]*` takes a
 *   whole line holding such text as one direction token, so in a flowchart
 *   that line is one statement (`isDirectionLine`).
 *
 * The scanner knows no diagram grammar beyond these: `understood` is false
 * only for what leaves the line kinds themselves unknown (an unclosed
 * frontmatter, directive or `accDescr` block, no header). What each type's
 * parser refuses is that parser's verdict.
 */

import type { Span } from "./edits";

export type LineKind =
  | "blank"
  | "comment"
  | "directive"
  | "frontmatter"
  | "acc"
  | "header"
  | "statement";

export interface ScannedLine {
  /** The line's index in the body, from 0. */
  index: number;
  /** Body offsets; `to` excludes the line break. */
  from: number;
  to: number;
  kind: LineKind;
  /** The line's leading spaces and tabs, as written. */
  indent: string;
  /**
   * For a header or statement line, its statements: split on each top-level
   * `;` (outside quotes, brackets, `|...|` and backticks), each span trimmed
   * of outer whitespace and without its `;`, empty ones left out. Empty for
   * every other kind.
   */
  statements: Span[];
}

export interface Scan {
  lines: ScannedLine[];
  /** The first line that is none of the removed or blank kinds. */
  header: ScannedLine | null;
  /** The header's first word (`flowchart`, `graph`, `sequenceDiagram`). */
  keyword: string | null;
  understood: boolean;
  reasons: string[];
}

/** The keywords whose lexer has the whole-line direction rule. */
const FLOWCHART_KEYWORDS = new Set(["flowchart", "graph", "flowchart-elk"]);

const COMMENT = /^\s*%%(?!\{)./;
const DIRECTION = /direction\s+(?:TB|BT|RL|LR|TD)/;

export function scanFence(body: string): Scan {
  const lines: ScannedLine[] = [];
  let from = 0;
  for (const [index, text] of body.split("\n").entries()) {
    lines.push({
      index,
      from,
      to: from + text.length,
      kind: "statement",
      indent: /^[ \t]*/.exec(text)?.[0] ?? "",
      statements: [],
    });
    from += text.length + 1;
  }
  const textOf = (line: ScannedLine) => body.slice(line.from, line.to);
  const reasons: string[] = [];

  let start = 0;
  const first = lines[0];
  const opening = first ? /^([ \t]*)---\s*$/.exec(textOf(first)) : null;
  if (first && opening) {
    const closing = `${opening[1] ?? ""}---`;
    const close = lines.findIndex(
      (line, i) =>
        i > 0 &&
        i < lines.length - 1 &&
        textOf(line).startsWith(closing) &&
        textOf(line).slice(closing.length).trim() === "",
    );
    if (close === -1) {
      first.kind = "frontmatter";
      start = 1;
      reasons.push("The frontmatter block is not closed.");
    } else {
      for (const line of lines.slice(0, close + 1)) {
        line.kind = "frontmatter";
      }
      start = close + 1;
    }
  }

  let header: ScannedLine | null = null;
  let keyword: string | null = null;
  let flowchart = false;
  let open: "directive" | "acc" | null = null;
  for (const line of lines.slice(start)) {
    const text = textOf(line);
    const rest = text.slice(line.indent.length);
    if (open === "directive") {
      line.kind = "directive";
      const end = text.indexOf("}%%");
      if (end !== -1) {
        open = null;
        afterDirective(text, end, reasons);
      }
      continue;
    }
    if (COMMENT.test(text)) {
      line.kind = "comment";
      continue;
    }
    if (open === "acc") {
      line.kind = "acc";
      if (text.includes("}")) {
        open = null;
      }
      continue;
    }
    if (rest.startsWith("%%{")) {
      line.kind = "directive";
      const end = text.indexOf("}%%", line.indent.length + 3);
      if (end === -1) {
        open = "directive";
      } else {
        afterDirective(text, end, reasons);
      }
      continue;
    }
    if (rest.trim() === "") {
      line.kind = "blank";
      continue;
    }
    if (header === null) {
      line.kind = "header";
      header = line;
      keyword = /^[^\s;]+/.exec(rest)?.[0] ?? null;
      flowchart = keyword !== null && FLOWCHART_KEYWORDS.has(keyword);
    } else if (/^acc(?:Title|Descr)\s*:/.test(rest)) {
      line.kind = "acc";
      continue;
    } else if (/^accDescr\s*\{/.test(rest)) {
      line.kind = "acc";
      if (!rest.slice(rest.indexOf("{") + 1).includes("}")) {
        open = "acc";
      }
      continue;
    } else {
      line.kind = "statement";
    }
    line.statements =
      flowchart && DIRECTION.test(text)
        ? spans(line.from, text, [{ from: 0, to: text.length }])
        : splitStatements(line.from, text);
    if (flowchart && /direction\s*$/.test(text)) {
      const next = lines[line.index + 1];
      if (next && /^\s*(?:TB|BT|RL|LR|TD)/.test(textOf(next))) {
        reasons.push(
          `Line ${line.index + 1} starts a direction that runs onto the next line.`,
        );
      }
    }
  }
  if (open === "directive") {
    reasons.push("A directive is not closed.");
  } else if (open === "acc") {
    reasons.push("An accDescr block is not closed.");
  }
  if (header === null) {
    reasons.push("The diagram has no keyword line.");
  }
  return {
    lines,
    header,
    keyword,
    understood: reasons.length === 0,
    reasons,
  };
}

/** Text after a directive's `}%%` stays on the line mermaid parses. */
function afterDirective(text: string, end: number, reasons: string[]): void {
  if (text.slice(end + 3).trim() !== "") {
    reasons.push("Text follows a directive on its line.");
  }
}

/**
 * Whether `line` is one mermaid's flowchart lexer reads as a direction
 * statement: it holds `direction` and one of the five directions anywhere,
 * even inside a label, and the line's other content vanishes. Only header
 * and statement lines are lexed; a removed line never is.
 */
export function isDirectionLine(body: string, line: ScannedLine): boolean {
  return (
    (line.kind === "statement" || line.kind === "header") &&
    DIRECTION.test(body.slice(line.from, line.to))
  );
}

/**
 * Whether a header or statement line holds a `%%` that is not a comment to
 * mermaid: a bare `%%` line (mermaid reads it as a node), or a `%%` after or
 * between statements, outside quotes, brackets, `|...|` and backticks (a
 * parse error, or a directive removed from the middle of a line). A `%%`
 * inside a label is label text.
 */
export function hasStrayComment(body: string, line: ScannedLine): boolean {
  if (line.kind !== "statement" && line.kind !== "header") {
    return false;
  }
  const text = body.slice(line.from, line.to);
  let stray = false;
  walkTopLevel(text, (i) => {
    if (text.startsWith("%%", i)) {
      stray = true;
    }
  });
  return stray;
}

/** `text`'s statements as body spans, `offset` being the line's start. */
function splitStatements(offset: number, text: string): Span[] {
  const dropped = droppedSemicolons(text);
  const pieces: Span[] = [];
  let at = 0;
  walkTopLevel(
    text,
    (i) => {
      if (text[i] !== ";") {
        return;
      }
      // A removed `;` with only whitespace (or removed `;`s) after it ends
      // the line's statements either way; one with more after it joins them.
      if (
        dropped.has(i) &&
        [...text.slice(i + 1)].some(
          (ch, k) => !dropped.has(i + 1 + k) && /\S/.test(ch),
        )
      ) {
        return;
      }
      pieces.push({ from: at, to: i });
      at = i + 1;
    },
    dropped,
  );
  pieces.push({ from: at, to: text.length });
  return spans(offset, text, pieces);
}

/** Line-relative `pieces` trimmed of outer whitespace, as body spans. */
function spans(offset: number, text: string, pieces: Span[]): Span[] {
  const out: Span[] = [];
  for (const piece of pieces) {
    let { from, to } = piece;
    while (from < to && /\s/.test(text.charAt(from))) {
      from++;
    }
    while (to > from && /\s/.test(text.charAt(to - 1))) {
      to--;
    }
    if (to > from) {
      out.push({ from: offset + from, to: offset + to });
    }
  }
  return out;
}

/**
 * The `;`s `encodeEntities` removes from a line before the parse: the last
 * `;` of a line matching `style.*:\S*#.*;`, then the last of the result if
 * it matches `classDef.*:\S*#.*;`.
 */
function droppedSemicolons(text: string): Set<number> {
  const all = new Set<number>();
  const style = /style.*:\S*#.*;/.exec(text);
  let styleAt = -1;
  let rest = text;
  if (style) {
    styleAt = style.index + style[0].length - 1;
    all.add(styleAt);
    rest = text.slice(0, styleAt) + text.slice(styleAt + 1);
  }
  const classDef = /classDef.*:\S*#.*;/.exec(rest);
  if (classDef) {
    const at = classDef.index + classDef[0].length - 1;
    all.add(styleAt !== -1 && at >= styleAt ? at + 1 : at);
  }
  return all;
}

/**
 * Calls `visit` with each index of `text` that is outside quotes, brackets,
 * a `|...|` label and backticks. An entity `#name;` is skipped whole (its
 * `;` is hidden from the lexer), unless its `;` is one `dropped` removes.
 */
function walkTopLevel(
  text: string,
  visit: (i: number) => void,
  dropped: ReadonlySet<number> = new Set(),
): void {
  let quote = false;
  let backtick = false;
  let pipe = false;
  let depth = 0;
  const entity = /#\w+;/y;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quote) {
      quote = c !== '"';
      continue;
    }
    if (backtick) {
      backtick = c !== "`";
      continue;
    }
    if (c === '"') {
      quote = true;
    } else if (c === "`") {
      backtick = true;
    } else if (c === "[" || c === "(" || c === "{") {
      depth++;
    } else if (c === "]" || c === ")" || c === "}") {
      depth = Math.max(0, depth - 1);
    } else if (depth === 0 && c === "|") {
      pipe = !pipe;
    } else if (depth === 0 && !pipe) {
      if (c === "#") {
        entity.lastIndex = i;
        const match = entity.exec(text);
        if (match && !dropped.has(i + match[0].length - 1)) {
          i += match[0].length - 1;
          continue;
        }
      }
      visit(i);
    }
  }
}
