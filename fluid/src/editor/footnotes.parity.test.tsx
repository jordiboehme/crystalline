/**
 * The editor's footnote numbers against the reading page's, for the cases
 * where the two could differ. The page is the authority: whatever it numbers
 * the editor must show, and what it leaves as text the editor leaves too.
 */

import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Markdown } from "../components/Markdown";
import { cleanupMarkViews, markView, setCaret } from "../test/markKit";
import { ThemeProvider } from "../theme/ThemeProvider";

vi.mock("mermaid", () => ({
  default: { initialize: vi.fn(), render: vi.fn() },
}));

afterEach(cleanupMarkViews);

const CASES: [string, string][] = [
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

async function pageNumbers(source: string): Promise<string[]> {
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
    .map((el) => el.textContent ?? "");
  unmount();
  return numbers;
}

describe("footnote numbers match the reading page", () => {
  it.each(CASES)("%s", async (_name, source) => {
    const view = markView(source);
    setCaret(view, source.length);
    const drawn = [...view.dom.querySelectorAll(".cm-footnote-ref")].map(
      (el) => el.textContent ?? "",
    );
    expect(drawn).toEqual(await pageNumbers(source));
  });
});
