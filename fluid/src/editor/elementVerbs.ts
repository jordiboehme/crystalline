/**
 * What an element popover writes, as changes relative to the document. Every
 * verb re-reads its element at `at` from the state it is given, so a request
 * the document moved away from (a peer deleted the link, the text changed
 * under it) returns null and the popover closes without writing. Nothing here
 * dispatches: the host does, as one gesture on the view that asked.
 *
 * Each verb writes only the span it edits: an address, the brackets of a new
 * link, the two link marks of an unlink, a wikilink target. Positions are
 * document positions; spans are read with `doc.sliceString`.
 */

import { syntaxTree } from "@codemirror/language";
import type { ChangeSpec, EditorState } from "@codemirror/state";
import type { SyntaxNode } from "@lezer/common";

import { isWebAddress } from "../format";
import { isIdLine } from "../reflectionText";
import { frontmatterRegion } from "./frontmatterRegion";
import { linkDefinition, marksOnLine } from "./marks/hiddenMarks";
import { CODE_CONTEXTS } from "./prose";

export interface Span {
  from: number;
  to: number;
}

/** A Markdown link as the popover reads it. */
export interface LinkElement extends Span {
  /** The opening `[`. */
  open: Span;
  /** The closing marks: `](address)`, `][ref]`, `][]` or `]`. */
  close: Span;
  /** The words between the brackets. */
  text: Span;
  /** The address span of an inline link (empty for `[t]()`), null for a reference. */
  address: Span | null;
  /** A reference link, whose address is a shared definition. */
  reference: boolean;
  /** The label a reference link names its definition by. */
  label: string;
}

/** A `[[Target]]` or `[[Target|words]]` as the popover reads it. */
export interface WikilinkElement extends Span {
  /** The target text, before a `|` when an alias is written. */
  target: Span;
  /** The alias words after the `|`, if written. */
  alias: Span | null;
}

/** Subtrees no popover edits in: code, comments (a Reflection's ids) and math. */
const NOT_PROSE = new Set([
  ...CODE_CONTEXTS,
  "Comment",
  "CommentBlock",
  "InlineMath",
]);

function inFrontmatter(state: EditorState, pos: number): boolean {
  const region = frontmatterRegion(state.doc);
  return region !== null && pos <= region.to;
}

function onIdLine(state: EditorState, pos: number): boolean {
  return isIdLine(state.doc.lineAt(pos).text);
}

/** Whether `from`..`to` touches code, a comment or math. */
function touchesNotProse(
  state: EditorState,
  from: number,
  to: number,
): boolean {
  const tree = syntaxTree(state);
  for (const pos of [from, to]) {
    for (const side of [-1, 1] as const) {
      for (
        let node: SyntaxNode | null = tree.resolveInner(pos, side);
        node;
        node = node.parent
      ) {
        if (NOT_PROSE.has(node.name) && node.from < pos && node.to > pos) {
          return true;
        }
      }
    }
  }
  let found = false;
  tree.iterate({
    from,
    to,
    enter: (node) => {
      if (found) return false;
      if (NOT_PROSE.has(node.name) && node.from < to && node.to > from) {
        found = true;
        return false;
      }
      return true;
    },
  });
  return found;
}

/** Whether the popovers may read or write anything at `pos`. */
function editableAt(state: EditorState, pos: number): boolean {
  return !inFrontmatter(state, pos) && !onIdLine(state, pos);
}

function char(state: EditorState, pos: number): string {
  return state.doc.sliceString(pos, pos + 1);
}

