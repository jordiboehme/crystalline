/**
 * A reflection's text, cut the way the engine cuts it.
 *
 * A comment anchors on a section id plus a quoted passage and its
 * occurrence, and the engine finds the passage again with `resolve_anchor`
 * over `split_sections` (core plan, Tasks 3 and 5): the heading line and the
 * body joined with LF, the id line left out, the text before the first `##`
 * as the section `_top`. This module produces exactly that text, so a quote
 * picked here is a quote the engine finds, and it counts occurrences the
 * way the engine walks them: left to right, never overlapping.
 *
 * The cut uses the heading lines the server already found (`sections[].line`
 * of `GET .../reflections/{path}`, one-based, frontmatter counted) rather
 * than a second fence-aware heading parser here, which could only drift.
 */

/** The id of the text before the first `##`, the engine's `TOP_SECTION`. */
export const TOP_SECTION = "_top";

/** The frontmatter block, as `MarkdownBody` spells it. */
const FRONTMATTER = /^---\r?\n[\s\S]*?\r?\n---[ \t]*(\r?\n|$)/;

/** A section id line: `<!-- r:<4 to 16 hex> -->` alone on its line (core `parse_id_line` trims). */
const ID_LINE = /^\s*<!-- r:[0-9a-f]{4,16} -->\s*$/;

/** Whether this line of a page is a section id line. */
export function isIdLine(line: string): boolean {
  return ID_LINE.test(line);
}

/**
 * The text with every id line removed: what a person is shown of a page, a
 * snippet or, with `diff`, a unified diff (whose lines carry one marker
 * character first). Ids are bookkeeping, never content.
 */
export function stripIdLines(text: string, diff = false): string {
  return text
    .split("\n")
    .filter((line) => !isIdLine(diff ? line.slice(1) : line))
    .join("\n");
}

/** One section's text, in the engine's shape. */
export interface SectionText {
  /** The id, `_top` for the intro, null for a heading that has none yet. */
  id: string | null;
  /** The heading text, empty for `_top`. */
  heading: string;
  /** The heading line and the body, LF-joined, without the id line. */
  text: string;
}

/** One section's text and where each of its lines stands in the page. */
interface Cut {
  section: SectionText;
  /** The zero-based page line of each line of `section.text`; the id line is the gap. */
  lines: number[];
}

/**
 * The page cut into its sections, `_top` first.
 *
 * `sections` is the server's list in file order. A file that ends in a line
 * break keeps its empty last element, so the last section's text (or the
 * `_top` text of a page with no `##`) ends in "\n", exactly as the engine's
 * `split_sections` returns it.
 */
export function sectionTexts(
  markdown: string,
  sections: readonly { id: string | null; heading: string; line: number }[],
): SectionText[] {
  return cutSections(markdown, sections).map((cut) => cut.section);
}

/** The cut behind {@link sectionTexts}, with each section's line map. */
function cutSections(
  markdown: string,
  sections: readonly { id: string | null; heading: string; line: number }[],
): Cut[] {
  const lf = markdown.replace(/\r\n/g, "\n");
  const front = FRONTMATTER.exec(lf);
  // One-based, as the engine counts it: the lines the frontmatter took, plus one.
  const bodyLine = front === null ? 1 : front[0].split("\n").length;
  const lines = lf.split("\n");
  // Where each heading really is: at its `line` when that line is the
  // heading, else the next line after the previous cut that is (defensive:
  // the server counts `line` on the markdown it answers). A heading that is
  // nowhere is left out, and a thread on it reads as its section gone.
  const starts: number[] = [];
  let cursor = bodyLine - 1;
  for (const section of sections) {
    let at = section.line - 1;
    if (at < cursor || !isLevelTwo(lines[at])) {
      at = lines.findIndex(
        (line, i) => i >= cursor && isHeading(line, section.heading),
      );
    }
    starts.push(at);
    if (at >= 0) {
      cursor = at + 1;
    }
  }
  const found = starts.filter((at) => at >= 0);
  const numbers = (from: number, to: number) =>
    Array.from({ length: Math.max(to - from, 0) }, (_, i) => from + i);
  const topEnd = found[0] ?? lines.length;
  const out: Cut[] = [
    {
      section: {
        id: TOP_SECTION,
        heading: "",
        text: lines.slice(bodyLine - 1, topEnd).join("\n"),
      },
      lines: numbers(bodyLine - 1, topEnd),
    },
  ];
  sections.forEach((section, index) => {
    const start = starts[index] ?? -1;
    if (start < 0) {
      return;
    }
    const end = found.find((at) => at > start) ?? lines.length;
    const own = lines.slice(start, end);
    const at = numbers(start, end);
    if (own.length > 1 && isIdLine(own[1] ?? "")) {
      own.splice(1, 1);
      at.splice(1, 1);
    }
    out.push({
      section: {
        id: section.id,
        heading: section.heading,
        text: own.join("\n"),
      },
      lines: at,
    });
  });
  return out;
}

