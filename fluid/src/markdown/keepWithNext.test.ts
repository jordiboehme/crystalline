import { describe, expect, it } from "vitest";

import { isPicture, keepWithNext, linesOf } from "./keepWithNext";

interface Node {
  type: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  children?: Node[];
  value?: string;
}

const el = (
  tagName: string,
  children: Node[] = [],
  properties: Record<string, unknown> = {},
): Node => ({ type: "element", tagName, properties, children });
const text = (value: string): Node => ({ type: "text", value });

describe("keepWithNext", () => {
  it("wraps a heading with the block after it, line breaks between them included", () => {
    const tree = {
      type: "root",
      children: [
        el("h2", [text("Status")]),
        text("\n"),
        el("p", [text("Short.")]),
        text("\n"),
        el("p", [text("Next.")]),
      ],
    };
    keepWithNext()(tree);
    expect(
      tree.children.map((c) => (c as { tagName?: string }).tagName ?? "text"),
    ).toEqual(["div", "text", "p"]);
    const kept = tree.children[0] as { properties: object; children: object[] };
    expect(kept.properties).toEqual({ "data-keep": "" });
    expect(kept.children).toHaveLength(3);
  });

  it("keeps a heading with only the first lines of a tall block, and of the next heading", () => {
    const rows = Array.from({ length: 30 }, () =>
      el("tr", [el("td", [text("x")])]),
    );
    const long = el("p", [text("word ".repeat(600))]);
    const tree = {
      type: "root",
      children: [
        el("h2", [text("A")]),
        el("h3", [text("B")]),
        long,
        el("h2", [text("C")]),
        el("table", [el("tbody", rows)]),
        el("h2", [text("D")]),
      ],
    };
    keepWithNext()(tree);
    const kids = tree.children;
    expect(kids.map((c) => c.tagName)).toEqual([
      "div",
      "div",
      "p",
      "div",
      "table",
      "h2",
    ]);
    // Each wrapper holds its heading alone and asks for room under it: a
    // heading's own lines and three more, a paragraph's first three lines,
    // a table's first two rows.
    for (const [index, lines] of [
      [0, 6],
      [1, 3],
      [3, 4.2],
    ] as const) {
      const lead = kids[index]!;
      expect(lead.properties).toEqual({
        "data-keep": "lead",
        style: `--keep-lines: ${String(lines)}`,
      });
      expect(lead.children!.map((c) => c.tagName)).toEqual([
        index === 1 ? "h3" : "h2",
      ]);
    }
    expect(kids[2]).toBe(long);
  });

  it("asks for room for a long list's first item, and caps it", () => {
    const item = (words: number) => el("li", [text("word ".repeat(words))]);
    const list = (first: Node) =>
      el("ul", [first, ...Array.from({ length: 40 }, () => item(10))]);
    const nested = el("li", [text("Step."), el("ul", [item(1)])]);
    const tree = {
      type: "root",
      children: [
        el("h2", [text("Short first")]),
        list(item(10)),
        el("h2", [text("Six lines")]),
        list(item(110)),
        el("h2", [text("Nested first")]),
        list(nested),
        el("h2", [text("Long first")]),
        list(item(400)),
      ],
    };
    keepWithNext()(tree);
    const styles = tree.children
      .filter((c) => c.properties?.["data-keep"] === "lead")
      .map((c) => c.properties?.["style"]);
    // One line, six lines, and a first item that may break: as a paragraph.
    expect(styles).toEqual([
      "--keep-lines: 1",
      "--keep-lines: 6",
      "--keep-lines: 3",
      "--keep-lines: 3",
    ]);
  });

  it("keeps a heading with a picture of its own, alone or in a link, as one unit", () => {
    const image = el("img", [], { src: "a.png" });
    const tree = {
      type: "root",
      children: [
        el("h2", [text("Alone")]),
        el("p", [text(" "), image, text("\n")]),
        el("h2", [text("Linked")]),
        el("p", [el("a", [image], { href: "https://example.com" })]),
        el("h2", [text("In a sentence")]),
        el("p", [text("See "), image, text(" here.")]),
      ],
    };
    keepWithNext()(tree);
    const kids = tree.children;
    // A picture in a sentence is text whose height is not known: the
    // heading keeps its first lines.
    expect(kids.map((c) => c.properties?.["data-keep"])).toEqual([
      "unit",
      "unit",
      "lead",
      undefined,
    ]);
    expect(kids[0]!.children!.map((c) => c.tagName)).toEqual(["h2", "p"]);
    expect(kids[1]!.children!.map((c) => c.tagName)).toEqual(["h2", "p"]);
  });

  it("tells a picture of its own from a picture in the text", () => {
    const image = el("img", [], { src: "a.png" });
    expect(isPicture(el("p", [image]))).toBe(true);
    expect(isPicture(el("p", [text("\n"), image, text("  ")]))).toBe(true);
    expect(isPicture(el("p", [el("a", [image])]))).toBe(true);
    expect(isPicture(el("p", [el("a", [image, text("caption")])]))).toBe(false);
    expect(isPicture(el("p", [image, image]))).toBe(false);
    expect(isPicture(el("p", [text("See "), image]))).toBe(false);
    expect(isPicture(el("div", [image]))).toBe(false);
    expect(isPicture(el("p", [el("em", [image])]))).toBe(false);
  });

  it("keeps a heading with a diagram of any length as one unit, and with long code as a pair", () => {
    // A diagram never breaks and prints no taller than a page less its
    // heading, so the heading goes with it whatever its source counts. Code
    // does not break either, so its heading goes with it too, in a block
    // that may still break if it is taller than a page.
    const fence = (lang: string) =>
      el("pre", [el("code", [text("x\n".repeat(40))], { className: [lang] })]);
    const tree = {
      type: "root",
      children: [
        el("h2", [text("Diagram")]),
        fence("language-mermaid"),
        el("h2", [text("Code")]),
        fence("language-rust"),
      ],
    };
    keepWithNext()(tree);
    expect(tree.children.map((c) => c.properties?.["data-keep"])).toEqual([
      "unit",
      "",
    ]);
  });

  it("counts a diagram taller than its source", () => {
    const code = (lang: string, lines: number) =>
      el("pre", [
        el("code", [text("x\n".repeat(lines))], { className: [lang] }),
      ]);
    expect(linesOf(code("language-mermaid", 8))!).toBeGreaterThan(
      linesOf(code("language-rust", 8))!,
    );
  });

  it("only groups the top level", () => {
    const quote = el("blockquote", [
      el("h2", [text("In")]),
      el("p", [text("x")]),
    ]);
    const tree = { type: "root", children: [quote] };
    keepWithNext()(tree);
    expect(tree.children[0]).toBe(quote);
  });
});