/** The `Link` node around `pos`, read as a link, or null. */
function readLink(state: EditorState, node: SyntaxNode): LinkElement | null {
  const open = node.firstChild;
  if (!open || open.name !== "LinkMark" || char(state, open.from) !== "[") {
    return null;
  }
  let close: SyntaxNode | null = open.nextSibling;
  while (
    close &&
    !(close.name === "LinkMark" && char(state, close.from) === "]")
  ) {
    close = close.nextSibling;
  }
  if (!close) {
    return null;
  }
  const text = { from: open.to, to: close.from };
  const words = state.doc.sliceString(text.from, text.to);
  // A footnote reference is not a link the popover edits.
  if (words.startsWith("^")) {
    return null;
  }
  // `[[Target]]`: the wikilink's, never a Markdown link.
  if (char(state, node.from - 1) === "[" && char(state, node.to) === "]") {
    return null;
  }
  const after = close.nextSibling;
  if (after?.name === "LinkMark" && char(state, after.from) === "(") {
    const url = node.getChild("URL");
    return {
      from: node.from,
      to: node.to,
      open: { from: open.from, to: open.to },
      close: { from: close.from, to: node.to },
      text,
      address: url
        ? { from: url.from, to: url.to }
        : { from: after.to, to: after.to },
      reference: false,
      label: "",
    };
  }
  let label = words;
  let end = close.to;
  if (after?.name === "LinkLabel") {
    end = after.to;
    const written = state.doc.sliceString(after.from + 1, after.to - 1);
    if (written.trim() !== "") {
      label = written;
    }
  }
  if (linkDefinition(state, label) === undefined) {
    return null;
  }
  return {
    from: node.from,
    to: end,
    open: { from: open.from, to: open.to },
    close: { from: close.from, to: end },
    text,
    address: null,
    reference: true,
    label,
  };
}

/** The Markdown link at `pos` (inclusive of both ends), or null. */
export function linkAt(state: EditorState, pos: number): LinkElement | null {
  if (pos < 0 || pos > state.doc.length || !editableAt(state, pos)) {
    return null;
  }
  const tree = syntaxTree(state);
  for (const side of [1, -1] as const) {
    for (
      let node: SyntaxNode | null = tree.resolveInner(pos, side);
      node;
      node = node.parent
    ) {
      if (node.name === "Image") {
        return null;
      }
      if (node.name === "Link") {
        const link = readLink(state, node);
        if (link && link.from <= pos && pos <= link.to) {
          return link;
        }
        break;
      }
    }
  }
  return null;
}

/** An address as written, without the angle brackets that may wrap it. */
function unwrap(written: string): string {
  return written.startsWith("<") && written.endsWith(">")
    ? written.slice(1, -1)
    : written;
}

/**
 * A backslash before ASCII punctuation, or a character reference, as
 * CommonMark reads them: in one pass, so an escaped `&` starts no reference.
 */
