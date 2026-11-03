/**
 * Which bullets the engram parser reads as a relation, and where their type
 * stands, ported from `crates/core/src/parse.rs` (`scan_body`, `bullet_end`,
 * `parse_relation`, `wiki_relation`, `relative_relation`, `relation_type`,
 * `relative_relation_type`, `relative_note_links`) so a chip is drawn only on
 * a type the server reads. A bullet is its line and the lines that wrap it;
 * the first `[[...]]` (or the first relative link to a note) is the target,
 * and the words before it are the type.
 */

/** The span of a relation's type in its bullet's first line. */
export interface TypeSpan {
  from: number;
  to: number;
}

const INDEX_FILE = "index.md";
const LOG_FILE = "log.md";

function reserved(path: string): boolean {
  const name = path.split("/").pop() ?? "";
  return name === INDEX_FILE || name === LOG_FILE;
}

/** `split_category`: the leading `[category]` of a bullet's content. */
function readsAsCategory(content: string): boolean {
  if (!content.startsWith("[")) {
    return false;
  }
  const rest = content.slice(1);
  const close = rest.indexOf("]");
  if (close < 0) {
    return false;
  }
  const category = rest.slice(0, close);
  if (category === "" || category.includes("[") || category.startsWith("^")) {
    return false;
  }
  if (category === " " || category === "x" || category === "X") {
    return false;
  }
  if (category === "~") {
    return false;
  }
  return !/^[([]/.test(rest.slice(close + 1));
}

/** Inline code spans of one line as `[start, end)` ranges, as `code_spans`. */
function codeSpans(line: string): [number, number][] {
  const spans: [number, number][] = [];
  let i = 0;
  while (i < line.length) {
    if (line[i] !== "`") {
      i += 1;
      continue;
    }
    let runEnd = i;
    while (line[runEnd] === "`") {
      runEnd += 1;
    }
    const run = runEnd - i;
    let k = runEnd;
    let closed = -1;
    while (k < line.length) {
      if (line[k] === "`") {
        let m = k;
        while (line[m] === "`") {
          m += 1;
        }
        if (m - k === run) {
          closed = m;
          break;
        }
        k = m;
      } else {
        k += 1;
      }
    }
    if (closed >= 0) {
      spans.push([i, closed]);
      i = closed;
    } else {
      i = runEnd;
    }
  }
  return spans;
}

function maskInlineCode(line: string): string {
  if (!line.includes("`")) {
    return line;
  }
  const chars = [...line];
  for (const [from, to] of codeSpans(line)) {
    for (let at = from; at < to; at += 1) {
      chars[at] = " ";
    }
  }
  return chars.join("");
}

function linkTextStart(line: string, close: number): number | null {
  let depth = 0;
  for (let i = close - 1; i >= 0; i -= 1) {
    const escaped = i > 0 && line[i - 1] === "\\";
    if (line[i] === "]" && !escaped) {
      depth += 1;
    } else if (line[i] === "[" && !escaped) {
      if (depth === 0) {
        return i;
      }
      depth -= 1;
    }
  }
  return null;
}

function closingParen(line: string, from: number): number | null {
  let depth = 1;
  for (let i = from; i < line.length; i += 1) {
    const char = line[i];
    if (char === "\\" && /[()\\]/.test(line[i + 1] ?? "")) {
      i += 1;
    } else if (char === "(") {
      depth += 1;
    } else if (char === ")") {
      depth -= 1;
      if (depth === 0) {
        return i;
      }
    }
  }
  return null;
}

/** `read_destination`: the decoded path of the link destination at `open`. */
function readDestination(
  line: string,
  open: number,
): { path: string; close: number } | null {
  let target: string;
  let close: number | null;
  if (line[open] === "<") {
    const gt = line.indexOf(">", open + 1);
    if (gt < 0) {
      return null;
    }
    close = closingParen(line, gt + 1);
    target = line.slice(open + 1, gt);
  } else {
    close = closingParen(line, open);
    if (close === null) {
      return null;
    }
    const inside = line.slice(open, close);
    let end = -1;
    for (let at = 0; at < inside.length && end < 0; at += 1) {
      const code = inside.charCodeAt(at);
      if (code === 32 || code < 32 || code === 127) {
        end = at;
      }
    }
    target = end < 0 ? inside : inside.slice(0, end);
  }
  if (close === null) {
    return null;
  }
  const hash = target.indexOf("#");
  let path = hash < 0 ? target : target.slice(0, hash);
  path = path.replace(/\\([!-/:-@[-`{-~])/g, "$1");
  try {
    path = decodeURIComponent(path);
  } catch {
    // A malformed escape is kept as written.
  }
  return { path, close };
}

/** `mask_link_destinations` applied to a line with inline code already masked. */
function maskLinkDestinations(line: string): string {
  const ranges: [number, number][] = [];
  let cursor = 0;
  for (;;) {
    const found = line.indexOf("](", cursor);
    if (found < 0) {
      break;
    }
    const open = found + 2;
    cursor = open;
    if (linkTextStart(line, found) === null) {
      continue;
    }
    const dest = readDestination(line, open);
    if (!dest) {
      continue;
    }
    ranges.push([open, dest.close]);
    cursor = dest.close;
  }
  if (ranges.length === 0) {
    return line;
  }
  return [...line]
    .map((char, at) =>
      ranges.some(([from, to]) => at >= from && at < to) ? " " : char,
    )
    .join("");
}

function maskForWikilinks(line: string): string {
  const code = maskInlineCode(line);
  return code.includes("](") ? maskLinkDestinations(code) : code;
}

/** The start of the first link to a note: `relative_note_links`, first match. */
function noteLinkStarts(line: string): number[] {
  const spans = codeSpans(line);
  const inCode = (at: number) =>
    spans.some(([from, to]) => at >= from && at < to);
  const starts: number[] = [];
  let cursor = 0;
  for (;;) {
    const found = line.indexOf("](", cursor);
    if (found < 0) {
      break;
    }
    const open = found + 2;
    cursor = open;
    const start = linkTextStart(line, found);
    if (start === null) {
      continue;
    }
    const dest = readDestination(line, open);
    if (!dest) {
      continue;
    }
    cursor = dest.close;
    const path = dest.path;
    const relative =
      path !== "" &&
      !path.startsWith("#") &&
      !path.startsWith("/") &&
      !path.includes("://") &&
      !path.startsWith("mailto:");
    if (
      line[start - 1] !== "!" &&
      path.toLowerCase().endsWith(".md") &&
      relative &&
      !inCode(found) &&
      !reserved(path)
    ) {
      starts.push(start);
    }
  }
  return starts;
}

/** `relation_type`: a quoted phrase, or one token not opening with `:` or `[`. */
function wikiType(before: string): boolean {
  const text = before.trim();
  if (text.length >= 2 && text.startsWith('"') && text.endsWith('"')) {
    return true;
  }
  return text !== "" && !/\s/.test(text) && !/^[:[]/.test(text);
}

const lower = (char: string) =>
  char === char.toLowerCase() && char !== char.toUpperCase();

/** `relative_relation_type`: narrower, so prose before a link is no type. */
function relativeType(before: string): boolean {
  const text = before.trim();
  if (text.length >= 2 && text.startsWith('"') && text.endsWith('"')) {
    return true;
  }
  const [first, ...rest] = [...text];
  return (
    first !== undefined &&
    lower(first) &&
    rest.every((c) => lower(c) || /[0-9_-]/.test(c))
  );
}

/** `LinkTarget::wikilink` as far as a relation needs it. */
function wikilinkTarget(inner: string): boolean {
  const bar = inner.indexOf("|");
  if (bar >= 0 && inner.slice(0, bar).trim() === "") {
    return false;
  }
  let target = (bar < 0 ? inner : inner.slice(0, bar)).trim();
  const hash = target.indexOf("#");
  if (hash >= 0) {
    target = target.slice(0, hash).trim();
  }
  const colon = target.indexOf(":");
  if (colon > 0) {
    const domain = target.slice(0, colon).trim();
    const rest = target.slice(colon + 1).trim();
    if (domain !== "" && rest !== "" && !/\s/.test(domain)) {
      target = rest;
    }
  }
  return !reserved(target);
}

function relationAt(content: string): number | null {
  const open = content.indexOf("[[");
  if (open >= 0) {
    let usable = true;
    if (content.includes("](")) {
      usable = maskForWikilinks(content)[open] === "[";
    }
    if (usable) {
      const after = content.slice(open + 2);
      const close = after.indexOf("]]");
      if (
        close >= 0 &&
        wikiType(content.slice(0, open)) &&
        wikilinkTarget(after.slice(0, close))
      ) {
        return open;
      }
    }
  }
  const first = content.indexOf("[");
  if (
    first >= 0 &&
    noteLinkStarts(content).includes(first) &&
    relativeType(content.slice(0, first))
  ) {
    return first;
  }
  return null;
}

// ---- how far a bullet wraps (bullet_end, continues_bullet) ----

function endsInsideLink(line: string): boolean {
  const masked = maskForWikilinks(line);
  const open = masked.lastIndexOf("[[");
  return open >= 0 && !masked.slice(open + 2).includes("]]");
}

function startsListItem(trimmed: string): boolean {
  const gap = (rest: string) => rest === "" || /^[ \t]/.test(rest);
  if (/^[-*+]/.test(trimmed)) {
    return gap(trimmed.slice(1));
  }
  const digits = /^\d*/.exec(trimmed)?.[0] ?? "";
  if (digits.length >= 1 && digits.length <= 9 && Number(digits) === 1) {
    const mark = trimmed[digits.length];
    if (mark === "." || mark === ")") {
      return gap(trimmed.slice(digits.length + 1));
    }
  }
  return false;
}

function thematicBreak(trimmed: string): boolean {
  const first = trimmed[0];
  if (first !== "-" && first !== "*" && first !== "_") {
    return false;
  }
  let count = 0;
  for (const char of trimmed) {
    if (char === first) {
      count += 1;
    } else if (char !== " " && char !== "\t") {
      return false;
    }
  }
  return count >= 3;
}

function continuesBullet(line: string): boolean {
  const trimmed = line.trim();
  if (trimmed === "" || /^ {0,3}#{1,6}(?:[ \t]|$)/.test(line)) {
    return false;
  }
  if (/^(?:`{3,}|~{3,})/.test(trimmed)) {
    return false;
  }
  return !(
    startsListItem(trimmed) ||
    /^[><|]/.test(trimmed) ||
    thematicBreak(trimmed)
  );
}

/**
 * The type of the relation the bullet starting at `lineAt(0)` is, as a span
 * of that first line, or null. `lineAt(n)` answers the n-th line after the
 * first (undefined at the end, or inside a fence).
 */
export function relationType(
  lineAt: (offset: number) => string | undefined,
): TypeSpan | null {
  const line = lineAt(0);
  if (line === undefined || !line.startsWith("- ")) {
    return null;
  }
  const first = line.slice(2);
  let content = first;
  if (!/^\[[^\]]*$/.test(first)) {
    let previous = line;
    for (let offset = 1; ; offset += 1) {
      const next = lineAt(offset);
      if (
        next === undefined ||
        !continuesBullet(next) ||
        endsInsideLink(previous)
      ) {
        break;
      }
      content = `${content.trimEnd()} ${next.trim()}`;
      previous = next;
    }
  }
  if (readsAsCategory(content)) {
    return null;
  }
  const open = relationAt(content);
  if (open === null) {
    return null;
  }
  const before = content.slice(0, open);
  const lead = before.length - before.trimStart().length;
  const from = 2 + lead;
  const to = from + before.trim().length;
  return to <= line.length ? { from, to } : null;
}
