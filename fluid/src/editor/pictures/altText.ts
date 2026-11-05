/**
 * A picture's alt text between the source an author wrote and the words a
 * field shows.
 *
 * The field shows the words the reading page gives the image (M3): the alt
 * is read as inline Markdown, its emphasis, strikethrough and code marks, a
 * link's brackets and address and an autolink's angle brackets are left out, character references are
 * decoded and backslash escapes taken out. An edit writes back only the part
 * that changed, worked out in display space and mapped to the source, so
 * untouched characters keep their bytes, marks and escapes included. What is
 * written is encoded so the reference still parses as written and the words
 * read as typed: brackets and backslashes escaped, never a `](` the core's
 * scanner would read as the end of the alt (it finds references by `](`),
 * and a typed `*`, `_`, `~`, backtick, `&` or `<` escaped only where it
 * would otherwise be read as Markdown.
 */

import { markdownLanguage } from "@codemirror/lang-markdown";
import type { NodeSet } from "@lezer/common";
import { decodeNamedCharacterReference } from "decode-named-character-reference";

/** Characters that may start Markdown inside an alt; without any, it is plain. */
const MARKUP = /[\\&*_~`[<]/;

/**
 * One inline element as the parser builds it (`children` is internal to
 * `@lezer/markdown`, as the table cell renderer notes too).
 */
interface InlineElement {
  readonly type: number;
  readonly from: number;
  readonly to: number;
  readonly children?: readonly InlineElement[];
}

interface InlineParser {
  readonly nodeSet: NodeSet;
  parseInline(text: string, offset: number): InlineElement[];
}

/** The page's own Markdown parser (GFM), read for inline content only. */
const inline = markdownLanguage.parser as unknown as InlineParser;

/** The marks the reading page leaves out of an alt wherever they stand. */
const DROPPED = new Set(["EmphasisMark", "StrikethroughMark", "CodeMark"]);

/** A link's or an image's parts that are no words of it. */
const LINK_PARTS = new Set(["LinkMark", "URL", "LinkTitle", "LinkLabel"]);

/** A character reference as written, decoded, or null when it is none. */
function decodeEntity(text: string): string | null {
  const named = /^&(\w+);$/.exec(text);
  if (named) {
    const value = decodeNamedCharacterReference(named[1] ?? "");
    return value === false ? null : value;
  }
  const number = /^&#(?:(\d+)|[xX]([0-9a-fA-F]+));$/.exec(text);
  if (!number) return null;
  const code =
    number[1] !== undefined
      ? Number(number[1])
      : Number.parseInt(number[2] ?? "", 16);
  // As the reading page reads it: zero and out of range are U+FFFD.
  return code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : "\uFFFD";
}

/**
 * The source spans the reading page shows differently: left out (`""`) or
 * shown as other text (an entity, an escape).
 */
function rewrites(
  source: string,
): { from: number; to: number; text: string }[] {
  const out: { from: number; to: number; text: string }[] = [];
  const name = (element: InlineElement) =>
    inline.nodeSet.types[element.type]?.name ?? "";
  const walk = (elements: readonly InlineElement[], inLink: boolean) => {
    for (const element of elements) {
      const kind = name(element);
      const span = source.slice(element.from, element.to);
      if (DROPPED.has(kind) || (inLink && LINK_PARTS.has(kind))) {
        out.push({ from: element.from, to: element.to, text: "" });
      } else if (kind === "Escape") {
        out.push({ from: element.from, to: element.to, text: span.slice(1) });
      } else if (kind === "Entity") {
        const text = decodeEntity(span);
        if (text !== null) {
          out.push({ from: element.from, to: element.to, text });
        }
      } else {
        // A link or an image written inline (with an address) shows its
        // words; a bracket with no address reads as the text it is.
        const children = element.children ?? [];
        const linked =
          (kind === "Link" || kind === "Image") &&
          children.some((child) => name(child) === "URL");
        if (kind === "Autolink") {
          // `<https://x.y>` reads as its address: the brackets go.
          for (const child of children) {
            if (name(child) === "LinkMark") {
              out.push({ from: child.from, to: child.to, text: "" });
            }
          }
          continue;
        }
        walk(children, linked);
      }
    }
  };
  walk(inline.parseInline(source, 0), false);
  return out.sort((a, b) => a.from - b.from);
}

/**
 * The display text of `source`, with each display character's source span:
 * `starts[i]` is where display character `i` begins in the source, and
 * `starts[display.length]` is the source's length. A left-out mark has no
 * display character; a decoded entity's characters all start at the entity.
 */