const ESCAPE_OR_REFERENCE =
  /\\([!-/:-@[-`{-~])|&(#[0-9]{1,7}|#[xX][0-9a-fA-F]{1,6}|[A-Za-z][A-Za-z0-9]{0,31});/g;

/** The character a numeric reference names, as CommonMark decodes it. */
function numeric(code: number): string {
  return code === 0 || code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff)
    ? "\uFFFD"
    : String.fromCodePoint(code);
}

/**
 * A textarea in a document of its own, made once: inert (no scripts, no
 * loads, no rendering), and its contents are read as text, never markup. The
 * live page never sees what an address holds.
 */
let decoder: HTMLTextAreaElement | null = null;

/** The decoder's textarea, made on first use; exported for its test. */
export function referenceDecoder(): HTMLTextAreaElement {
  decoder ??= document.implementation
    .createHTMLDocument("")
    .createElement("textarea");
  return decoder;
}

/**
 * The character a named reference stands for, or null for a name HTML does
 * not know. Read through the browser's own HTML parser in an inert document,
 * so the editor carries no table of names; a legacy name a parser matches by
 * prefix (`&ampfoo;`) is not a reference and stays as written.
 */
function named(reference: string): string | null {
  const area = referenceDecoder();
  area.innerHTML = reference;
  const value = area.value;
  return value !== reference && value.length <= 2 ? value : null;
}

/**
 * Text as a reader sees it: every backslash escape taken away and every
 * character reference decoded, as the reading page renders it.
 */
function unescaped(written: string): string {
  return written.replace(
    ESCAPE_OR_REFERENCE,
    (found, escaped: string | undefined, reference: string | undefined) => {
      if (escaped !== undefined) return escaped;
      if (reference === undefined) return found;
      if (reference.startsWith("#x") || reference.startsWith("#X")) {
        return numeric(parseInt(reference.slice(2), 16));
      }
      if (reference.startsWith("#")) {
        return numeric(parseInt(reference.slice(1), 10));
      }
      return named(found) ?? found;
    },
  );
}

/** An `&` that would start a character reference, which a reader decodes. */
const REFERENCE = /&(?=#?[A-Za-z0-9]+;)/g;

/** The words of the link at `pos` as a reader sees them, or null. */
export function linkWords(state: EditorState, pos: number): string | null {
  const link = linkAt(state, pos);
  return link
    ? unescaped(state.doc.sliceString(link.text.from, link.text.to))
    : null;
}

/**
 * The address of the link at `pos` as a reader follows it: an inline link's
 * own, a reference link's definition's. Empty with none; null with no link.
 */
export function linkAddress(state: EditorState, pos: number): string | null {
  const link = linkAt(state, pos);
  if (!link) {
    return null;
  }
  const span = link.reference
    ? (linkDefinition(state, link.label) ?? null)
    : link.address;
  return span
    ? unescaped(unwrap(state.doc.sliceString(span.from, span.to)))
    : "";
}

/**
 * An address as it is written into `(...)`, so that a reader follows exactly
 * what was typed: every backslash doubled and an `&` that would start a
 * character reference escaped; without a space its parentheses and a leading
 * `<` escaped too, and with one inside angle brackets. Null when it cannot be
 * written at all: empty, a line break, or a space beside an angle bracket
 * (the parser reads no escaped angle bracket inside angle brackets).
 */
export function writtenAddress(address: string): string | null {
  const trimmed = address.trim();
  if (trimmed === "" || /[\r\n]/.test(trimmed)) {
    return null;
  }
  const escaped = trimmed.replace(/\\/g, "\\\\").replace(REFERENCE, "\\&");
  if (!/\s/.test(escaped)) {
    return escaped.replace(/[()]|^</g, (found) => `\\${found}`);
  }
  return /[<>]/.test(escaped) ? null : `<${escaped}>`;
}

/**
 * The words of a new link as written, so that a reader sees exactly what was
 * typed: a backslash, brackets, emphasis, code, strikethrough and angle
 * brackets escaped, an `&` that would start a character reference, and a `^`
 * at the start (which would make the link a footnote reference).
 */
function writtenText(text: string): string {
  return text
    .replace(/[\\[\]*_`~<>]/g, (found) => `\\${found}`)
    .replace(REFERENCE, "\\&")
    .replace(/^\^/, "\\^");
}

/** Gives the link at `at` the address `address`; only the address span is written. */
export function linkChanges(
  state: EditorState,
  at: number,
  { address }: { address: string },
): ChangeSpec[] | null {
  const link = linkAt(state, at);
  if (!link || link.reference || !link.address) {
    return null;
  }
  const written = writtenAddress(address);
  if (written === null) {
    return null;
  }
  const { from, to } = link.address;
  if (state.doc.sliceString(from, to) === written) {
    return [];
  }
  return [{ from, to, insert: written }];
}

/**
 * Whether a new link may wrap `from`..`to`: one line of prose, outside the
 * frontmatter, an id line, code, comments and math, inside no link, and not
 * splitting a pair of marks (a link's brackets around half of a bold word).
 */
function wrappable(state: EditorState, from: number, to: number): boolean {
  if (from > to || from < 0 || to > state.doc.length) {
    return false;
  }
  const line = state.doc.lineAt(from);
  if (to > line.to) {
    return false;
  }
  if (!editableAt(state, from) || touchesNotProse(state, from, to)) {
    return false;
  }
  if (linkAt(state, from) || linkAt(state, to)) {
    return false;
  }
  const marks = marksOnLine(state, line.number, { always: true });
  return marks.every((mark) => {
    if (mark.kind === "link" || mark.kind === "comment") {
      return mark.to <= from || mark.from >= to;
    }
    // A mark the range cuts through, or one of a pair inside without its partner.
    if (mark.from < from && mark.to > from) return false;
    if (mark.from < to && mark.to > to) return false;
    const partner = mark.partner === null ? null : marks[mark.partner];
    if (!partner) return true;
    const inside = (m: { from: number; to: number }) =>
      m.from >= from && m.to <= to;
    return inside(mark) === inside(partner);
  });
}

