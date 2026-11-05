/**
 * A picture's alt text between the source an author wrote and the words a
 * field shows: escapes out for display, and an edit that writes back only
 * what changed, encoded so the reference still parses as written.
 */

import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import { unified } from "unified";
import { describe, expect, it } from "vitest";

import { altDisplay, altEdit } from "./altText";

/** The alt the reading page gives `![source](x.png)`: the oracle. */
function readingAlt(source: string): string | null {
  const tree = unified()
    .use(remarkParse)
    .use(remarkGfm)
    .parse(`![${source}](x.png)`);
  let found: string | null = null;
  const walk = (node: {
    type: string;
    alt?: string | null;
    children?: unknown[];
  }) => {
    if (found !== null) return;
    if (node.type === "image") {
      found = node.alt ?? "";
      return;
    }
    for (const child of node.children ?? []) walk(child as typeof node);
  };
  walk(tree);
  return found;
}

/** Alt sources whose display the reading page decides. */
const SOURCES = [
  "a *b* c",
  "a **b** c",
  "a ~~b~~ c",
  "a _b_ c",
  "***both***",
  "a `code` b",
  "a &amp; b",
  "&#65;&#x42;",
  "&copy; 2026",
  "&nosuch; x",
  "a \\*b\\* c",
  "a \\[b\\]",
  "x_y_z",
  "2 * 3 * 4",
  "a [b](https://x.y) c",
  "snake_case_name",
  "a < b",
  "a <https://x.y> b",
  "mail <me@x.y> now",
  "plain words",
];

/** The source after `altEdit`'s change, or the source itself. */
function applied(source: string, next: string): string {
  const edit = altEdit(source, next);
  if (edit === null) return source;
  return source.slice(0, edit.from) + edit.insert + source.slice(edit.to);
}

describe("altDisplay", () => {
  it("drops backslash escapes of ASCII punctuation", () => {
    expect(altDisplay("a \\[b\\]")).toBe("a [b]");
    expect(altDisplay("x \\\\ y \\( z")).toBe("x \\ y ( z");
  });

  it("keeps a backslash before anything else", () => {
    expect(altDisplay("a\\b \\é")).toBe("a\\b \\é");
  });
});

describe("altDisplay reads as the reading page does (M3)", () => {
  it.each(SOURCES)("%j", (source) => {
    expect(altDisplay(source)).toBe(readingAlt(source));
  });

  it("drops emphasis marks and decodes entities", () => {
    expect(altDisplay("a *b* c")).toBe("a b c");
    expect(altDisplay("a &amp; b")).toBe("a & b");
  });
});

describe("altEdit over marks and entities (M3)", () => {
  const EDITS: [string, string][] = [
    ["a *b* c", "a *b* d"],
    ["a *b* c", "a b d"],
    ["a *b* c", "a  c"],
    ["a *b* c", "a c"],
    ["a *b* c", "X a b c"],
    ["a **bold** c", "a bolder c"],
    ["a &amp; b", "a & c"],
    ["a &amp; b", "a + b"],
    ["~~s~~ t", "s t u"],
    ["a b", "a *b*"],
    ["a b", "a_b_c &amp; `x`"],
    ["x", "~~y~~"],
    ["<https://x.y>", "X https://x.y"],
    ["a <https://x.y> b", "a https://x.y c"],
  ];

  it.each(EDITS)("%j to %j reads as typed", (source, next) => {
    expect(altDisplay(applied(source, next))).toBe(next);
  });

  it("types before an autolink without touching it", () => {
    expect(altEdit("<https://x.y>", "X https://x.y")).toEqual({
      from: 0,
      to: 0,
      insert: "X ",
    });
    expect(applied("<https://x.y> a", "X https://x.y a")).toBe(
      "X <https://x.y> a",
    );
    expect(applied("<https://x.y> a", "https://x.y ab")).toBe(
      "<https://x.y> ab",
    );
  });

  it("keeps the marks it does not need to touch", () => {
    expect(applied("a *b* c", "a *b* d".replaceAll("*", ""))).toBe("a *b* d");
    expect(altEdit("a *b* c", "a b d")).toEqual({
      from: 6,
      to: 7,
      insert: "d",
    });
    expect(applied("a &amp; b", "a & c")).toBe("a &amp; c");
  });

  it("writes a typed mark character as text only where it would be a mark", () => {
    expect(applied("Shot", "Shot 2*3")).toBe("Shot 2*3");
    expect(applied("a b", "a *b*")).toBe("a \\*b\\*");
  });

  it("answers null when the words read the same", () => {
    expect(altEdit("a *b* c", "a b c")).toBeNull();
    expect(altEdit("a &amp; b", "a & b")).toBeNull();
  });
});

describe("altEdit", () => {
  it("inserts only what was typed", () => {
    expect(altEdit("Shot", "Shot 2")).toEqual({ from: 4, to: 4, insert: " 2" });
  });

  it("replaces only the changed character and keeps brackets as written", () => {
    expect(altEdit("a [b] c", "a [b] d")).toEqual({
      from: 6,
      to: 7,
      insert: "d",
    });
  });

  it("keeps the escapes of untouched characters", () => {
    expect(applied("a \\[b\\] c", "a [b] d")).toBe("a \\[b\\] d");
    expect(altEdit("a \\[b\\] c", "a [b] d")).toEqual({
      from: 8,
      to: 9,
      insert: "d",
    });
  });

  it("encodes brackets, backslashes and a parenthesis after a bracket", () => {
    expect(altEdit("x", "a](b")).toEqual({
      from: 0,
      to: 1,
      insert: "a\\]\\(b",
    });
    expect(applied("x", "[\\]")).toBe("\\[\\\\\\]");
  });

  it("never leaves a bracket and a parenthesis side by side", () => {
    // A typed `]` before an untouched `(`.
    expect(applied("a (b)", "a] (b)").includes("](")).toBe(false);
    expect(applied("a(b", "a](b")).toBe("a\\]\\(b");
    // A typed `(` after an untouched `]`.
    expect(applied("[a] b", "[a](b")).toBe("[a]\\(b");
    // A deleted space that brings the two together.
    expect(applied("[a] (b)", "[a](b)")).toBe("[a]\\(b)");
  });

  it("writes a typed line break or tab as one space", () => {
    expect(altEdit("a", "a\nb")).toEqual({ from: 1, to: 1, insert: " b" });
    expect(applied("a", "a\r\nb\tc")).toBe("a b c");
  });

  it("answers null when nothing changed", () => {
    expect(altEdit("x", "x")).toBeNull();
    expect(altEdit("a \\[b\\]", "a [b]")).toBeNull();
  });
});