function readAlt(source: string): {
  display: string;
  starts: number[];
  /** The source spans left out of the display (marks, brackets). */
  dropped: { from: number; to: number }[];
} {
  let display = "";
  const starts: number[] = [];
  const dropped: { from: number; to: number }[] = [];
  if (!MARKUP.test(source)) {
    for (let at = 0; at <= source.length; at += 1) starts.push(at);
    return { display: source, starts, dropped };
  }
  let at = 0;
  for (const rewrite of rewrites(source)) {
    if (rewrite.from < at) continue;
    if (rewrite.text === "") {
      dropped.push({ from: rewrite.from, to: rewrite.to });
    }
    for (; at < rewrite.from; at += 1) {
      starts.push(at);
      display += source[at] ?? "";
    }
    for (let i = 0; i < rewrite.text.length; i += 1) starts.push(rewrite.from);
    display += rewrite.text;
    at = rewrite.to;
  }
  for (; at < source.length; at += 1) {
    starts.push(at);
    display += source[at] ?? "";
  }
  starts.push(source.length);
  return { display, starts, dropped };
}

/** `source` as a field shows it: as the reading page reads the alt. */
export function altDisplay(source: string): string {
  return readAlt(source).display;
}

/** The characters an escape keeps from being read as Markdown. */
const MARK_CHARS = new Set(["*", "_", "~", "`", "&", "<"]);

/**
 * Typed text as alt source. `before` is the source character right in front
 * of it, so a `(` written right after a `]` is escaped too. `strict` also
 * escapes every character that could be read as Markdown.
 */
function encode(text: string, before: string, strict = false): string {
  let out = "";
  let last = before;
  for (const char of text) {
    let written: string;
    if (char === "\\" || char === "[" || char === "]") {
      written = `\\${char}`;
    } else if (char === "(" && last === "]") {
      written = "\\(";
    } else if (strict && MARK_CHARS.has(char)) {
      written = `\\${char}`;
    } else {
      written = char;
    }
    out += written;
    last = written[written.length - 1] ?? last;
  }
  return out;
}

/** `source` with `edit` applied. */
function apply(
  source: string,
  edit: { from: number; to: number; insert: string },
): string {
  return source.slice(0, edit.from) + edit.insert + source.slice(edit.to);
}

/**
 * The one change that turns alt source `source` into one that displays as
 * `next`, in offsets inside the alt span, or null when nothing changed.
 * The changed middle is written as typed where that reads as typed, then
 * with every Markdown character escaped; where neither does (an edit that
 * took one mark of a pair and left the other), the whole alt is written
 * anew, escaped, so the field never shows what the page would not.
 */
export function altEdit(
  source: string,
  typed: string,
): { from: number; to: number; insert: string } | null {
  // A line break or a tab is written as one space (the alt is one line).
  const next = typed.replace(/\r\n|[\r\n\t]/g, " ");
  const { display, starts, dropped } = readAlt(source);
  if (display === next) {
    return null;
  }
  let prefix = 0;
  const shortest = Math.min(display.length, next.length);
  while (prefix < shortest && display[prefix] === next[prefix]) {
    prefix += 1;
  }
  let suffix = 0;
  while (
    suffix < shortest - prefix &&
    display[display.length - 1 - suffix] === next[next.length - 1 - suffix]
  ) {
    suffix += 1;
  }
  const from = starts[prefix] ?? source.length;
  const to = starts[display.length - suffix] ?? source.length;
  const middle = next.slice(prefix, next.length - suffix);
  // Where the change starts: right at the first changed character, else in
  // front of the marks just before it (text typed before an autolink goes
  // before its `<`, not inside it). An insert ends where it starts.
  let before = from;
  for (let moved = true; moved;) {
    moved = false;
    for (const span of dropped) {
      if (span.to === before && span.from < before) {
        before = span.from;
        moved = true;
      }
    }
  }
  for (const start of before === from ? [from] : [from, before]) {
    for (const strict of [false, true]) {
      const edit = middleEdit(
        source,
        start,
        to === from ? start : to,
        middle,
        strict,
      );
      if (altDisplay(apply(source, edit)) === next) return edit;
    }
  }
  return middleEdit(source, 0, source.length, next, true);
}

/** `middle` written over `from`..`to`, never a `](` against the rest. */
function middleEdit(
  source: string,
  from: number,
  to: number,
  middle: string,
  strict: boolean,
): { from: number; to: number; insert: string } {
  let end = to;
  let insert = encode(middle, source[from - 1] ?? "", strict);
  // The changed part may end in a `]` (typed, or the untouched one before a
  // deletion) right against an untouched `(`: that `(` is escaped too.
  const last =
    insert === "" ? (source[from - 1] ?? "") : insert[insert.length - 1];
  if (last === "]" && source[end] === "(") {
    end += 1;
    insert += "\\(";
  }
  return { from, to: end, insert };
}