/**
 * A new link: `[` before and `](address)` after a selection, or
 * `[text](address)` at a caret (the address is the text when none is typed).
 */
export function newLinkChanges(
  state: EditorState,
  range: Span,
  { text, address }: { text: string; address: string },
): ChangeSpec[] | null {
  const written = writtenAddress(address);
  if (written === null || !wrappable(state, range.from, range.to)) {
    return null;
  }
  if (range.from === range.to) {
    const words = text === "" ? address.trim() : text;
    return [
      {
        from: range.from,
        insert: `[${writtenText(words)}](${written})`,
      },
    ];
  }
  return [
    { from: range.from, insert: "[" },
    { from: range.to, insert: `](${written})` },
  ];
}

/** Takes the link at `at` away and keeps its words: two deletes, nothing else. */
export function unlinkChanges(
  state: EditorState,
  at: number,
): ChangeSpec[] | null {
  const link = linkAt(state, at);
  if (!link) {
    return null;
  }
  return [
    { from: link.open.from, to: link.open.to },
    { from: link.close.from, to: link.close.to },
  ];
}

/** The pattern of one wikilink, as the chips read it. */
const WIKILINK_ONE = /\[\[([^[\]]+)\]\]/g;

/** The `[[...]]` at `pos` (inclusive of both ends), or null. */
export function wikilinkAt(
  state: EditorState,
  pos: number,
): WikilinkElement | null {
  if (pos < 0 || pos > state.doc.length || !editableAt(state, pos)) {
    return null;
  }
  const line = state.doc.lineAt(pos);
  // Two wikilinks side by side share a position: the one starting there wins
  // over the one ending there, as `linkAt` reads links.
  let found: RegExpExecArray | null = null;
  for (const match of line.text.matchAll(WIKILINK_ONE)) {
    const from = line.from + match.index;
    const to = from + match[0].length;
    if (pos >= from && pos < to) {
      found = match;
      break;
    }
    if (pos === to) {
      found = match;
    }
  }
  if (found) {
    const match = found;
    const from = line.from + match.index;
    const to = from + match[0].length;
    if (touchesNotProse(state, from, to)) {
      return null;
    }
    const inner = match[1] ?? "";
    const bar = inner.indexOf("|");
    const start = from + 2;
    const end = to - 2;
    return bar < 0
      ? { from, to, target: { from: start, to: end }, alias: null }
      : {
          from,
          to,
          target: { from: start, to: start + bar },
          alias: { from: start + bar + 1, to: end },
        };
  }
  return null;
}

/** Gives the wikilink at `at` the target `target`; its alias is kept. */
export function wikilinkChanges(
  state: EditorState,
  at: number,
  { target }: { target: string },
): ChangeSpec[] | null {
  const link = wikilinkAt(state, at);
  const written = target.trim();
  if (!link || written === "" || /[[\]|\r\n]/.test(written)) {
    return null;
  }
  const { from, to } = link.target;
  if (state.doc.sliceString(from, to) === written) {
    return [];
  }
  return [{ from, to, insert: written }];
}

/**
 * Takes the wikilink at `at` away: its brackets go, and with an alias the
 * `Target|` before it, so the words a reader saw stay.
 */
export function unwikilinkChanges(
  state: EditorState,
  at: number,
): ChangeSpec[] | null {
  const link = wikilinkAt(state, at);
  if (!link) {
    return null;
  }
  return [
    { from: link.from, to: link.alias ? link.alias.from : link.from + 2 },
    { from: link.to - 2, to: link.to },
  ];
}

/**
 * The address a link may open in a new tab: a web or mail address. Anything
 * else (a `javascript:` string a peer wrote, a relative path, a
 * `crystalline://` name) opens nothing.
 */
export function openableAddress(address: string): string | null {
  const trimmed = address.trim();
  return isWebAddress(trimmed) || trimmed.startsWith("mailto:")
    ? trimmed
    : null;
}
