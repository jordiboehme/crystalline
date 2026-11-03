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
import type { StatusColour } from "../markdown/directives";
import { isIdLine } from "../reflectionText";
import { STATUS_OPEN } from "./dialectHighlight";
import { calloutAt, statusAround, statusColourChanges } from "./elementContext";
import { fenceBody, fenceInfo } from "./fenceInfo";
import { frontmatterRegion } from "./frontmatterRegion";
import {
  SECTION_HEADING,
  escapedAt,
  mathBlocks,
  mathFence,
  mathSections,
  quotePrefix,
  unpadded,
  unquoted,
} from "./mathBlocks";
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

/** A status label as the popover reads it. */
export interface StatusElement extends Span {
  /** The words between `:status[` and their closing `]`. */
  words: Span;
  colour: StatusColour;
}

/** The status label at `pos` (inclusive of both ends), or null. */
export function statusAt(
  state: EditorState,
  pos: number,
): StatusElement | null {
  if (pos < 0 || pos > state.doc.length || !editableAt(state, pos)) {
    return null;
  }
  const status = statusAround(state, pos);
  if (status === null) {
    return null;
  }
  return {
    from: status.from,
    to: status.to,
    words: { from: status.from + STATUS_OPEN.length, to: status.closeFrom },
    colour: status.colour,
  };
}

/**
 * A label's words as the field shows them, the inverse of `writtenLabel`.
 * Read run by run: a run of backslashes before a bracket or the end was
 * doubled by the writer (one more escapes the bracket), so it is halved; a
 * run before any other character is shown as written, so the words' own
 * Markdown (`\*`, `\\*`) is kept byte for byte.
 */
export function statusWords(state: EditorState, pos: number): string | null {
  const status = statusAt(state, pos);
  if (status === null) {
    return null;
  }
  const source = state.doc.sliceString(status.words.from, status.words.to);
  let out = "";
  let at = 0;
  while (at < source.length) {
    let run = 0;
    while (source[at + run] === "\\") run += 1;
    const next = source[at + run];
    if (run === 0) {
      out += next ?? "";
      at += 1;
    } else if (next === undefined || next === "[" || next === "]") {
      out += "\\".repeat(Math.floor(run / 2)) + (next ?? "");
      at += run + 1;
    } else {
      out += "\\".repeat(run);
      at += run;
    }
  }
  return out;
}

/**
 * Words as they are written between `:status[` and `]`, so that the label
 * closes where the reader closes it (`labelEnd`) and `statusWords` reads them
 * back as typed:
 *
 * - a bracket with no partner is escaped; a pair stays, so a link inside the
 *   words keeps working;
 * - a run of backslashes before a bracket or the end is doubled;
 * - a run before any other character is written as typed.
 *
 * Null for words that cannot be a label: nothing but spaces, or a line break.
 */
export function writtenLabel(words: string): string | null {
  if (words.trim() === "" || /[\r\n]/.test(words)) {
    return null;
  }
  const chars = [...words];
  const unpaired = new Set<number>();
  const open: number[] = [];
  chars.forEach((c, index) => {
    if (c === "[") {
      open.push(index);
    } else if (c === "]") {
      if (open.length > 0) {
        open.pop();
      } else {
        unpaired.add(index);
      }
    }
  });
  for (const index of open) {
    unpaired.add(index);
  }
  let out = "";
  let at = 0;
  while (at < chars.length) {
    let run = 0;
    while (chars[at + run] === "\\") run += 1;
    const next = chars[at + run];
    const bracket = next === "[" || next === "]";
    out += "\\".repeat(next === undefined || bracket ? run * 2 : run);
    at += run;
    if (next !== undefined) {
      out += bracket && unpaired.has(at) ? `\\${next}` : next;
      at += 1;
    }
  }
  return out;
}

/**
 * Gives the label at `at` new words, a new colour, or both, in one change
 * list. A field left out (or equal to what the document says) is not
 * written, so a peer's change to it stays. Null when there is no label at
 * `at` or the words cannot be written.
 */
