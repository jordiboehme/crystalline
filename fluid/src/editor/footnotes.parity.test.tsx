/**
 * The editor's footnotes against the reading page's, per reference: for each
 * case the sorted reference numbers, and which note text each number carries.
 * DOM order is not compared (the page moves its notes to a footer). The page
 * is the authority: what it numbers the editor shows, and what it leaves as
 * text the editor leaves too. The cases are the earlier ones plus a reviewer's
 * set of documents where a hand-written grammar disagreed with the page.
 */

import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Markdown } from "../components/Markdown";
import { footnoteModel } from "./footnotes";
import { cleanupMarkViews, markView, setCaret } from "../test/markKit";
import { ThemeProvider } from "../theme/ThemeProvider";

vi.mock("mermaid", () => ({
  default: { initialize: vi.fn(), render: vi.fn() },
}));

afterEach(cleanupMarkViews);

const EARLIER: [string, string][] = [
  ["two references in order", "a[^n] b[^m]\n\n[^m]: M\n[^n]: N\n"],
  ["a reference used twice", "a[^x] b[^y] c[^x]\n\n[^y]: Y\n[^x]: X\n"],
  ["a definition nobody refers to", "text[^a]\n\n[^a]: A\n[^z]: Z\n"],
  ["a reference with no definition", "text[^a] and[^q]\n\n[^a]: A\n"],
  ["labels in different case", "a[^A] b[^a] c[^B]\n\n[^a]: x\n[^b]: y\n"],
  [
    "references in code",
    "x[^a] `[^a]`\n\n```\n[^b]\n```\n\n[^a]: A\n[^b]: B\n",
  ],
  [
    "a reference inside a note",
    "a[^1] b[^2]\n\n[^1]: one[^3]\n[^2]: two\n[^3]: three\n",
  ],
  [
    "two definitions of a label",
    "a[^1] b[^2]\n\n[^2]: second\n[^1]: first\n[^1]: again\n",
  ],
  [
    "a reference in a table cell",
    "| a[^1] |\n| --- |\n\nb[^2]\n\n[^2]: two\n[^1]: one\n",
  ],
];

