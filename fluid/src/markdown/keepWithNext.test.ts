import { describe, expect, it } from "vitest";

import { keepWithNext, linesOf } from "./keepWithNext";

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

  it("leaves a heading alone before another heading, a picture or a tall block", () => {
    const rows = Array.from({ length: 30 }, () =>
      el("tr", [el("td", [text("x")])]),
    );
    const tree = {
      type: "root",
      children: [
        el("h2", [text("A")]),
        el("h3", [text("B")]),
        el("p", [el("img", [], { src: "a.png" })]),
        el("h2", [text("C")]),
        el("table", [el("tbody", rows)]),
        el("h2", [text("D")]),
      ],
    };
    keepWithNext()(tree);
    expect(
      tree.children.map((c) => (c as { tagName?: string }).tagName),
    ).toEqual(["h2", "h3", "p", "h2", "table", "h2"]);
  });

  it("keeps a heading with a diagram of any length, and not with long code", () => {
    // A diagram never breaks and prints no taller than a page less its
    // heading, so the heading goes with it whatever its source counts.
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
    expect(
      tree.children.map((c) => (c as { tagName?: string }).tagName),
    ).toEqual(["div", "h2", "pre"]);
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

  it("leaves a heading alone before a quote too tall to keep", () => {
    const tree = {
      type: "root",
      children: [el("h2", [text("Steps")]), el("blockquote", [items(30)])],
    };
    keepWithNext()(tree);
    expect(
      tree.children.map((c) => (c as { tagName?: string }).tagName),
    ).toEqual(["h2", "blockquote"]);
  });
});
