/**
 * The MANIFEST bullet renderer: bold and inline code drawn rather than shown
 * as their own punctuation, and never a block element or raw HTML.
 *
 * The renderer itself is a lazy seam (`InlineMarkdownBody`), so every test
 * here waits past the Suspense fallback - the plain source text - before
 * asserting on the drawn result.
 */

import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { InlineMarkdown } from "./InlineMarkdown";

describe("InlineMarkdown", () => {
  it("draws bold text around a code span instead of the literal punctuation", async () => {
    const { container } = render(
      <InlineMarkdown source="**Search `project2030` first**" />,
    );

    await waitFor(() => {
      expect(container.querySelector("strong")).not.toBeNull();
    });
    const strong = container.querySelector("strong");
    const code = strong?.querySelector("code");
    expect(code).not.toBeNull();
    expect(code).toHaveTextContent("project2030");
    expect(screen.queryByText(/\*\*/)).toBeNull();
    expect(screen.queryByText(/`/)).toBeNull();
    // No block element at all: the caller supplies its own wrapper (a `<li>`,
    // in practice), and this renderer never adds one of its own.
    expect(container.querySelector("p")).toBeNull();
  });

  it("draws a link with the app's own styling and opens an outward one in a new tab", async () => {
    render(
      <InlineMarkdown source="See [the guide](https://example.com/guide) first." />,
    );

    const link = await screen.findByRole("link", { name: "the guide" });
    expect(link).toHaveAttribute("href", "https://example.com/guide");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link.className).toContain("underline");
  });

  it("leaves plain prose with no markup untouched", async () => {
    render(<InlineMarkdown source="Route here for eng questions." />);

    // True of the Suspense fallback already, since it is the source text
    // itself - and stays true once the renderer's chunk has landed.
    expect(
      await screen.findByText("Route here for eng questions."),
    ).toBeVisible();
  });

  it("never turns raw HTML into elements", async () => {
    const { container } = render(
      <InlineMarkdown source="<img src=x onerror=alert(1)> a claim" />,
    );

    await waitFor(() => {
      expect(screen.queryByText(/a claim/)).not.toBeNull();
    });
    expect(container.querySelector("img")).toBeNull();
  });

  it("unwraps a block construct instead of drawing it", async () => {
    // A bullet that happens to start with `#` is still one line of prose to
    // this renderer: the heading it would become in a block context loses
    // only its own wrapper, never the text.
    render(<InlineMarkdown source="# Not a heading here" />);

    expect(await screen.findByText("Not a heading here")).toBeVisible();
    expect(screen.queryByRole("heading")).toBeNull();
  });
});