const REVIEWED: [string, string][] = [
  [
    "display math",
    "a[^1]\n\n$$\nx[^2]\n$$\n\nb[^3]\n\n[^1]: one\n[^2]: two\n[^3]: three\n",
  ],
  ["math fence", "a[^1]\n\n```math\nx[^2]\n```\n\n[^1]: one\n[^2]: two\n"],
  ["def in list", "a[^1]\n\n- [^1]: one\n"],
  ["def in quote", "a[^1]\n\n> [^1]: one\n"],
  ["escaped", "a\\[^1] b[^2]\n\n[^1]: one\n[^2]: two\n"],
  [
    "ref in link text",
    "[see[^1]](http://x.org) b[^2]\n\n[^1]: one\n[^2]: two\n",
  ],
  ["ref in image alt", "![see[^1]](x.png) b[^2]\n\n[^1]: one\n[^2]: two\n"],
  [
    "ref in link url",
    "[see](http://x.org/[^1]) b[^2]\n\n[^1]: one\n[^2]: two\n",
  ],
  ["ref in autolink", "<http://x.org/[^1]> b[^2]\n\n[^1]: one\n[^2]: two\n"],
  ["ref in bare url", "http://x.org/[^1] b[^2]\n\n[^1]: one\n[^2]: two\n"],
  ["label with space", "a[^a b] c[^1]\n\n[^a b]: x\n[^1]: one\n"],
  ["unicode label case", "a[^Ü] c[^1]\n\n[^ü]: x\n[^1]: one\n"],
  ["sharp s", "a[^ß] c[^1]\n\n[^SS]: x\n[^1]: one\n"],
  [
    "multi paragraph note",
    "a[^1] b[^2]\n\n[^1]: one\n\n    more[^3]\n\n[^2]: two\n[^3]: three\n",
  ],
  [
    "heading after def",
    "a[^1]\n\n[^1]: one\n# H[^3]\n\nb[^2]\n\n[^2]: two\n[^3]: three\n",
  ],
  [
    "list after def",
    "a[^1]\n\n[^1]: one\n- item[^3]\n\nb[^2]\n\n[^2]: two\n[^3]: three\n",
  ],
  [
    "lazy line",
    "a[^1]\n\n[^1]: one\nlazy[^3]\n\nb[^2]\n\n[^2]: two\n[^3]: three\n",
  ],
  ["def interrupts para", "a[^1]\n[^1]: one\n"],
  ["escaped bracket label", "a[^a\\]b] c[^1]\n\n[^a\\]b]: x\n[^1]: one\n"],
  ["caret in label", "a[^a^b]\n\n[^a^b]: x\n"],
  ["html inline", "<span>a[^1]</span> b[^2]\n\n[^1]: one\n[^2]: two\n"],
  ["html block", "<div>\na[^1]\n</div>\n\nb[^2]\n\n[^1]: one\n[^2]: two\n"],
  ["github inline math", "x $`a[^1]`$ b[^2]\n\n[^1]: one\n[^2]: two\n"],
  ["dollar inline", "x $$a[^1]$$ b[^2]\n\n[^1]: one\n[^2]: two\n"],
  ["def indented code ctx", "a[^1]\n\n    [^1]: one\n"],
  ["ref in wikilink", "[[T[^1]]] b[^2]\n\n[^1]: one\n[^2]: two\n"],
  ["ref in comment", "a <!-- [^1] --> b[^2]\n\n[^1]: one\n[^2]: two\n"],
  [
    "ref in def of other but referenced from body later",
    "a[^1]\n\n[^1]: one[^2]\n\nb[^3] c[^2]\n\n[^2]: two\n[^3]: three\n",
  ],
  ["def in list item continuation", "- a[^1]\n\n  [^1]: one\n"],
  [
    "ref in link ref def",
    "[x]: http://a.org/[^1]\n\nb[^2]\n\n[^1]: one\n[^2]: two\n",
  ],
  ["ref label is link ref", "[^1]\n\n[^1]: http://a\n"],
  ["empty-ish def tab", "a[^1]\n\n[^1]:\tone\n"],
  ["ref then colon", "a [^1]: b\n\n[^1]: one\n"],
  [
    "ref in table cell then body",
    "| h |\n| - |\n| a[^2] |\n\nb[^1]\n\n[^1]: one\n[^2]: two\n",
  ],
  ["callout", "> [!NOTE]\n> a[^1]\n\n[^1]: one\n"],
  ["setext", "a[^2]\n===\n\n[^1]: one\n[^2]: two\n"],
  [
    "tab indent def",
    "a[^1] b[^2]\n\n[^1]: one\n\n\tmore[^3]\n\n[^2]: two\n[^3]: three\n",
  ],
];

const CASES = [...EARLIER, ...REVIEWED];

/** A label as both sides compare it: case and escapes aside. */
const labelOf = (label: string) => label.toLowerCase().toUpperCase();

const firstWord = (text: string) => /[A-Za-z]+/.exec(text)?.[0] ?? "";

async function onPage(source: string): Promise<string[]> {
  const { container, unmount } = render(
    <MemoryRouter>
      <ThemeProvider>
        <Markdown source={source} />
      </ThemeProvider>
    </MemoryRouter>,
  );
  await waitFor(() => {
    expect(
      screen.queryByRole("status", { name: "Loading the text" }),
    ).toBeNull();
  });
  const numbers = [...container.querySelectorAll("sup a")]
    .filter((el) => !el.hasAttribute("data-footnote-backref"))
    .map(
      (el) =>
        `${labelOf(decodeURIComponent(el.getAttribute("href") ?? "").replace(/^.*-fn-/, ""))}=${el.textContent ?? ""}`,
    )
    .sort();
  const notes = [...container.querySelectorAll("[data-footnotes] li")].map(
    (li, index) =>
      `${index + 1}:${firstWord((li.textContent ?? "").replace(/↩.*/s, ""))}`,
  );
  unmount();
  return [...numbers, "|", ...notes.sort()];
}

describe("footnotes match the reading page", () => {
  it.each(CASES)("%s", async (_name, source) => {
    const view = markView(source);
    setCaret(view, source.length);
    const model = footnoteModel(view.state);
    const numbers = model.refs
      .map((ref) => `${labelOf(ref.label.slice(2, -1))}=${ref.number}`)
      .sort();
    const notes = model.notes.map(
      (note) => `${note.number}:${firstWord(note.text)}`,
    );
    expect([...numbers, "|", ...notes.sort()]).toEqual(await onPage(source));
  });
});
