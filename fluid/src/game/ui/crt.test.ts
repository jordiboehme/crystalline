import { describe, expect, it } from "vitest";

import { CRT_COLUMNS, crtLines, type CrtLine } from "./crt";

/** Shorthand for the expected lines of a case. */
function l(kind: CrtLine["kind"], text: string): CrtLine {
  return { kind, text };
}

describe("crtLines", () => {
  it("drops the frontmatter", () => {
    const { lines } = crtLines(
      ["---", "title: Hidden", "type: note", "---", "Visible text"].join("\n"),
    );
    expect(lines).toEqual([l("text", "Visible text")]);
  });

  it("turns every heading level into a heading line and keeps its case", () => {
    const md = [1, 2, 3, 4, 5, 6]
      .map((n) => `${"#".repeat(n)} Level ${String(n)} Title ##`)
      .join("\n");
    const { lines } = crtLines(md);
    expect(lines).toEqual(
      [1, 2, 3, 4, 5, 6].map((n) => l("heading", `Level ${String(n)} Title`)),
    );
  });

  it("joins a paragraph and word-wraps it to the columns", () => {
    const { lines } = crtLines(
      "the quick brown fox\njumps over   the lazy dog",
      20,
    );
    expect(lines).toEqual([
      l("text", "the quick brown fox"),
      l("text", "jumps over the lazy"),
      l("text", "dog"),
    ]);
  });

  it("hard-breaks a word longer than the columns", () => {
    const word = "x".repeat(200);
    const { lines } = crtLines(`see ${word} end`);
    expect(lines).toEqual([
      l("text", "see"),
      l("text", "x".repeat(CRT_COLUMNS)),
      l("text", "x".repeat(CRT_COLUMNS)),
      l("text", `${"x".repeat(40)} end`),
    ]);
  });

  it("keeps every list marker and hangs the wrapped text under it", () => {
    const md = [
      "- dash item",
      "* star item",
      "+ plus item",
      "1. first one that wraps around",
    ].join("\n");
    const { lines } = crtLines(md, 20);
    expect(lines).toEqual([
      l("list", "- dash item"),
      l("list", "* star item"),
      l("list", "+ plus item"),
      l("list", "1. first one that"),
      l("list", "   wraps around"),
    ]);
  });

  it("indents nested lists by 2 spaces per level", () => {
    const md = ["- top", "  - middle", "    - bottom", "- back"].join("\n");
    expect(crtLines(md).lines).toEqual([
      l("list", "- top"),
      l("list", "  - middle"),
      l("list", "    - bottom"),
      l("list", "- back"),
    ]);
  });

  it("handles mixed nesting of ordered and unordered lists at any indent", () => {
    const md = [
      "1. step",
      "    - detail with a longer text",
      "        1. sub step",
      "    - second detail",
      "2. next step",
      "   continued on the next line",
    ].join("\n");
    expect(crtLines(md, 24).lines).toEqual([
      l("list", "1. step"),
      l("list", "  - detail with a longer"),
      l("list", "    text"),
      l("list", "    1. sub step"),
      l("list", "  - second detail"),
      l("list", "2. next step continued"),
      l("list", "   on the next line"),
    ]);
  });

  it("keeps fenced code verbatim and hard-wraps it at the columns", () => {
    const md = [
      "```rust",
      "fn main() {",
      "    let **x** = [a](b);",
      "",
      "}",
      "```",
      "~~~",
      "0123456789abcdefghijXYZ",
      "~~~",
    ].join("\n");
    expect(crtLines(md, 20).lines).toEqual([
      l("code", "fn main() {"),
      l("code", "    let **x** = [a]("),
      l("code", "b);"),
      l("code", ""),
      l("code", "}"),
      l("code", "0123456789abcdefghij"),
      l("code", "XYZ"),
    ]);
  });

  it("turns a mermaid or plantuml fence into one diagram note", () => {
    const md = [
      "```mermaid",
      "graph TD",
      "  A --> B",
      "```",
      "~~~plantuml",
      "@startuml",
      "@enduml",
      "~~~",
    ].join("\n");
    expect(crtLines(md).lines).toEqual([
      l("note", "[DIAGRAM: mermaid]"),
      l("note", "[DIAGRAM: plantuml]"),
    ]);
  });

  it("turns an image into a note with its alt text, or its source without one", () => {
    const md = [
      "![The station map](map.png)",
      "",
      "Before ![](diagrams/flow.svg) after",
    ].join("\n");
    expect(crtLines(md).lines).toEqual([
      l("note", "[IMAGE: The station map]"),
      l("blank", ""),
      l("text", "Before"),
      l("note", "[IMAGE: diagrams/flow.svg]"),
      l("text", "after"),
    ]);
  });

  it("keeps only the text of links and wikilinks", () => {
    const md =
      "See [the docs](https://example.com), [[Target]], [[ops:Runbook]] and [[ops:Runbook|the runbook]].";
    expect(crtLines(md).lines).toEqual([
      l("text", "See the docs, Target, Runbook and the runbook."),
    ]);
  });

  it("strips emphasis and code markers but keeps the text", () => {
    const md =
      "**bold** __strong__ *em* _under_ `code` stay: snake_case_name 2*3";
    expect(crtLines(md).lines).toEqual([
      l("text", "bold strong em under code stay: snake_case_name 23"),
    ]);
  });

  it("turns blockquote lines into quotes with a prefix", () => {
    const md = ["> first **quoted** line", ">second"].join("\n");
    expect(crtLines(md).lines).toEqual([
      l("quote", "> first quoted line"),
      l("quote", "> second"),
    ]);
  });

  it("keeps table rows as trimmed text", () => {
    const md = ["  | a | b |  ", "|---|---|", "| 1 | 2 |"].join("\n");
    expect(crtLines(md).lines).toEqual([
      l("text", "| a | b |"),
      l("text", "|---|---|"),
      l("text", "| 1 | 2 |"),
    ]);
  });

  it("collapses runs of blank lines into one blank", () => {
    const md = ["\n\n", "one", "", "", "   ", "two", "", ""].join("\n");
    expect(crtLines(md).lines).toEqual([
      l("text", "one"),
      l("blank", ""),
      l("text", "two"),
    ]);
  });

  it("lists every level-two heading's line index under its text, in order", () => {
    const md = [
      "# Title",
      "## Notes",
      "one",
      "### Deeper",
      "```",
      "## Not a heading",
      "```",
      "## Notes",
      "two",
    ].join("\n");
    const { lines, sections } = crtLines(md);
    expect(sections).toEqual(new Map([["Notes", [1, 5]]]));
    expect(lines[1]).toEqual(l("heading", "Notes"));
    expect(lines[5]).toEqual(l("heading", "Notes"));
  });

  it("keys a section by the heading as written, markup included", () => {
    const { sections } = crtLines("## The **real** one");
    expect(sections).toEqual(new Map([["The **real** one", [0]]]));
  });

  it("returns no lines and no sections for an empty document", () => {
    for (const md of ["", "\n\n  \n", "---\ntitle: x\n---\n"]) {
      const { lines, sections } = crtLines(md);
      expect(lines).toEqual([]);
      expect(sections.size).toBe(0);
    }
  });
});
