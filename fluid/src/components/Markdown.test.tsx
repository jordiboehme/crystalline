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
import { MemoryRouter } from "react-router";
import { describe, expect, it } from "vitest";

import { ThemeProvider } from "../theme/ThemeProvider";
import type { WikilinkResolver } from "../wikilinks";
import { Markdown } from "./Markdown";

/**
 * Render and wait for the renderer's own chunk to arrive: `Markdown` is a lazy
 * seam, so nothing is on screen until it has. The wait is for the fallback to
 * be gone rather than for it to appear and go, because after the first test in
 * a file the module is loaded and there is no fallback at all.
 *
 * Mounted inside a router because a resolved wikilink navigates in place, so
 * it is a router link rather than an anchor.
 */
async function renderMarkdown(source: string, wikilinks?: WikilinkResolver) {
  const result = render(
    <MemoryRouter>
      <ThemeProvider>
        <Markdown source={source} wikilinks={wikilinks} />
      </ThemeProvider>
    </MemoryRouter>,
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

  it("turns a wikilink into a link when a resolver says where it goes", async () => {
    await renderMarkdown("See [[Alpha]] for the rest.", (inner) =>
      inner === "Alpha"
        ? { kind: "resolved", href: "/d/eng/e/alpha", label: "Alpha" }
        : null,
    );

    const link = screen.getByRole("link", { name: "Alpha" });
    expect(link).toHaveAttribute("href", "/d/eng/e/alpha");
    // The brackets are the source's punctuation for a reference; once it is a
    // link the link itself says so.
    expect(screen.queryByText(/\[\[Alpha\]\]/)).toBeNull();
  });

  it("marks a wikilink the index could not resolve without linking it", async () => {
    await renderMarkdown("See [[Ghost]] for the rest.", () => ({
      kind: "unresolved",
    }));

    const marked = screen.getByTitle("not resolved");
    // Left as written, so a reader can see exactly what the engram claims
    // points somewhere.
    expect(marked).toHaveTextContent("[[Ghost]]");
    expect(marked.className).toContain("decoration-dotted");
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("leaves a wikilink the resolver knows nothing about as text", async () => {
    // Nothing known is not the same as known to be broken: before the graph
    // has answered, a wikilink is prose rather than a claim either way.
    await renderMarkdown("See [[Alpha]] for the rest.", () => null);

    expect(screen.getByText(/See \[\[Alpha\]\] for the rest\./)).toBeVisible();
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.queryByTitle("not resolved")).toBeNull();
  });

  it("never rewrites a wikilink inside code", async () => {
    await renderMarkdown(
      [
        "```md",
        "See [[Alpha]] here.",
        "```",
        "",
        "And `[[Alpha]]` inline.",
      ].join("\n"),
      () => ({ kind: "resolved", href: "/d/eng/e/alpha", label: "Alpha" }),
    );

    expect(screen.queryByRole("link")).toBeNull();
  });
});
