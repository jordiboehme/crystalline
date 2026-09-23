/**
 * The MANIFEST bullet renderer: bold and inline code drawn rather than shown
 * as their own punctuation, and never a block element or raw HTML.
 */

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { InlineMarkdown } from "./InlineMarkdown";

describe("InlineMarkdown", () => {
  it("draws bold text around a code span instead of the literal punctuation", () => {
    const { container } = render(
      <InlineMarkdown source="**Search `project2030` first**" />,
    );

    const strong = container.querySelector("strong");
    expect(strong).not.toBeNull();
    const code = strong?.querySelector("code");
    expect(code).not.toBeNull();
    expect(code).toHaveTextContent("project2030");
    expect(screen.queryByText(/\*\*/)).toBeNull();
    expect(screen.queryByText(/`/)).toBeNull();
    // No block element at all: the caller supplies its own wrapper (a `<li>`,
    // in practice), and this renderer never adds one of its own.
    expect(container.querySelector("p")).toBeNull();
  });

  it("draws a link with the app's own styling and opens an outward one in a new tab", () => {
    render(
      <InlineMarkdown source="See [the guide](https://example.com/guide) first." />,
    );

    const link = screen.getByRole("link", { name: "the guide" });
    expect(link).toHaveAttribute("href", "https://example.com/guide");
    expect(link).toHaveAttribute("target", "_blank");
    expect(link.className).toContain("underline");
  });

  it("leaves plain prose with no markup untouched", () => {
    render(<InlineMarkdown source="Route here for eng questions." />);

    expect(screen.getByText("Route here for eng questions.")).toBeVisible();
  });

  it("never turns raw HTML into elements", () => {
    const { container } = render(
      <InlineMarkdown source="<img src=x onerror=alert(1)> a claim" />,
    );

    expect(container.querySelector("img")).toBeNull();
  });

  it("unwraps a block construct instead of drawing it", () => {
    // A bullet that happens to start with `#` is still one line of prose to
    // this renderer: the heading it would become in a block context loses
    // only its own wrapper, never the text.
    render(<InlineMarkdown source="# Not a heading here" />);

    expect(screen.queryByRole("heading")).toBeNull();
    expect(screen.getByText("Not a heading here")).toBeVisible();
  });
});
