/**
 * A picture's alt text between the source an author wrote and the words a
 * field shows.
 *
 * The field shows the words: backslash escapes are taken out. An edit writes
 * back only the part that changed, worked out in display space and mapped to
 * the source through the escapes, so untouched characters keep their bytes,
 * escaped or not. What is written is encoded so the reference still parses as
 * written: brackets and backslashes escaped, and never a `](` the core's
 * scanner would read as the end of the alt (it finds references by `](`).
 */

/** ASCII punctuation, the characters a backslash escapes in markdown. */
const PUNCTUATION = /^[!-/:-@[-`{-~]$/;

/**
 * The display text of `source`, with each display character's source span:
 * `starts[i]` is where display character `i` begins in the source, and
 * `starts[display.length]` is the source's length.
 */
function readAlt(source: string): { display: string; starts: number[] } {
  let display = "";
  const starts: number[] = [];
  for (let at = 0; at < source.length; at += 1) {
    starts.push(at);
    const next = source[at + 1];
    if (source[at] === "\\" && next !== undefined && PUNCTUATION.test(next)) {
      display += next;
      at += 1;
    } else {
      display += source[at];
    }
  }
  starts.push(source.length);
  return { display, starts };
}

/** `source` as a field shows it: backslash escapes of ASCII punctuation removed. */
export function altDisplay(source: string): string {
  return readAlt(source).display;
}

/**
 * Typed text as alt source. `before` is the source character right in front
 * of it, so a `(` written right after a `]` is escaped too.
 */
function encode(text: string, before: string): string {
  let out = "";
  let last = before;
  for (const char of text.replace(/\r\n|[\r\n\t]/g, " ")) {
    let written: string;
    if (char === "\\" || char === "[" || char === "]") {
      written = `\\${char}`;
    } else if (char === "(" && last === "]") {
      written = "\\(";
    } else {
      written = char;
    }
    out += written;
    last = written[written.length - 1] ?? last;
  }
  return out;
}

/**
 * The one change that turns alt source `source` into one that displays as
 * `next`, in offsets inside the alt span, or null when nothing changed.
 */
export function altEdit(
  source: string,
  next: string,
): { from: number; to: number; insert: string } | null {
  const { display, starts } = readAlt(source);
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
  let to = starts[display.length - suffix] ?? source.length;
  let insert = encode(
    next.slice(prefix, next.length - suffix),
    source[from - 1] ?? "",
  );
  // The changed part may end in a `]` (typed, or the untouched one before a
  // deletion) right against an untouched `(`: that `(` is escaped too.
  const end =
    insert === "" ? (source[from - 1] ?? "") : insert[insert.length - 1];
  if (end === "]" && source[to] === "(") {
    to += 1;
    insert += "\\(";
  }
  return { from, to, insert };
}
