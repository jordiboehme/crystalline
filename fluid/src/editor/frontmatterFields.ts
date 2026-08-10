/**
 * Line-level field access to the frontmatter block. Deliberately NOT a YAML
 * parser: parsing and re-emitting the whole block would reformat lines the
 * user never touched, which is the diff noise this editor exists to never
 * make. Each write is the smallest edit that says what changed - one scalar
 * line, or one tags entry - and everything else keeps its bytes.
 *
 * Scope, honestly stated: top-level `key: value` scalars and the tags list
 * (inline or block). A key written in a fancier YAML shape (anchors, flow
 * maps) is read as its raw text and rewritten as a plain scalar when edited
 * through the form - the raw editor is always there for the fancy cases.
 *
 * String mathematics only, over the buffer's own text: nothing here reads a
 * CodeMirror document, so the offsets are offsets into that string. A caller
 * dispatching them into a view translates them through the document's line API
 * first, because a CRLF buffer counts each break as one position and these
 * offsets count it as two.
 */

import { separatorOf } from "./setup";

/** One edit, as offsets into the string it was computed from. */
export interface FieldEdit {
  from: number;
  to: number;
  insert: string;
}

/** One line, as the offsets that bound it, terminator included. */
interface LineSpan {
  lineStart: number;
  lineEnd: number;
}

/** The block's inner line spans, or null when there is no block to edit. */
function blockLines(doc: string): LineSpan[] | null {
  if (!doc.startsWith("---\n") && !doc.startsWith("---\r\n")) {
    return null;
  }
  const newline = separatorOf(doc);
  const lines: LineSpan[] = [];
  let at = doc.indexOf(newline) + newline.length;
  while (at < doc.length) {
    let end = doc.indexOf(newline, at);
    if (end === -1) {
      end = doc.length;
    } else {
      end += newline.length;
    }
    const text = doc.slice(at, end).replace(/\r?\n$/, "");
    if (text.trimEnd() === "---") {
      return lines;
    }
    lines.push({ lineStart: at, lineEnd: end });
    at = end;
  }
  // Never closed: not a block this form edits.
  return null;
}

/**
 * Whether there is a block here for the form to edit at all.
 *
 * The same question `writeScalar` and `writeTagList` answer by returning null,
 * asked before anything is drawn: a form whose every edit silently did nothing
 * - because the opening fence was never closed, say - would be worse than the
 * plain note a caller shows instead.
 */
export function hasFrontmatterBlock(doc: string): boolean {
  return blockLines(doc) !== null;
}

/** One line's text without its terminator. */
function lineText(doc: string, span: LineSpan): string {
  return doc.slice(span.lineStart, span.lineEnd).replace(/\r?\n$/, "");
}

function unquote(value: string): string {
  const trimmed = value.trim();
  if (
    trimmed.length >= 2 &&
    ((trimmed.startsWith('"') && trimmed.endsWith('"')) ||
      (trimmed.startsWith("'") && trimmed.endsWith("'")))
  ) {
    return trimmed.slice(1, -1);
  }
  return trimmed;
}

/** Quote a value yaml would misread as structure; pass plain ones through. */
function plainOrQuoted(value: string): string {
  return /[:#[\]{}&*!|>'"%@`]|^\s|\s$/.test(value) &&
    !/^\d+(\.\d+)?$/.test(value)
    ? `"${value.replace(/"/g, '\\"')}"`
    : value;
}

function findKey(doc: string, key: string): LineSpan | null {
  const lines = blockLines(doc);
  if (!lines) {
    return null;
  }
  const shape = new RegExp(`^${key}\\s*:`);
  return lines.find((span) => shape.test(lineText(doc, span))) ?? null;
}

/** The trimmed, unquoted value of a top-level scalar key, or null. */
export function readScalar(doc: string, key: string): string | null {
  const span = findKey(doc, key);
  if (!span) {
    return null;
  }
  const text = lineText(doc, span);
  const value = unquote(text.slice(text.indexOf(":") + 1));
  return value === "" ? null : value;
}

/**
 * The edit that sets, rewrites or (value = null) removes the key's line.
 * Null when the document has no frontmatter block to edit.
 */
export function writeScalar(
  doc: string,
  key: string,
  value: string | null,
): FieldEdit | null {
  const lines = blockLines(doc);
  if (!lines) {
    return null;
  }
  const newline = separatorOf(doc);
  const span = findKey(doc, key);
  if (value === null) {
    // Absent means unbounded (or unset): the line goes, whole. Nothing here
    // ever writes a sentinel date or an empty value in its place.
    return span ? { from: span.lineStart, to: span.lineEnd, insert: "" } : null;
  }
  const line = `${key}: ${plainOrQuoted(value)}`;
  if (span) {
    const text = lineText(doc, span);
    return {
      from: span.lineStart,
      to: span.lineStart + text.length,
      insert: line,
    };
  }
  // A new key lands just before the closing fence.
  const last = lines.at(-1);
  const at = last ? last.lineEnd : doc.indexOf(newline) + newline.length;
  return { from: at, to: at, insert: `${line}${newline}` };
}

/** The tags entry's whole span (key line plus any block items), or null. */
function tagsSpan(
  doc: string,
): { from: number; to: number; inline: string | null } | null {
  const lines = blockLines(doc);
  if (!lines) {
    return null;
  }
  const index = lines.findIndex((span) =>
    /^tags\s*:/.test(lineText(doc, span)),
  );
  if (index === -1) {
    return null;
  }
  const key = lines[index];
  if (!key) {
    return null;
  }
  const keyText = lineText(doc, key);
  const after = keyText.slice(keyText.indexOf(":") + 1).trim();
  let to = key.lineEnd;
  if (after === "") {
    for (let i = index + 1; i < lines.length; i += 1) {
      const item = lines[i];
      if (!item || !/^\s+-\s/.test(lineText(doc, item))) {
        break;
      }
      to = item.lineEnd;
    }
  }
  return { from: key.lineStart, to, inline: after === "" ? null : after };
}

/** The tags, from an inline [a, b] or a block list. */
export function readTagList(doc: string): string[] {
  const span = tagsSpan(doc);
  if (!span) {
    return [];
  }
  if (span.inline !== null) {
    return span.inline
      .replace(/^\[|\]$/g, "")
      .split(",")
      .map((tag) => unquote(tag))
      .filter((tag) => tag !== "");
  }
  return doc
    .slice(span.from, span.to)
    .split(/\r?\n/)
    .slice(1)
    .map((line) => line.trim())
    .filter((line) => line.startsWith("- "))
    .map((line) => unquote(line.slice(2)));
}

/**
 * The edit that replaces the whole tags entry with a block list (or removes
 * it for an empty list). Null without a frontmatter block.
 */
export function writeTagList(doc: string, tags: string[]): FieldEdit | null {
  const lines = blockLines(doc);
  if (!lines) {
    return null;
  }
  const newline = separatorOf(doc);
  const span = tagsSpan(doc);
  if (tags.length === 0) {
    return span ? { from: span.from, to: span.to, insert: "" } : null;
  }
  const entry =
    `tags:${newline}` +
    tags.map((tag) => `  - ${plainOrQuoted(tag)}`).join(newline) +
    newline;
  if (span) {
    return { from: span.from, to: span.to, insert: entry };
  }
  const last = lines.at(-1);
  const at = last ? last.lineEnd : doc.indexOf(newline) + newline.length;
  return { from: at, to: at, insert: entry };
}
