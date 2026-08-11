/**
 * Marking the searched-for words inside a search snippet.
 *
 * The engine sends a snippet as text: a window cut around the match, with no
 * highlight markers of any kind - though the window is cut out of the file, so
 * whatever markdown syntax it crossed comes along (see
 * {@link stripSnippetMarkup}). So the marking is done here, by finding the
 * query's own terms in the text the same way the engine found them -
 * whitespace-separated and case insensitively - and handing back the pieces for
 * a component to render as elements.
 *
 * Pieces rather than a string of markup, and that is the point: nothing in this
 * app ever turns a server string into HTML, so a snippet cut out of an engram
 * that happens to contain `<script>` is shown as the text it is.
 */

/**
 * A snippet with the markdown syntax taken back out.
 *
 * The engine cuts its window from the raw file, so a hit near a heading
 * arrives wearing `#` and a hit near a relation wears its brackets. Readers get
 * the text, searchers still get the mark - the terms are matched AFTER
 * stripping, so a word beside a marker is marked where it now sits.
 *
 * Only the syntax that survives a one-line cut is handled, and only when it
 * looks like syntax: a `#` with no space after it is a tag, not a heading, and
 * anything that is not markdown at all comes back exactly as it was written.
 */
export function stripSnippetMarkup(text: string): string {
  return text
    .replace(/```[^\n]*/g, " ")
    .replace(/^#{1,6}[ \t]+/gm, "")
    .replace(/(^|\s)#{1,6}[ \t]+/g, "$1")
    .replace(/^\s*>[ \t]?/gm, "")
    .replace(/\*\*([^*]+)\*\*/g, "$1")
    .replace(/(^|[^*])\*([^*]+)\*/g, "$1$2")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/\[\[([^\]]+)\]\]/g, "$1")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^---[ \t]*$/gm, "")
    .replace(/[ \t]+/g, " ")
    .trim();
}

/** One piece of a snippet: either matched text, or the text between matches. */
export interface SnippetPart {
  /** The text of this piece. */
  text: string;
  /** Whether it is one of the searched-for words. */
  match: boolean;
}

/**
 * The terms of a query, as the engine splits them: whitespace separated,
 * lowercased, each one kept once.
 */
export function searchTerms(query: string): string[] {
  const seen = new Set<string>();
  for (const term of query.split(/\s+/)) {
    const lowered = term.toLowerCase();
    if (lowered !== "") {
      seen.add(lowered);
    }
  }
  return [...seen];
}

/** Cut `text` into matched and unmatched pieces, in order. */
export function snippetParts(text: string, terms: string[]): SnippetPart[] {
  const ranges = matchRanges(text, terms);
  if (ranges.length === 0) {
    return [{ text, match: false }];
  }
  const parts: SnippetPart[] = [];
  let at = 0;
  for (const [start, end] of ranges) {
    if (start > at) {
      parts.push({ text: text.slice(at, start), match: false });
    }
    parts.push({ text: text.slice(start, end), match: true });
    at = end;
  }
  if (at < text.length) {
    parts.push({ text: text.slice(at), match: false });
  }
  return parts;
}

/**
 * Where the terms sit in the text, merged and in order.
 *
 * Case folding is done on a copy and the offsets are read off that copy, which
 * only holds while folding preserves length. It does not for every character in
 * Unicode, so a text whose lowercase copy is a different length is left
 * unmarked rather than marked in the wrong places.
 */
function matchRanges(text: string, terms: string[]): [number, number][] {
  const haystack = text.toLowerCase();
  if (haystack.length !== text.length) {
    return [];
  }
  const found: [number, number][] = [];
  for (const term of terms) {
    const needle = term.toLowerCase();
    if (needle === "" || needle.length !== term.length) {
      continue;
    }
    let at = haystack.indexOf(needle);
    while (at !== -1) {
      found.push([at, at + needle.length]);
      at = haystack.indexOf(needle, at + needle.length);
    }
  }
  found.sort((a, b) => a[0] - b[0] || b[1] - a[1]);

  const merged: [number, number][] = [];
  for (const [start, end] of found) {
    const last = merged[merged.length - 1];
    if (last && start <= last[1]) {
      last[1] = Math.max(last[1], end);
    } else {
      merged.push([start, end]);
    }
  }
  return merged;
}
