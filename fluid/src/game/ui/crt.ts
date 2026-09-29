/**
 * An engram's markdown laid out for the CRT reader: a list of screen lines,
 * each at most `columns` characters wide, and where each section starts.
 *
 * The reader is a terminal, not a browser, so the markdown is flattened to
 * what an 80-column screen can show. Everything here is plain text; the
 * reader renders it as text nodes, so markup that survives the flattening
 * (an `<img>` tag in the prose, say) is shown literally and never parsed.
 *
 * The rules, in the order a line meets them:
 * - Frontmatter (a `---` block on the first line) is dropped.
 * - Fenced code (``` or ~~~) keeps its lines verbatim as `code`, hard-wrapped
 *   at `columns`. A fence tagged `mermaid` or `plantuml` becomes one `note`
 *   line `[DIAGRAM: <tag>]` instead, since a diagram's source is no use on a
 *   terminal.
 * - `#` to `######` headings become one `heading` line with the text, wrapped
 *   when long. The case is kept; the PETSCII look uppercases in the reader.
 * - List items (`-`, `*`, `+`, `1.`) keep their marker; a wrapped item hangs
 *   its continuation lines under the text after the marker. Nested lists are
 *   indented 2 spaces per level, whatever indent the source used, and a
 *   plain line right after an item continues that item.
 * - Blockquote lines become `quote` lines with a `> ` prefix.
 * - Table rows (lines starting with `|`) stay as `text`, trimmed.
 * - `![alt](src)` becomes a `note` line `[IMAGE: <alt or src>]`, splitting the
 *   paragraph around it.
 * - Everything else is a paragraph: its lines are joined and word-wrapped to
 *   `columns`, and a word longer than `columns` is hard-broken.
 * - Inline, `[text](url)` keeps the text, `[[Target]]` and
 *   `[[domain:Target|label]]` keep the label or else the target, and
 *   backticks are stripped. `**`, `__`, `*` and `_` are stripped only as
 *   emphasis markers around text (`**x**`, `__x__`, `*x*`, `_x_`), so a lone
 *   `*` as in `2*3` stays, and so does a `_` inside a word (`snake_case`).
 * - Runs of blank lines collapse to one `blank`; none leads or trails.
 *
 * `sections` maps the text of each `##` heading, exactly as written after the
 * hashes, to the index of its line, one entry per occurrence in document
 * order. That is the same text and the same order the room generator gives a
 * terminal (see `world/sections.ts`), so a terminal's heading and occurrence
 * find their place here.
 */

import { parseWikiTarget } from "../../wikilinks";

/** The width of the reader's screen, in characters. */
export const CRT_COLUMNS = 80;

/**
 * One line on the reader's screen. `kind` decides how it is drawn: a
 * `heading` in reverse video, a `note` for what the terminal cannot show,
 * and the rest as they read. `text` is never wider than the columns it was
 * laid out for.
 */
export type CrtLine = {
  kind: "text" | "heading" | "list" | "code" | "quote" | "note" | "blank";
  text: string;
};

const FENCE = /^\s*(```|~~~)\s*([^\s`]*)/;
const HEADING = /^(#{1,6})\s+(.*?)\s*#*\s*$/;
const SECTION = /^##\s+(.+?)\s*#*\s*$/;
const LIST_ITEM = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
const QUOTE = /^\s*>\s?(.*)$/;
const IMAGE = /!\[([^\]]*)\]\(\s*([^)\s]*)[^)]*\)/g;
const DIAGRAM_TAGS = new Set(["mermaid", "plantuml"]);

/** The note an image turns into: its alt text, or its source without one. */
function imageNote(alt: string, src: string): string {
  const label = alt.trim() === "" ? src : alt.trim();
  return `[IMAGE: ${label}]`;
}

/**
 * Inline markdown reduced to its text: images to their note, links and
 * wikilinks to their visible words, emphasis and code markers dropped.
 */
