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
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";

import { ThemeProvider } from "../theme/ThemeProvider";
import { Markdown } from "./Markdown";

// The full-window layer loads the pan-and-zoom library on first open, and
// jsdom has no layout for it to work with. What this file is about is that an
// image offers the layer at all; the layer's own contract is pinned in
// `DiagramOverlay.test.tsx`.
vi.mock("@panzoom/panzoom", () => ({
  default: vi.fn(() => ({
    zoomIn: vi.fn(),
    zoomOut: vi.fn(),
    zoom: vi.fn(),
    pan: vi.fn(),
    reset: vi.fn(),
    zoomWithWheel: vi.fn(),
    destroy: vi.fn(),
  })),
}));

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

/**
 * The wrapper around it, which is what carries the placement: the image fills
 * the wrapper and the wrapper sits where the fragment asked for, so the
 * toolbar floating in its corner floats in the image's corner.
 */
function placement(container: HTMLElement): HTMLElement {
  const wrapper = image(container).parentElement;
  if (!wrapper) {
    throw new Error("the image was drawn without its wrapper");
  }
  return wrapper;
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
    const around = placement(container);
    expect(around.style.display).toBe("block");
    expect(around.style.marginLeft).toBe("auto");
    expect(around.style.marginRight).toBe("auto");
    expect(around.style.maxWidth).toBe("100%");
    expect(around.style.float).toBe("");
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
    expect(placement(container).style.float).toBe("right");
    expect(placement(container).style.width).toBe("50%");
  });

  it("floats left, fills the column and measures in pixels", async () => {
    const left = await renderMarkdown("![a](assets/a.png#left)", DOMAIN);
    expect(placement(left.container).style.float).toBe("left");
    left.unmount();

    const full = await renderMarkdown("![a](assets/a.png#full)", DOMAIN);
    expect(placement(full.container).style.width).toBe("100%");
    full.unmount();

    const pixels = await renderMarkdown("![a](assets/a.png#w=300)", DOMAIN);
    expect(placement(pixels.container).style.width).toBe("300px");
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

  it("resolves a reference written with a leading ./ , image and link alike", async () => {
    // The core scanner strips the `./` before it tests the prefix, so such a
    // reference IS a reference to the engine: it dangling-checks, it is swept
    // by evolve, and a reading view that left it alone would draw a broken
    // image beside a rail row that says the file is there.
    const { container } = await renderMarkdown(
      "![Shot](./assets/2026/08/shot.png#right)\n\n[The deck](./assets/deck.pdf)",
      DOMAIN,
    );
    expect(image(container).getAttribute("src")).toBe(
      "/api/v1/domains/eng/files/assets/2026/08/shot.png",
    );
    expect(placement(container).style.float).toBe("right");
    expect(screen.getByRole("link", { name: "The deck" })).toHaveAttribute(
      "href",
      "/api/v1/domains/eng/files/assets/deck.pdf",
    );
  });

  it("strips the fragment from a document link too, which the route never sees", async () => {
    await renderMarkdown("[The deck](assets/deck.pdf#right)", DOMAIN);
    expect(screen.getByRole("link", { name: "The deck" })).toHaveAttribute(
      "href",
      "/api/v1/domains/eng/files/assets/deck.pdf",
    );
  });

  it("builds no URL at all for a target carrying a dot segment", async () => {
    // The core validator refuses `..` outright, so no file can be stored under
    // one and this app must never ask for the address it would resolve to.
    const { container } = await renderMarkdown(
      "![x](assets/../../evil.png)\n\n[y](assets/../secret.pdf)",
      DOMAIN,
    );
    expect(image(container).getAttribute("src")).toBe("assets/../../evil.png");
    expect(screen.getByRole("link", { name: "y" })).toHaveAttribute(
      "href",
      "assets/../secret.pdf",
    );
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

  it("offers an image in the full window, by its button and by a click", async () => {
    const { container } = await renderMarkdown(
      "![The map](assets/map.png)",
      DOMAIN,
    );
    expect(placement(container).className).toContain("group");
    const open = screen.getByRole("button", { name: "Open in full window" });
    // An image has no reading width to fold back to, so the width action the
    // diagram toolbar carries is simply not there.
    expect(screen.queryByRole("button", { name: /width/ })).toBeNull();
    await userEvent.click(open);
    const dialog = await screen.findByRole("dialog");
    expect(dialog.querySelector("img")?.getAttribute("alt")).toBe("The map");
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    await userEvent.click(image(container));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("lets a link win over the full window when the image is inside one", async () => {
    const { container } = await renderMarkdown(
      "[![The map](assets/map.png)](https://example.org/map)",
      DOMAIN,
    );
    await userEvent.click(image(container));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Open in full window" }),
    ).toBeNull();
  });
});