export function statusChanges(
  state: EditorState,
  at: number,
  { words, colour }: { words?: string; colour?: StatusColour },
): ChangeSpec[] | null {
  const status = statusAt(state, at);
  if (status === null) {
    return null;
  }
  const changes: ChangeSpec[] = [];
  if (words !== undefined && words !== statusWords(state, at)) {
    const written = writtenLabel(words);
    if (written === null) {
      return null;
    }
    changes.push({ ...status.words, insert: written });
  } else if (words !== undefined && writtenLabel(words) === null) {
    return null;
  }
  if (colour !== undefined && colour !== status.colour) {
    const coloured = statusColourChanges(state, status.from, colour);
    if (coloured === null) {
      return null;
    }
    changes.push(...coloured);
  }
  return changes;
}

/** How a formula is written. */
export type FormulaForm =
  /** `` $`...`$ ``: inline code between two dollar signs. */
  | "code"
  /** `$$...$$` on one line. */
  | "inline"
  /** A `$$` block. */
  | "dollars"
  /** A `math` fence. */
  | "fence";

/** A formula as the popover reads it. */
export interface MathElement extends Span {
  form: FormulaForm;
  block: boolean;
  /**
   * The formula as the page reads it: an inline one without the one space
   * of padding at each end, a block's lines without what contains them.
   */
  formula: string;
  /** Inline: the text between the delimiters. A block: its inner lines, or none. */
  body: Span | null;
  /** Inline: the two delimiter runs (backticks or dollar signs). */
  delimiters: { open: Span; close: Span } | null;
  /** A block's line where an empty body is written after. */
  openLine: Span;
  /**
   * What stands before a block's delimiter on its opening line (quote marks,
   * a list item's indentation), which every line of the block carries.
   */
  prefix: string;
}

/** The inline formula `node` is, or null. */
function inlineFormula(
  state: EditorState,
  node: SyntaxNode,
): MathElement | null {
  const doc = state.doc;
  const open = node.firstChild;
  const close = node.lastChild;
  if (open === null || close === null || open === close) {
    return null;
  }
  let from = node.from;
  let to = node.to;
  let form: FormulaForm = "inline";
  if (node.name === "InlineCode") {
    // The form the preview and the reader set: `$` on both sides, the first
    // one not escaped, with any run of backticks.
    if (
      doc.sliceString(node.from - 1, node.from) !== "$" ||
      escapedAt(doc, node.from - 1) ||
      doc.sliceString(node.to, node.to + 1) !== "$"
    ) {
      return null;
    }
    from = node.from - 1;
    to = node.to + 1;
    form = "code";
  } else if (node.name !== "InlineMath") {
    return null;
  }
  const body = { from: open.to, to: close.from };
  return {
    from,
    to,
    form,
    block: false,
    formula: unpadded(doc.sliceString(body.from, body.to)),
    body,
    delimiters: {
      open: { from: open.from, to: open.to },
      close: { from: close.from, to: close.to },
    },
    openLine: body,
    prefix: "",
  };
}

/** `line` without the container `prefix` of its block, read as the page reads it. */
function inBlock(line: string, prefix: string, depth: number): string {
  if (line.startsWith(prefix)) {
    return line.slice(prefix.length);
  }
  // An empty line of the block, with its container's marks only in part.
  if (prefix.startsWith(line) || prefix.trimEnd() === line.trimEnd()) {
    return "";
  }
  return unquoted(line, depth);
}