/**
 * Where `offset`, counted into the text of the `index`-th section of
 * {@link sectionTexts} (`_top` is 0), stands in the page: the frontmatter,
 * the id lines and any CR before a line break counted, exactly as the GET
 * answered it. Null when the offset is not inside that section.
 */
export function pageOffset(
  markdown: string,
  sections: readonly { id: string | null; heading: string; line: number }[],
  index: number,
  offset: number,
): number | null {
  const cut = cutSections(markdown, sections)[index];
  if (cut === undefined || offset < 0 || offset > cut.section.text.length) {
    return null;
  }
  const before = cut.section.text.slice(0, offset);
  const row = before.split("\n").length - 1;
  const column = offset - (before.lastIndexOf("\n") + 1);
  const line = cut.lines[row];
  if (line === undefined) {
    return null;
  }
  const page = markdown.split("\n");
  let start = 0;
  for (let i = 0; i < line; i += 1) {
    start += (page[i]?.length ?? 0) + 1;
  }
  const own = (page[line] ?? "").replace(/\r$/, "");
  return column > own.length ? null : start + column;
}

/**
 * An ATX heading line of level two, as the engine's `parse_heading` reads it:
 * up to three spaces of indent, exactly two `#`, then the end of the line or
 * a space or tab. Its text is trimmed and loses a closing `#` run.
 */