describe("short tables", () => {
  it("marks a table of at most 12 rows short, and no longer one", () => {
    const rows = (n: number) =>
      Array.from({ length: n }, () => el("tr", [el("td", [text("x")])]));
    const small = el("table", [el("tbody", rows(12))]);
    const long = el("table", [el("tbody", rows(13))]);
    keepWithNext()({ type: "root", children: [small, long] });
    expect(small.properties).toHaveProperty("dataShort", "");
    expect(long.properties).not.toHaveProperty("dataShort");
  });
});

describe("short list items", () => {
  it("keeps a short item whole, and not one holding a list or a long one", () => {
    const short = el("li", [text("Check the lamp.")]);
    const long = el("li", [text("word ".repeat(200))]);
    const inner = el("li", [text("Sub step.")]);
    const outer = el("li", [text("Step."), el("ul", [inner])]);
    keepWithNext()({
      type: "root",
      children: [el("ul", [short, long, outer])],
    });
    expect(short.properties).toHaveProperty("dataShort", "");
    expect(long.properties).not.toHaveProperty("dataShort");
    expect(outer.properties).not.toHaveProperty("dataShort");
    expect(inner.properties).toHaveProperty("dataShort", "");
  });
});

describe("the height estimate", () => {
  const items = (n: number) =>
    el(
      "ul",
      Array.from({ length: n }, () => el("li", [el("p", [text("Short.")])])),
    );

  it("counts what a quote or a callout holds, not only its characters", () => {
    // Thirty short items are few characters and many lines.
    const quote = el("blockquote", [items(30)]);
    expect(linesOf(quote)!).toBeGreaterThan(30);
    const callout = el("div", [el("div", [text("Note")]), items(30)]);
    expect(linesOf(callout)!).toBeGreaterThan(30);
  });

  it("counts every hard break and every wrapped code line", () => {
    const breaks = el(
      "p",
      Array.from({ length: 20 }, () => [text("x"), el("br")]).flat(),
    );
    expect(linesOf(breaks)!).toBeGreaterThanOrEqual(20);
    const code = el("pre", [el("code", [text(`${"y".repeat(800)}\n`)])]);
    expect(linesOf(code)!).toBeGreaterThanOrEqual(10);
  });

  it("counts a table row as tall as its longest cell", () => {
    const row = (cell: string) =>
      el("tr", [el("td", [text(cell)]), el("td", [text("x")])]);
    const narrow = el("table", [el("tbody", [row("x"), row("x")])]);
    const wide = el("table", [el("tbody", [row("z ".repeat(300)), row("x")])]);
    expect(linesOf(wide)!).toBeGreaterThan(linesOf(narrow)! + 5);
  });

  it("keeps a heading with a quote of any length, which never breaks either", () => {
    const tree = {
      type: "root",
      children: [el("h2", [text("Steps")]), el("blockquote", [items(30)])],
    };
    keepWithNext()(tree);
    expect(tree.children.map((c) => c.properties)).toEqual([
      { "data-keep": "" },
    ]);
  });

  it("keeps a heading before a paragraph with a picture in it to its first lines", () => {
    const tree = {
      type: "root",
      children: [
        el("h2", [text("Mixed")]),
        el("p", [text("See "), el("img", [], { src: "a.png" })]),
      ],
    };
    keepWithNext()(tree);
    expect((tree.children[0] as Node).properties).toEqual({
      "data-keep": "lead",
      style: "--keep-lines: 3",
    });
  });
});

describe("the notes at the end", () => {
  it("keeps the notes' label with the first notes, and leaves other sections alone", () => {
    const notes = el("ol", [el("li", [el("p", [text("The note.")])])]);
    const footnotes = el(
      "section",
      [el("h2", [text("Footnotes")]), text("\n"), notes],
      { dataFootnotes: true },
    );
    const other = el("section", [el("h2", [text("Other")]), el("p", [])]);
    const tree = { type: "root", children: [footnotes, other] };
    keepWithNext()(tree);
    expect(footnotes.children!.map((c) => c.properties)).toEqual([
      { "data-keep": "" },
    ]);
    expect(other.children!.map((c) => c.tagName)).toEqual(["h2", "p"]);
  });
});