/** The display formula whose lines hold `pos`, or null. */
function blockFormula(state: EditorState, pos: number): MathElement | null {
  const doc = state.doc;
  const block = mathBlocks(state).find(
    (found) => found.from <= pos && pos <= found.to,
  );
  if (block === undefined) {
    return null;
  }
  for (let number = block.open; number <= block.close; number += 1) {
    if (isIdLine(doc.line(number).text)) {
      return null;
    }
  }
  if (touchesNotProse(state, block.from, block.from)) {
    return null;
  }
  const openLine = doc.line(block.open);
  const mark =
    block.form === "fence"
      ? (/[`~]{3,}/.exec(openLine.text)?.index ?? -1)
      : openLine.text.indexOf("$$");
  const prefix =
    mark >= 0
      ? openLine.text.slice(0, mark)
      : quotePrefix(openLine.text, block.depth);
  const lines: string[] = [];
  for (let number = block.open + 1; number < block.close; number += 1) {
    lines.push(inBlock(doc.line(number).text, prefix, block.depth));
  }
  const body =
    block.close - block.open > 1
      ? {
          from: doc.line(block.open + 1).from,
          to: doc.line(block.close - 1).to,
        }
      : null;
  return {
    from: block.from,
    to: block.to,
    form: block.form,
    block: true,
    formula: lines.join("\n"),
    body,
    delimiters: null,
    openLine: { from: openLine.from, to: openLine.to },
    prefix,
  };
}

/**
 * The formula at `pos` (inclusive of both ends), or null: inline first (the
 * one starting there wins over the one ending there), then a block.
 */
export function mathAt(state: EditorState, pos: number): MathElement | null {
  if (pos < 0 || pos > state.doc.length || !editableAt(state, pos)) {
    return null;
  }
  const tree = syntaxTree(state);
  // `` $`x`$ ``: the dollar sign outside the code span is a position away.
  for (const [at, side] of [
    [pos, 1],
    [pos + 1, 1],
    [pos, -1],
    [pos - 1, -1],
  ] as const) {
    if (at < 0 || at > state.doc.length) continue;
    for (
      let node: SyntaxNode | null = tree.resolveInner(at, side);
      node;
      node = node.parent
    ) {
      if (node.name === "InlineMath" || node.name === "InlineCode") {
        const math = inlineFormula(state, node);
        if (math !== null && math.from <= pos && pos <= math.to) {
          return math;
        }
        break;
      }
    }
  }
  return blockFormula(state, pos);
}

/** Why a formula cannot be written in `form`, or null when it can. */
export type FormulaRefusal =
  "empty" | "line-break" | "dollar-line" | "fence-line" | "heading-line";

/**
 * Why `formula` cannot be written in `form` so that it reads back as typed,
 * or null. An inline formula always can (its delimiters grow and it is
 * padded as needed, see `mathChanges`) unless it holds a line break. A block
 * formula that holds its own delimiter would close early: a line of only
 * `$$` in a `$$` block, a line of backticks in a fence, and on a page read
 * section by section a `##` line outside a quote.
 */
export function formulaRefusal(
  form: FormulaForm,
  formula: string,
  sections = false,
  quoted = false,
): FormulaRefusal | null {
  if (formula.trim() === "") {
    return "empty";
  }
  if (form === "code" || form === "inline") {
    return /[\r\n]/.test(formula) ? "line-break" : null;
  }
  const lines = formula.split(/\r?\n/);
  if (form === "fence") {
    // A line of backticks or tildes would close the fence (either kind).
    if (lines.some((line) => /^ {0,3}(?:`{3,}|~{3,})[ \t]*$/.test(line))) {
      return "fence-line";
    }
  } else if (lines.some((line) => line.trim() === "$$")) {
    return "dollar-line";
  }
  // A section heading ends a block only outside a quote: a line written
  // into a quote carries its quote marks.
  return sections && !quoted && lines.some((line) => SECTION_HEADING.test(line))
    ? "heading-line"
    : null;
}

/**
 * The shortest run of `sign`, at least `least` long, that `formula` holds no
 * run of exactly that length of: a delimiter that cannot close early.
 */
function delimiterRun(formula: string, sign: string, least: number): number {
  const runs = new Set(
    [...formula.matchAll(new RegExp(`\\${sign}+`, "g"))].map(
      (found) => found[0].length,
    ),
  );
  let run = least;
  while (runs.has(run)) run += 1;
  return run;
}

/**
 * Gives the formula at `at` the text `formula`. Only the text between its
 * delimiters is written, so that the page reads exactly `formula`:
 *
 * - inline, with one space of padding at each end when the page would take
 *   one away or a delimiter sign would touch it, and with longer delimiters
 *   (never shorter ones) when the formula holds a run of the current length;
 * - a block, its inner lines, each with what stands before the block's
 *   opening delimiter (quote marks, a list item's indentation).
 *
 * Nothing for an unchanged formula; null when there is none at `at` or
 * `formulaRefusal` refuses it.
 */
export function mathChanges(
  state: EditorState,
  at: number,
  { formula }: { formula: string },
): ChangeSpec[] | null {
  const math = mathAt(state, at);
  if (
    math === null ||
    formulaRefusal(
      math.form,
      formula,
      state.facet(mathSections),
      math.prefix !== "",
    ) !== null
  ) {
    return null;
  }
  if (formula === math.formula) {
    return [];
  }
  if (math.delimiters !== null && math.body !== null) {
    const sign = math.form === "code" ? "`" : "$";
    const { open, close } = math.delimiters;
    const run = open.to - open.from;
    const padded =
      (formula.startsWith(" ") && formula.endsWith(" ")) ||
      formula.startsWith(sign) ||
      formula.endsWith(sign)
        ? ` ${formula} `
        : formula;
    const needed = delimiterRun(formula, sign, run);
    if (needed === run) {
      return [{ ...math.body, insert: padded }];
    }
    const mark = sign.repeat(needed);
    return [
      { from: open.from, to: close.to, insert: `${mark}${padded}${mark}` },
    ];
  }
  const lines = formula
    .split(/\r?\n/)
    .map((line) => `${math.prefix}${line}`)
    .join("\n");
  return math.body !== null
    ? [{ ...math.body, insert: lines }]
    : [{ from: math.openLine.to, insert: `\n${lines}` }];
}

/**
 * Takes the formula at `at` away whole: one deletion. A block goes with its
 * lines and one line break, so no empty line is left behind (in a quote or a
 * callout an empty line would end it).
 */
export function removeMathChanges(
  state: EditorState,
  at: number,
): ChangeSpec[] | null {
  const math = mathAt(state, at);
  if (math === null) {
    return null;
  }
  if (!math.block) {
    return [{ from: math.from, to: math.to }];
  }
  if (math.to < state.doc.length) {
    return [{ from: math.from, to: math.to + 1 }];
  }
  return [{ from: Math.max(0, math.from - 1), to: math.to }];
}

/** The task box (`[ ]`, `[x]`, `[~]`) of the line `pos` is on, or null. */
export function taskAt(state: EditorState, pos: number): Span | null {
  if (pos < 0 || pos > state.doc.length || !editableAt(state, pos)) {
    return null;
  }
  const line = state.doc.lineAt(pos);
  const mark = marksOnLine(state, line.number, { always: true }).find(
    (found) => found.kind === "task",
  );
  // The mark holds the space after the box too; the box is three characters.
  return mark ? { from: mark.from, to: mark.from + 3 } : null;
}

/** The callout marker `[!KIND]` (without its sign) of the callout around `pos`, or null. */
export function calloutMarkAt(state: EditorState, pos: number): Span | null {
  if (pos < 0 || pos > state.doc.length || !editableAt(state, pos)) {
    return null;
  }
  const callout = calloutAt(state, pos);
  return callout === null
    ? null
    : { from: callout.markFrom, to: callout.kindTo + 1 };
}

/**
 * A closed fence the code box draws: its whole span (the opening line's start
 * to the closing line's end), its two fence lines and the info word.
 */
export interface CodeFence extends Span {
  /** The document lines of the opening and the closing fence. */
  open: number;
  close: number;
  /** The opening fence's backticks or tildes. */
  marks: Span;
  /**
   * The info word, read as CommonMark and the editor's parser read it: after
   * any spaces that follow the marks, up to the next space or the line end.
   * Empty (at the marks' end) when there is none.
   */
  word: Span;
  /** Whether more info text follows the word on the opening line. */
  more: boolean;
  /** A tilde fence, whose info may hold a backtick. */
  tilde: boolean;
}

/**
 * The source of a mermaid fence the editor draws as a diagram
 * (`fencePreviews.ts`), or null: a closed fence whose info word is
 * `mermaid` (read as the page reads it, `fenceInfo`), with something in it.
 * The reading page draws exactly these. The one test for both layers.
 */
export function mermaidFence(
  state: EditorState,
  node: SyntaxNode,
): string | null {
  const info = fenceInfo(state.doc, node);
  if (info?.name !== "mermaid" || info.close === null) {
    return null;
  }
  const body = fenceBody(state.doc, info);
  return body.trim() === "" ? null : body;
}

/**
 * Whether another layer draws the fence `node`: a mermaid diagram, or a
 * formula with something in it. The code box leaves exactly those.
 */
function drawnElsewhere(state: EditorState, node: SyntaxNode): boolean {
  if (mermaidFence(state, node) !== null) {
    return true;
  }
  const math = mathFence(state, node);
  return math !== null && math.tex.trim() !== "";
}

/** Whether `text` before position `upTo` holds only indentation. */
function onlyIndent(text: string, upTo: number): boolean {
  return /^[ \t]*$/.test(text.slice(0, upTo));
}

/**
 * The code box of `node`, a `FencedCode`, or null when it draws none: an
 * unclosed fence, one with no code line, one in a quote or after a list
 * marker, one in the frontmatter, on an id line, or one another layer
 * draws (a mermaid diagram, a formula).
 */
export function codeFenceOf(
  state: EditorState,
  node: SyntaxNode,
): CodeFence | null {
  const doc = state.doc;
  const region = frontmatterRegion(doc);
  if (region !== null && node.from <= region.to) {
    return null;
  }
  const first = node.firstChild;
  const last = node.lastChild;
  if (first?.name !== "CodeMark" || last?.name !== "CodeMark") {
    return null;
  }
  const openLine = doc.lineAt(first.from);
  const closeLine = doc.lineAt(last.from);
  // Closed, with at least one code line between the fences.
  if (closeLine.number < openLine.number + 2) {
    return null;
  }
  if (
    !onlyIndent(openLine.text, first.from - openLine.from) ||
    !onlyIndent(closeLine.text, last.from - closeLine.from) ||
    closeLine.text.slice(last.to - closeLine.from).trim() !== "" ||
    isIdLine(openLine.text) ||
    isIdLine(closeLine.text)
  ) {
    return null;
  }
  if (drawnElsewhere(state, node)) {
    return null;
  }
  const info = fenceInfo(doc, node);
  if (info === null) {
    return null;
  }
  return {
    from: openLine.from,
    to: closeLine.to,
    open: openLine.number,
    close: closeLine.number,
    marks: info.marks,
    word: info.word,
    more: info.more,
    tilde: doc.sliceString(first.from, first.from + 1) === "~",
  };
}

/** The code box `pos` lies in (a fence line or a code line), or null. */
export function codeFenceAt(state: EditorState, pos: number): CodeFence | null {
  if (pos < 0 || pos > state.doc.length) {
    return null;
  }
  const tree = syntaxTree(state);
  // In a fence line's indentation (a list item's, up to three spaces) the
  // fence starts at its marks, past the indent.
  const line = state.doc.lineAt(pos);
  const indent = /^[ \t]*/.exec(line.text)?.[0].length ?? 0;
  const at = pos < line.from + indent ? line.from + indent : pos;
  for (const side of [1, -1] as const) {
    for (
      let node: SyntaxNode | null = tree.resolve(at, side);
      node;
      node = node.parent
    ) {
      if (node.name === "FencedCode") {
        const fence = codeFenceOf(state, node);
        if (fence !== null && fence.from <= pos && pos <= fence.to) {
          return fence;
        }
        break;
      }
    }
  }
  return null;
}

export { languageOf } from "./codeLanguage";

/**
 * Writes `word` as the info word of the code box starting at `at` (`null`
 * takes the word away, which is "Plain text"). The word goes right after the
 * fence marks, in place of the old word and the spaces before it; any
 * further info text stays, and the fence keeps its backticks or tildes.
 * Nothing is written when the word is already that text. Null when there is
 * no code box starting at `at`, when `word` cannot be an info word (empty,
 * with a space, or with a backtick in a backtick fence), or for Plain text
 * when more info follows the word: CommonMark would read that text as the
 * language (`CodeFence.more`).
 */
export function languageChanges(
  state: EditorState,
  at: number,
  word: string | null,
): ChangeSpec[] | null {
  const fence = codeFenceAt(state, at);
  if (fence?.from !== at) {
    return null;
  }
  if (
    word !== null &&
    (word === "" || /\s/.test(word) || (!fence.tilde && word.includes("`")))
  ) {
    return null;
  }
  if (word === null && fence.more) {
    return null;
  }
  if (state.doc.sliceString(fence.word.from, fence.word.to) === (word ?? "")) {
    return [];
  }
  return [{ from: fence.marks.to, to: fence.word.to, insert: word ?? "" }];
}

/**
 * One line break after the closing fence of the code box starting at `at`:
 * an empty line below it, the way out of a box that ends the document.
 */
export function paragraphBelowChanges(
  state: EditorState,
  at: number,
): ChangeSpec[] | null {
  const fence = codeFenceAt(state, at);
  return fence?.from === at ? [{ from: fence.to, insert: "\n" }] : null;
}
