/**
 * What the markdown renderer is allowed to turn into elements.
 *
 * The one rule that matters for safety: markdown arrives from whatever wrote
 * the engram, so raw HTML inside it stays text. react-markdown holds that line
 * by default and only gives it up if someone adds `rehype-raw`, so this file is
 * the tripwire for that edit rather than a test of the library.
 *
 * The rest is wiring, asserted once each: GitHub tables, syntax highlighting,
 * the frontmatter block that every engram and manifest carries, and a mermaid
 * fence, which becomes a diagram rather than a code block.
 */

import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { ThemeProvider } from "../theme/ThemeProvider";
import { Markdown } from "./Markdown";

/**
 * Render and wait for the renderer's own chunk to arrive: `Markdown` is a lazy
 * seam, so nothing is on screen until it has. The wait is for the fallback to
 * be gone rather than for it to appear and go, because after the first test in
 * a file the module is loaded and there is no fallback at all.
 */
async function renderMarkdown(source: string) {
  const result = render(
    <ThemeProvider>
      <Markdown source={source} />
    </ThemeProvider>,
  );
  await waitFor(() => {
    expect(screen.queryByText("Rendering")).toBeNull();
  });
  return result;
}

describe("the markdown renderer", () => {
  it("never turns raw HTML into elements", async () => {
    const { container } = await renderMarkdown(
      [
        "<script>globalThis.pwned = true;</script>",
        "",
        '<img src=x onerror="globalThis.pwned = true">',
        "",
        "A <b>bold</b> claim.",
      ].join("\n"),
    );

    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("b")).toBeNull();
    expect((globalThis as Record<string, unknown>).pwned).toBeUndefined();
    // The markup is shown as what it is: the characters somebody wrote, not
    // an element the browser acts on.
    expect(screen.getByText("A <b>bold</b> claim.")).toBeVisible();
  });

  it("drops the frontmatter block rather than drawing it", async () => {
    await renderMarkdown(
      ["---", "title: Alpha", "status: stable", "---", "", "# Alpha", ""].join(
        "\n",
      ),
    );

    expect(screen.getByRole("heading", { name: "Alpha" })).toBeVisible();
    expect(screen.queryByText(/title: Alpha/)).toBeNull();
  });

  it("renders GitHub tables", async () => {
    const { container } = await renderMarkdown(
      ["| a | b |", "| - | - |", "| 1 | 2 |", ""].join("\n"),
    );

    expect(container.querySelector("table")).not.toBeNull();
    expect(screen.getByRole("columnheader", { name: "a" })).toBeVisible();
  });

  it("highlights a fenced code block", async () => {
    const { container } = await renderMarkdown(
      ["```ts", "const answer = 42;", "```", ""].join("\n"),
    );

    expect(container.querySelector("code.hljs")).not.toBeNull();
  });

  it("renders a mermaid fence as a diagram, not as code", async () => {
    const { container } = await renderMarkdown(
      ["```mermaid", "graph TD;", "  A-->B;", "```", ""].join("\n"),
    );

    expect(screen.getByLabelText("Diagram")).toBeInTheDocument();
    expect(container.querySelector("code.hljs")).toBeNull();
  });

  it("leaves wikilinks as the text they are written as", async () => {
    // Resolution belongs to the engram page, which has the API's resolved
    // links; here a `[[link]]` is prose.
    await renderMarkdown("See [[Alpha]] for the rest.");

    expect(screen.getByText(/See \[\[Alpha\]\] for the rest\./)).toBeVisible();
    expect(screen.queryByRole("link")).toBeNull();
  });
});