const LEVEL_TWO = /^ {0,3}##(?:[ \t]+(.*))?$/;
function headingText(line: string | undefined): string | null {
  const match = LEVEL_TWO.exec(line ?? "");
  if (match === null) {
    return null;
  }
  const text = (match[1] ?? "").trim();
  const without = text.replace(/#+$/, "");
  if (without.length === text.length) {
    return text;
  }
  if (without === "") {
    return "";
  }
  return /[ \t]$/.test(without) ? without.trim() : text;
}
/** Whether a line is a level-two heading at all: the server said one is here. */
export function isLevelTwo(line: string | undefined): boolean {
  return headingText(line) !== null;
}
function isHeading(line: string | undefined, heading: string): boolean {
  return headingText(line) === heading;
}

/**
 * Which occurrence of `quote` in `text` starts at `start`, counted the way
 * `resolve_anchor` walks: each match resumes after the previous one ends, so
 * matches never overlap. A selection that starts inside an earlier match
 * (possible only with overlapping text) is filed under that match.
 */
export function occurrenceAt(
  text: string,
  quote: string,
  start: number,
): number {
  let from = 0;
  let n = 0;
  for (;;) {
    const at = text.indexOf(quote, from);
    if (at < 0 || at > start) {
      return Math.max(n, 1);
    }
    n += 1;
    if (at === start) {
      return n;
    }
    from = at + quote.length;
  }
}

/** Where the `occurrence`-th match of `quote` is, or null when it is gone. */
export function locateQuote(
  text: string,
  quote: string,
  occurrence: number,
): { start: number; end: number } | null {
  if (quote === "") {
    return { start: 0, end: text.length };
  }
  let from = 0;
  for (let n = 1; n <= Math.max(occurrence, 1); n += 1) {
    const at = text.indexOf(quote, from);
    if (at < 0) {
      return null;
    }
    if (n === Math.max(occurrence, 1)) {
      return { start: at, end: at + quote.length };
    }
    from = at + quote.length;
  }
  return null;
}

/**
 * The page without the `reflection:` block of its frontmatter: the text a
 * live room holds (core Task 18), so a conflict's "theirs" compares like
 * with like. The block is a top-level key; it ends at the next top-level key
 * or at the closing `---`.
 */
export function stripProvenance(markdown: string): string {
  return stripTopKeys(markdown, PROVENANCE_KEYS);
}

const PROVENANCE_KEYS = ["reflection"];

/**
 * The page settings a curator changes beside the text (`freshness`,
 * `follows`, `status`), with the block: what a live room leaves out of its
 * text (the engine's `without_page_keys` over `without_provenance`). The
 * editor never shows them, and a conflict compares the room's own text.
 */
const ROOM_HIDDEN_KEYS = ["reflection", "freshness", "follows", "status"];

/** The page as a live room holds it: no block, no page settings. */
export function roomText(markdown: string): string {
  return stripTopKeys(markdown, ROOM_HIDDEN_KEYS);
}

/** The page without the top-level frontmatter `keys`, each with its block. */
function stripTopKeys(markdown: string, keys: readonly string[]): string {
  const front = FRONTMATTER.exec(markdown);
  if (front === null) {
    return markdown;
  }
  const lines = front[0].split("\n");
  const kept: string[] = [];
  let inBlock = false;
  lines.forEach((raw, index) => {
    // A CRLF page splits into lines that end in `\r`; the line is judged
    // without it and kept with it.
    const line = raw.replace(/\r$/, "");
    const topLevel =
      index === 0 || /^---[ \t]*$/.test(line) || /^[^\s#-][^:]*:/.test(line);
    if (topLevel) {
      inBlock = keys.some((key) => line.startsWith(`${key}:`));
    }
    if (!inBlock) {
      kept.push(raw);
    }
  });
  // Nothing left between the fences: the fences go too and the body loses
  // its leading blank lines, which is the engine's cut (core Task 12,
  // `without_provenance`), so a conflict compares the room's own text.
  if (kept.every((line) => line.trim() === "---" || line.trim() === "")) {
    return markdown.slice(front[0].length).replace(/^[\r\n]+/, "");
  }
  return kept.join("\n") + markdown.slice(front[0].length);
}

/** How long something has waited, in the largest whole unit. */
export function ageText(seconds: number): string {
  if (seconds < 60) {
    return "just now";
  }
  const [size, one, many] =
    seconds < 3600
      ? [60, "minute", "minutes"]
      : seconds < 86400
        ? [3600, "hour", "hours"]
        : [86400, "day", "days"];
  const count = Math.floor(seconds / size);
  return `${String(count)} ${count === 1 ? one : many}`;
}

/**
 * The text of a page's own title heading: the first `# Title` line, or the
 * first line of text underlined with `=`, outside fenced code. A line inside a
 * fence is code, not a title, and so is everything after a fence that is
 * never closed. Null when there is none. The server reads a page's title the
 * same way (`title_of`), after its frontmatter `title`.
 */
export function firstTitle(text: string): string | null {
  let fence: string | null = null;
  // The line before, when it is text an `=` line would turn into a title.
  let previous: string | null = null;
  for (const line of text.split(/\r?\n/)) {
    const marker = /^ {0,3}(`{3,}|~{3,})/.exec(line)?.[1];
    if (fence !== null) {
      if (
        marker !== undefined &&
        marker[0] === fence[0] &&
        marker.length >= fence.length
      ) {
        fence = null;
      }
      continue;
    }
    if (marker !== undefined) {
      fence = marker;
      previous = null;
      continue;
    }
    const atx = /^ {0,3}#[ \t]+(.*?)(?:[ \t]+#+)?[ \t]*$/.exec(line);
    if (atx !== null) {
      return atx[1] ?? "";
    }
    if (previous !== null && /^ {0,3}=+[ \t]*$/.test(line)) {
      return previous.trim();
    }
    previous =
      line.trim() !== "" && !/^ {0,3}#{1,6}([ \t]|$)/.test(line) ? line : null;
  }
  return null;
}

/**
 * Whether the text before the first `##` holds the page's own title: a
 * `# Title` line, or a line of text underlined with `=`. Not only the first
 * line: an image or a paragraph may stand above it.
 */
export function hasOwnTitle(intro: string): boolean {
  return firstTitle(intro) !== null;
}