function plain(text: string): string {
  return text
    .replace(IMAGE, (_m, alt: string, src: string) => imageNote(alt, src))
    .replace(/\[\[([^[\]]+)\]\]/g, (_m, inner: string) => {
      const bar = inner.indexOf("|");
      if (bar >= 0) {
        const label = inner.slice(bar + 1).trim();
        if (label !== "") return label;
      }
      return parseWikiTarget(bar >= 0 ? inner.slice(0, bar) : inner).target;
    })
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/`/g, "")
    .replace(/\*\*(?=\S)(.+?)(?<=\S)\*\*/g, "$1")
    .replace(
      /(?<![\p{L}\p{N}_])__(?=\S)(.+?)(?<=\S)__(?![\p{L}\p{N}_])/gu,
      "$1",
    )
    .replace(/(?<![\p{L}\p{N}])\*(?=\S)(.+?)(?<=\S)\*(?![\p{L}\p{N}])/gu, "$1")
    .replace(/(?<![\p{L}\p{N}_])_(?=\S)(.+?)(?<=\S)_(?![\p{L}\p{N}_])/gu, "$1");
}

/** A string cut into pieces of at most `width` characters. */
function hardWrap(text: string, width: number): string[] {
  if (text.length <= width) return [text];
  const pieces: string[] = [];
  for (let i = 0; i < text.length; i += width) {
    pieces.push(text.slice(i, i + width));
  }
  return pieces;
}

/**
 * Words laid into lines of at most `width` characters, one space between
 * words. A word wider than a line is cut; its last piece can share a line
 * with the words after it. No words gives no lines.
 */
function wordWrap(text: string, width: number): string[] {
  const out: string[] = [];
  let line = "";
  for (const word of text.split(/\s+/)) {
    if (word === "") continue;
    const pieces = hardWrap(word, width);
    for (const [n, piece] of pieces.entries()) {
      const last = n === pieces.length - 1;
      if (line === "") {
        line = piece;
      } else if (line.length + 1 + piece.length <= width) {
        line = `${line} ${piece}`;
      } else {
        out.push(line);
        line = piece;
      }
      if (!last) {
        out.push(line);
        line = "";
      }
    }
  }
  if (line !== "") out.push(line);
  return out;
}

/** What the lines of a paragraph or list item are gathered into before layout. */
type OpenBlock =
  | { kind: "paragraph"; parts: string[] }
  | { kind: "list"; prefix: string; parts: string[] };

/** The source lines of `markdown` after a leading `---` frontmatter block. */
function bodyLines(markdown: string): string[] {
  const source = markdown.split(/\r?\n/);
  if (source[0]?.trim() !== "---") return source;
  let i = 1;
  while (i < source.length && source[i]?.trim() !== "---") i++;
  return source.slice(i + 1);
}

/**
 * The part of an engram's markdown the reader shows: everything after a
 * leading `---` frontmatter block, which the reader never draws.
 */
export function readerBody(markdown: string): string {
  return bodyLines(markdown).join("\n");
}

/**
 * An engram's markdown as screen lines, `columns` wide, plus the line index
 * of each `##` section by heading text. See the module doc for the rules.
 */
export function crtLines(
  markdown: string,
  columns = CRT_COLUMNS,
): { lines: CrtLine[]; sections: Map<string, number[]> } {
  const width = Math.max(1, Math.floor(columns));
  const source = bodyLines(markdown);
  const lines: CrtLine[] = [];
  const sections = new Map<string, number[]>();

  let i = 0;

  let open: OpenBlock | null = null;
  // The source indent of each open list level, outermost first.
  let listIndents: number[] = [];

  const push = (kind: CrtLine["kind"], text: string) => {
    lines.push({ kind, text });
  };
  const blank = () => {
    const last = lines[lines.length - 1];
    if (last !== undefined && last.kind !== "blank") push("blank", "");
  };
  // Lays out a paragraph's text, turning each image in it into a note line
  // of its own between the words before and after.
  const paragraph = (text: string) => {
    let rest = text;
    for (;;) {
      IMAGE.lastIndex = 0;
      const image = IMAGE.exec(rest);
      if (image === null) break;
      for (const line of wordWrap(plain(rest.slice(0, image.index)), width)) {
        push("text", line);
      }
      for (const line of hardWrap(
        imageNote(image[1] ?? "", image[2] ?? ""),
        width,
      )) {
        push("note", line);
      }
      rest = rest.slice(image.index + image[0].length);
    }
    for (const line of wordWrap(plain(rest), width)) push("text", line);
  };
  const flush = () => {
    if (open === null) return;
    const text = open.parts.join(" ");
    if (open.kind === "paragraph") {
      paragraph(text);
    } else {
      const hang = " ".repeat(open.prefix.length);
      const wrapped = wordWrap(
        plain(text),
        Math.max(1, width - open.prefix.length),
      );
      if (wrapped.length === 0) wrapped.push("");
      for (const [n, line] of wrapped.entries()) {
        const row = `${n === 0 ? open.prefix : hang}${line}`;
        for (const piece of hardWrap(row, width)) push("list", piece);
      }
    }
    open = null;
  };

  for (; i < source.length; i++) {
    const line = source[i] ?? "";

    const fence = FENCE.exec(line);
    if (fence !== null) {
      flush();
      listIndents = [];
      const marker = fence[1] ?? "```";
      const tag = (fence[2] ?? "").toLowerCase();
      const body: string[] = [];
      i++;
      while (
        i < source.length &&
        !(source[i] ?? "").trim().startsWith(marker)
      ) {
        body.push(source[i] ?? "");
        i++;
      }
      if (DIAGRAM_TAGS.has(tag)) {
        push("note", `[DIAGRAM: ${tag}]`);
      } else {
        for (const code of body) {
          for (const piece of hardWrap(code, width)) push("code", piece);
        }
      }
      continue;
    }

    if (line.trim() === "") {
      flush();
      blank();
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading !== null) {
      flush();
      listIndents = [];
      const section = SECTION.exec(line);
      if (section !== null && !line.startsWith("###")) {
        const key = section[1] ?? "";
        const at = sections.get(key);
        if (at === undefined) sections.set(key, [lines.length]);
        else at.push(lines.length);
      }
      const text = wordWrap(plain(heading[2] ?? ""), width);
      if (text.length === 0) text.push("");
      for (const row of text) push("heading", row);
      continue;
    }

    const item = LIST_ITEM.exec(line);
    if (item !== null) {
      flush();
      const indent = (item[1] ?? "").replace(/\t/g, "    ").length;
      while (listIndents.length > 0 && (listIndents.at(-1) ?? 0) > indent) {
        listIndents.pop();
      }
      if (listIndents.at(-1) !== indent) listIndents.push(indent);
      const level = listIndents.length - 1;
      open = {
        kind: "list",
        prefix: `${"  ".repeat(level)}${item[2] ?? "-"} `,
        parts: [item[3] ?? ""],
      };
      continue;
    }

    const quote = QUOTE.exec(line);
    if (quote !== null) {
      flush();
      listIndents = [];
      const text = wordWrap(plain(quote[1] ?? ""), Math.max(1, width - 2));
      if (text.length === 0) text.push("");
      for (const row of text) push("quote", `> ${row}`.trimEnd());
      continue;
    }

    if (line.trim().startsWith("|")) {
      flush();
      listIndents = [];
      for (const piece of hardWrap(plain(line.trim()), width)) {
        push("text", piece);
      }
      continue;
    }

    // A plain line continues the open paragraph or list item, or starts a
    // new paragraph.
    if (open === null) {
      listIndents = [];
      open = { kind: "paragraph", parts: [] };
    }
    open.parts.push(line.trim());
  }
  flush();

  while (lines.at(-1)?.kind === "blank") lines.pop();
  return { lines, sections };
}
