/**
 * What the reading view does with an attachment: an image drawn from the files
 * route, a document linked to it, and everything else left exactly as written.
 *
 * The rewrite is domain-aware, like the wikilink resolver beside it - a stored
 * path is relative to the domain the engram lives in and means nothing without
 * one - so a renderer handed no domain rewrites nothing at all rather than
 * guessing at an address.
 *
 * The fragment is the other half. It never reaches the files route: the server
 * strips fragments when it resolves a reference, so an `src` carrying `#right`
 * would be asking for a file by a name nobody stored.
 */

import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { describe, expect, it } from "vitest";

import { ThemeProvider } from "../theme/ThemeProvider";
import { Markdown } from "./Markdown";

const DOMAIN = "eng";

/** The renderer is a lazy seam: nothing is on screen until its chunk lands. */
async function renderMarkdown(source: string, domain?: string) {
  const result = render(
    <MemoryRouter>
      <ThemeProvider>
        <Markdown
          source={source}
          {...(domain === undefined ? {} : { domain })}
        />
      </ThemeProvider>
    </MemoryRouter>,
  );
  await waitFor(() => {
    expect(screen.queryByText("crystallizing")).toBeNull();
  });
  return result;
}

/** The one image the document drew. */
function image(container: HTMLElement): HTMLImageElement {
  const found = container.querySelector("img");
  if (!found) {
    throw new Error("the document drew no image");
  }
  return found;
}

describe("attachments in the reading view", () => {
  it("draws a stored image from the files route", async () => {
    const { container } = await renderMarkdown(
      "![Shot](assets/2026/08/shot.png)",
      DOMAIN,
    );
    const drawn = image(container);
    expect(drawn.getAttribute("src")).toBe(
      "/api/v1/domains/eng/files/assets/2026/08/shot.png",
    );
    expect(drawn.getAttribute("alt")).toBe("Shot");
  });

  it("centers a fragment-free image as a responsive block", async () => {
    const { container } = await renderMarkdown("![Shot](assets/a.png)", DOMAIN);
    const drawn = image(container);
    expect(drawn.style.display).toBe("block");
    expect(drawn.style.marginLeft).toBe("auto");
    expect(drawn.style.marginRight).toBe("auto");
    expect(drawn.style.maxWidth).toBe("100%");
    expect(drawn.style.float).toBe("");
  });

  it("reads the fragment for placement and width, and never sends it", async () => {
    const { container } = await renderMarkdown(
      "![Shot](assets/a.png#right,w=50%)",
      DOMAIN,
    );
    const drawn = image(container);
    expect(drawn.getAttribute("src")).toBe(
      "/api/v1/domains/eng/files/assets/a.png",
    );
    expect(drawn.style.float).toBe("right");
    expect(drawn.style.width).toBe("50%");
  });

  it("floats left, fills the column and measures in pixels", async () => {
    const left = await renderMarkdown("![a](assets/a.png#left)", DOMAIN);
    expect(image(left.container).style.float).toBe("left");
    left.unmount();

    const full = await renderMarkdown("![a](assets/a.png#full)", DOMAIN);
    expect(image(full.container).style.width).toBe("100%");
    full.unmount();

    const pixels = await renderMarkdown("![a](assets/a.png#w=300)", DOMAIN);
    expect(image(pixels.container).style.width).toBe("300px");
  });

  it("links a non-image attachment to the files route in a new tab", async () => {
    await renderMarkdown("[The deck](assets/2026/08/deck.pdf)", DOMAIN);
    const link = screen.getByRole("link", { name: "The deck" });
    expect(link).toHaveAttribute(
      "href",
      "/api/v1/domains/eng/files/assets/2026/08/deck.pdf",
    );
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAttribute("rel", "noreferrer");
  });

  it("leaves an external image and an absolute one exactly as written", async () => {
    const external = await renderMarkdown(
      "![x](https://example.com/a.png)",
      DOMAIN,
    );
    expect(image(external.container).getAttribute("src")).toBe(
      "https://example.com/a.png",
    );
    external.unmount();

    const rooted = await renderMarkdown("![x](/assets/a.png)", DOMAIN);
    expect(image(rooted.container).getAttribute("src")).toBe("/assets/a.png");
  });

  it("leaves an external link alone, target and all", async () => {
    await renderMarkdown("[out](https://example.com/deck.pdf)", DOMAIN);
    const link = screen.getByRole("link", { name: "out" });
    expect(link).toHaveAttribute("href", "https://example.com/deck.pdf");
  });

  it("rewrites nothing when nobody said which domain", async () => {
    const { container } = await renderMarkdown("![Shot](assets/a.png)");
    expect(image(container).getAttribute("src")).toBe("assets/a.png");
  });

  it("asks for a name written in another script by the name it was stored under", async () => {
    // The renderer percent-encodes every target on its way through, so a name
    // that is not ASCII arrives as escapes. Encoding those a second time would
    // 404 on a file sitting right there.
    const { container } = await renderMarkdown(
      "![s](assets/2026/08/設計.png)",
      DOMAIN,
    );
    expect(image(container).getAttribute("src")).toBe(
      `/api/v1/domains/eng/files/assets/2026/08/${encodeURIComponent("設計.png")}`,
    );
  });

  it("encodes a path segment rather than handing the browser a raw one", async () => {
    const { container } = await renderMarkdown(
      "![s](assets/2026/08/a%b.png)",
      DOMAIN,
    );
    expect(image(container).getAttribute("src")).toBe(
      "/api/v1/domains/eng/files/assets/2026/08/a%25b.png",
    );
  });
});
