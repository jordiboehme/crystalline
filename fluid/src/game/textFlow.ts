/**
 * The inline rules and the wrap shared by the CRT reader (`ui/crt.ts`) and
 * the text drawn on walls (`render/text.ts`), so a link reads as its words in
 * both. Pure functions on strings; the only import is Fluid's wikilink
 * parser, so the generator side never needs this module and the render side
 * may use it.
 *
 * `fitRows` lays lines into a fixed number of rows of a fixed width: each line
 * is reduced to its text, word-wrapped, and what does not fit is cut with
 * `CUT_MARK` on the last row (0.22 R10, R11).
 */

import { parseWikiTarget } from "../wikilinks";

/** An inline markdown image: its alt text and its source. */
export const IMAGE = /!\[([^\]]*)\]\(\s*([^)\s]*)[^)]*\)/g;

/** The note an image turns into: its alt text, or its source without one. */
export function imageNote(alt: string, src: string): string {
  const label = alt.trim() === "" ? src : alt.trim();
  return `[IMAGE: ${label}]`;
}

/**
 * Inline markdown reduced to its text: images to their note, links and
 * wikilinks to their visible words, emphasis and code markers dropped.
 */
export function plainInline(text: string): string {
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
export function hardWrap(text: string, width: number): string[] {
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
export function wordWrap(text: string, width: number): string[] {
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

/** The mark a cut row ends with. */
export const CUT_MARK = "\u2026";

/**
 * `lines` as at most `rows` rows of at most `columns` characters: each line
 * through `plainInline`, then `wordWrap`; empty lines dropped; when text is
 * left over, the last row is cut to `columns - 1` characters (at its last
 * space when that lies in the row's last third) and ends with `CUT_MARK`.
 * 0.22 R10, R11.
 */
export function fitRows(
  lines: readonly string[],
  columns: number,
  rows: number,
): string[] {
  const width = Math.max(1, Math.floor(columns));
  if (rows <= 0) return [];
  const all = lines.flatMap((line) => wordWrap(plainInline(line), width));
  if (all.length <= rows) return all;
  const kept = all.slice(0, rows);
  const room = width - 1;
  const last = (kept[rows - 1] ?? "").slice(0, room);
  const space = last.lastIndexOf(" ");
  const cut =
    space > 0 && space >= (room * 2) / 3 ? last.slice(0, space) : last;
  kept[rows - 1] = `${cut.trimEnd()}${CUT_MARK}`;
  return kept;
}
