/**
 * Cutting a snippet into marked and unmarked pieces.
 *
 * The pieces have to add back up to the snippet exactly. A highlighter that
 * drops or duplicates a character is worse than no highlighter at all: the
 * reader is looking at the sentence to decide whether the hit is the one they
 * want, and every test here is a way that sentence could come out changed.
 */

import { describe, expect, it } from "vitest";

import { searchTerms, snippetParts, stripSnippetMarkup } from "./snippet";

/** The pieces, put back together. */
function rejoined(text: string, terms: string[]): string {
  return snippetParts(text, terms)
    .map((part) => part.text)
    .join("");
}

/** Just the marked pieces. */
function marked(text: string, terms: string[]): string[] {
  return snippetParts(text, terms)
    .filter((part) => part.match)
    .map((part) => part.text);
}

describe("the query's terms", () => {
  it("splits on whitespace, lowercases and keeps each word once", () => {
    expect(searchTerms("  Rule  of RULE thumb ")).toEqual([
      "rule",
      "of",
      "thumb",
    ]);
  });

  it("has no terms for an empty query", () => {
    expect(searchTerms("   ")).toEqual([]);
  });
});

describe("a snippet's markup", () => {
  it("is taken back out, so a reader gets the sentence rather than its source", () => {
    expect(stripSnippetMarkup("# Lantern Protocol")).toBe("Lantern Protocol");
    expect(
      stripSnippetMarkup("## Relations - relates_to [[Lantern Protocol]]"),
    ).toBe("Relations - relates_to Lantern Protocol");
    expect(stripSnippetMarkup("uses `docText` and **bold** text")).toBe(
      "uses docText and bold text",
    );
  });

  it("leaves the words a search matches on where they were", () => {
    // The terms are matched after stripping, so what the mark lands on is the
    // stripped text: a hit next to a heading marker still marks the word.
    expect(marked(stripSnippetMarkup("## Lantern rules"), ["lantern"])).toEqual(
      ["Lantern"],
    );
  });

  it("leaves text that is not markdown exactly as it was written", () => {
    // A snippet quoting HTML is text like any other: nothing here turns it
    // into markup, and nothing here eats it either.
    expect(stripSnippetMarkup("<b>rule</b> of thumb")).toBe(
      "<b>rule</b> of thumb",
    );
    expect(stripSnippetMarkup("tagged #eng and issue #42")).toBe(
      "tagged #eng and issue #42",
    );
  });
});

describe("a snippet's pieces", () => {
  it("marks each occurrence, whatever case it was written in", () => {
    const text = "The Rule of thumb: every rule has one.";
    expect(marked(text, ["rule"])).toEqual(["Rule", "rule"]);
    expect(rejoined(text, ["rule"])).toBe(text);
  });

  it("merges terms that overlap rather than nesting them", () => {
    const text = "retrieval latency";
    expect(marked(text, ["retrieval", "trie", "val lat"])).toEqual([
      "retrieval lat",
    ]);
    expect(rejoined(text, ["retrieval", "trie", "val lat"])).toBe(text);
  });

  it("leaves a snippet with no match in one unmarked piece", () => {
    expect(snippetParts("nothing here", ["absent"])).toEqual([
      { text: "nothing here", match: false },
    ]);
    expect(snippetParts("nothing here", [])).toEqual([
      { text: "nothing here", match: false },
    ]);
  });

  it("marks nothing rather than the wrong thing when case folding shifts", () => {
    // Folding this character lengthens the string, so the offsets read off the
    // folded copy would point somewhere else in the original.
    const text = "İstanbul rule";
    expect(marked(text, ["rule"])).toEqual([]);
    expect(rejoined(text, ["rule"])).toBe(text);
  });
});
