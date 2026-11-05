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
 *   indent and `---` on line 2 or later, not the body's last.
 * - Directives (`directiveRegex`, used as mermaid writes it): a line that
 *   starts with `%%{` and a word after the brace (the word may sit on a
 *   later line), up to the next `}%%`, across lines. A `%%{` without a word
 *   (`%%{ }%%`) is neither removed nor a comment, so it is lexed as written:
 *   a header or statement line that `hasStrayComment` flags.
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
 *   that line is one statement (`isDirectionLine`). Its `\s+` crosses line
 *   breaks, and the blank, comment and directive lines between are already
 *   gone, so a line ending in `direction` whose next lexed line starts with
 *   a direction is not understood.
 * - The `accDescr {` block ends at its first `}`; the lexer reads the rest
 *   of that line as statements, so that `acc` line carries them.
 *
 * The scanner knows no diagram grammar beyond these: `understood` is false
 * only for what leaves the line kinds themselves unknown (an unclosed
 * frontmatter, directive or `accDescr` block, text after a directive, a
 * direction run onto a later line, no header). What each type's
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
   * of outer whitespace and without its `;`, empty ones left out. An `acc`
   * line that closes an `accDescr { }` block holds the statements after its
   * `}` (mermaid lexes them). Empty for every other line.
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
/** mermaid 12.2.0's `directiveRegex` (regexes.ts), flags set where used. */
const DIRECTIVE =
  /%{2}{\s*(?:(\w+)\s*:|(\w+))\s*(?:(\w+)|((?:(?!}%{2}).|\r?\n)*))?\s*(?:}%{2})?/;

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
        // `matchFrontMatter` looks for the close only after the opening
        // line's break and the next one, so line 2 is the earliest.
        i > 1 &&
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
  /** The body offset a directive runs to, while its lines are scanned. */
  let directiveEnd = -1;
  let accOpen = false;
  const statementsOf = (offset: number, text: string): Span[] =>
    flowchart && DIRECTION.test(text)
      ? spans(offset, text, [{ from: 0, to: text.length }])
      : splitStatements(offset, text);
  for (const line of lines.slice(start)) {
    const text = textOf(line);
    const rest = text.slice(line.indent.length);
    if (line.from < directiveEnd) {
      line.kind = "directive";
      if (directiveEnd <= line.to) {
        afterDirective(body, directiveEnd, line, reasons);
      }
      continue;
    }
    // Directives are removed before comments, and both before the lexer
    // reads an `accDescr` block, so they are tested first.
    if (rest.startsWith("%%{")) {
      const directive = new RegExp(DIRECTIVE.source, "iy");
      directive.lastIndex = line.from + line.indent.length;
      const match = directive.exec(body);
      if (match !== null) {
        line.kind = "directive";
        directiveEnd = directive.lastIndex;
        if (!match[0].endsWith("}%%") && directiveEnd === body.length) {
          reasons.push("A directive is not closed.");
        } else if (directiveEnd <= line.to) {
          afterDirective(body, directiveEnd, line, reasons);
        }
        continue;
      }
      // No word after the brace: mermaid removes nothing and the comment
      // rule skips `%%{`, so the line is lexed as written.
    }
    if (COMMENT.test(text)) {
      line.kind = "comment";
      continue;
    }
    if (accOpen) {
      line.kind = "acc";
      const close = text.indexOf("}");
      if (close !== -1) {
        accOpen = false;
        // The lexer leaves the block at its `}` and lexes the rest.
        line.statements = statementsOf(
          line.from + close + 1,
          text.slice(close + 1),
        );
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
      const open = text.indexOf("{");
      const close = text.indexOf("}", open + 1);
      if (close === -1) {
        accOpen = true;
      } else {
        line.statements = statementsOf(
          line.from + close + 1,
          text.slice(close + 1),
        );
      }
      continue;
    } else {
      line.kind = "statement";
    }
    line.statements = statementsOf(line.from, text);
  }
  if (flowchart) {
    runOnDirections(body, lines, reasons);
  }
  if (accOpen) {
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

/** Text after a directive on its last line stays for mermaid to parse. */
function afterDirective(
  body: string,
  end: number,
  line: ScannedLine,
  reasons: string[],
): void {
  if (body.slice(end, line.to).trim() !== "") {
    reasons.push("Text follows a directive on its line.");
  }
}

/**
 * The lexer's `direction\s+(TB|BT|RL|LR|TD)` crosses line breaks, and the
 * blank, comment and directive lines between are gone by the time it runs.
 * So a statement that ends in `direction` and whose next lexed line starts
 * with a direction swallows every line up to it: not understood.
 */
function runOnDirections(
  body: string,
  lines: ScannedLine[],
  reasons: string[],
): void {
  for (const line of lines) {
    if (line.statements.length === 0) {
      continue;
    }
    if (!/direction\s*$/.test(body.slice(line.from, line.to))) {
      continue;
    }
    const next = lines
      .slice(line.index + 1)
      .find(
        (l) =>
          l.kind !== "blank" && l.kind !== "comment" && l.kind !== "directive",
      );
    if (next && /^\s*(?:TB|BT|RL|LR|TD)/.test(body.slice(next.from, next.to))) {
      reasons.push(
        `Line ${line.index + 1} starts a direction that runs onto line ${next.index + 1}.`,
      );
    }
  }
}

/**
 * Whether `line` is one mermaid's flowchart lexer reads as a direction
 * statement: it holds `direction` and one of the five directions anywhere,
 * even inside a label, and the line's other content vanishes. Only header
 * and statement lines are lexed; a removed line never is.
 */
export function isDirectionLine(body: string, line: ScannedLine): boolean {
  const text = lexedText(body, line);
  return text !== null && DIRECTION.test(text);
}

/**
 * The part of `line` mermaid's lexer reads: a header or statement line
 * whole, the text after the `}` of an `acc` line that closes a block with
 * statements after it, or null for a line the lexer never sees.
 */
function lexedText(body: string, line: ScannedLine): string | null {
  if (line.kind === "statement" || line.kind === "header") {
    return body.slice(line.from, line.to);
  }
  const first = line.statements[0];
  return line.kind === "acc" && first !== undefined
    ? body.slice(first.from, line.to)
    : null;
}

/**
 * Whether the lexed part of a line (`lexedText`) holds a `%%` that is not a comment to
 * mermaid: a bare `%%` line (mermaid reads it as a node), or a `%%` after or
 * between statements, outside quotes, brackets, `|...|` and backticks (a
 * parse error, or a directive removed from the middle of a line). A `%%`
 * inside a label is label text.
 */
export function hasStrayComment(body: string, line: ScannedLine): boolean {
  const text = lexedText(body, line);
  if (text === null) {
    return false;
  }
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
