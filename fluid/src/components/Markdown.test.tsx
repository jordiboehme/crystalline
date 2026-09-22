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

import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router";
import { describe, expect, it, vi } from "vitest";

import { ThemeProvider } from "../theme/ThemeProvider";
import type { WikilinkResolver } from "../wikilinks";
import { Markdown } from "./Markdown";

// Mermaid draws nothing under jsdom - it measures text through layout this
// environment does not do - so a fence would always fall back to its source
// and the diagram's own controls would never exist. A stub drawing is what
// lets this file follow a fence all the way to the download it offers. The
// pan-and-zoom library is stubbed for the same reason the overlay's own test
// stubs it: what is asserted here is the name on the file, not the geometry.
vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn(() =>
      Promise.resolve({ svg: '<svg viewBox="0 0 600 400"><g/></svg>' }),
    ),
  },
}));

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

/**
 * Render and wait for the renderer's own chunk to arrive: `Markdown` is a lazy
 * seam, so nothing is on screen until it has. The wait is for the fallback to
 * be gone rather than for it to appear and go, because after the first test in
 * a file the module is loaded and there is no fallback at all.
 *
 * Mounted inside a router because a resolved wikilink navigates in place, so
 * it is a router link rather than an anchor.
 */
async function renderMarkdown(
  source: string,
  wikilinks?: WikilinkResolver,
  foldTitle?: string,
  documentName?: string,
  anchors?: { pageUrl: string },
  entry = "/",
) {
  const result = render(
    <MemoryRouter initialEntries={[entry]}>
      <ThemeProvider>
        <Markdown
          source={source}
          {...(wikilinks ? { wikilinks } : {})}
          {...(foldTitle === undefined ? {} : { foldTitle })}
          {...(documentName === undefined ? {} : { documentName })}
          {...(anchors === undefined ? {} : { anchors })}
        />
      </ThemeProvider>
    </MemoryRouter>,
  );
  await waitFor(() => {
    expect(screen.queryByText("crystallizing")).toBeNull();
  });
  return result;
}

/** Where the router stands, for the tests that care what a click did to it. */
function LocationProbe() {
  const { search, hash } = useLocation();
  return <span data-testid="location">{`${search}${hash}`}</span>;
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

  it("aligns a table column the way its delimiter row asks", async () => {
    // The colons in the delimiter row are the only place a markdown table can
    // say how a column reads, and they arrive here as a `style` prop carrying
    // `textAlign` - not as an `align` attribute - so a component map that
    // takes only `children` drops them and every column renders left. Header
    // and body cells both, because the alignment is the column's rather than
    // the row's.
    const { container } = await renderMarkdown(
      [
        "| mid | end | plain |",
        "| :-: | --: | --- |",
        "| 1 | 2 | 3 |",
        "",
      ].join("\n"),
    );

    const headers = [...container.querySelectorAll("th")];
    const cells = [...container.querySelectorAll("td")];
    expect(headers.map((cell) => cell.style.textAlign)).toEqual([
      "center",
      "right",
      "",
    ]);
    expect(cells.map((cell) => cell.style.textAlign)).toEqual([
      "center",
      "right",
      "",
    ]);
    // A column that says nothing keeps today's default: a header reading left
    // from its class, a body cell with no alignment of its own at all.
    expect(headers[2]?.className).toContain("text-left");
  });

  it("highlights a fenced code block", async () => {
    const { container } = await renderMarkdown(
      ["```ts", "const answer = 42;", "```", ""].join("\n"),
    );

    expect(container.querySelector("code.hljs")).not.toBeNull();
  });

  it("colors the individual tokens inside a fenced code block", async () => {
    // rehype-highlight wraps each token in its own `<span class="hljs-*">`,
    // and the custom `span` renderer above has one job for any span that is
    // not an unresolved wikilink marker: pass its incoming class through
    // rather than drop it. A prior version dropped it for every span,
    // silently coloring every fenced code block the page's plain text color
    // in both themes - the outer `code.hljs` check above could not catch
    // that, since it never looks at what is inside.
    const { container } = await renderMarkdown(
      ["```python", 'value = "a string token"', "```", ""].join("\n"),
    );

    expect(container.querySelector(".hljs-string")).not.toBeNull();
    expect(container.querySelector(".hljs-string")?.textContent).toContain(
      "a string token",
    );
  });

  it("renders a mermaid fence as a diagram, not as code", async () => {
    const { container } = await renderMarkdown(
      ["```mermaid", "graph TD;", "  A-->B;", "```", ""].join("\n"),
    );

    expect(screen.getByLabelText("Diagram")).toBeInTheDocument();
    expect(container.querySelector("code.hljs")).toBeNull();
  });

  it("lets wide artifacts break out of the reading measure", async () => {
    const { container } = await renderMarkdown(
      [
        "Prose.",
        "",
        "| a | b |",
        "| - | - |",
        "| 1 | 2 |",
        "",
        "```",
        "a very wide line of plain code",
        "```",
        "",
        "```mermaid",
        "graph TD;",
        "  A-->B;",
        "```",
        "",
      ].join("\n"),
    );

    const measured = container.querySelector(".measured");
    expect(measured).not.toBeNull();
    const breakouts = [...container.querySelectorAll(".breakout")];
    // The table's scroll box, the plain code block and the diagram figure.
    expect(breakouts.length).toBe(3);
    // Load-bearing: the rule is `.measured > :not(.breakout)`, so a breakout
    // that is not a DIRECT child of the measured container is silently capped
    // at 70ch along with the prose around it.
    for (const breakout of breakouts) {
      expect(breakout.parentElement).toBe(measured);
    }
    expect(screen.getByLabelText("Diagram").className).toContain("breakout");
  });

  it("leaves a task list item as a checkbox rather than chipping it", async () => {
    // `[x]` at the head of a bullet is GFM's checkbox, not an observation
    // category: the renderer has already turned it into an input, so the
    // bullet's text starts after it and there is no mark to chip.
    const { container } = await renderMarkdown(
      ["- [x] Ship the handover", ""].join("\n"),
    );

    expect(container.querySelector('input[type="checkbox"]')).not.toBeNull();
    expect(screen.queryByText("[x]")).toBeNull();
    expect(screen.getByText(/Ship the handover/)).toBeVisible();
  });

  it("draws a rel type written with a hyphen or an underscore as a chip", async () => {
    // The engine's rel types are identifiers, and both separators occur in
    // them; the chip has to survive either.
    await renderMarkdown(
      ["- superseded_by [[Alpha]]", "- part-of [[Beta]]", ""].join("\n"),
    );

    expect(screen.getByText("superseded_by")).toBeVisible();
    expect(screen.getByText("part-of")).toBeVisible();
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

  it("folds a leading H1 that repeats the title the page already drew", async () => {
    await renderMarkdown(
      ["# Lantern Protocol", "", "Body.", "", "# Another Heading", ""].join(
        "\n",
      ),
      undefined,
      "Lantern Protocol",
    );

    expect(screen.getByText("Body.")).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "Lantern Protocol" }),
    ).toBeNull();
    // Only the opening one, and only when it repeats: a later heading is the
    // document's own structure whatever it says.
    expect(
      screen.getByRole("heading", { name: "Another Heading" }),
    ).toBeVisible();
  });

  it("keeps a leading H1 that says something else", async () => {
    await renderMarkdown(
      ["# Different", "", "Body.", ""].join("\n"),
      undefined,
      "Lantern Protocol",
    );

    expect(screen.getByRole("heading", { name: "Different" })).toBeVisible();
  });

  it("draws an observation bullet's category as a chip", async () => {
    await renderMarkdown(
      [
        "## Observations",
        "",
        "- [gotcha] An unsigned handover is not a handover #protocol",
        "",
      ].join("\n"),
    );

    const chip = screen.getByText("[gotcha]");
    expect(chip.className).toContain("font-mono");
    // The line itself stays whole beside it, tag and all.
    expect(
      screen.getByText(/An unsigned handover is not a handover #protocol/),
    ).toBeVisible();
  });

  it("draws a relation bullet's type as a chip", async () => {
    await renderMarkdown(
      ["## Relations", "", "- relates_to [[Harbor Signal Log]]", ""].join("\n"),
    );

    expect(screen.getByText("relates_to")).toBeVisible();
    // With no resolver the target stays the literal text it was written as.
    expect(screen.getByText(/\[\[Harbor Signal Log\]\]/)).toBeVisible();
  });

  it("draws a relation bullet's type as a chip once the target is a link", async () => {
    await renderMarkdown(["- relates_to [[Alpha]]", ""].join("\n"), (inner) =>
      inner === "Alpha"
        ? { kind: "resolved", href: "/d/eng/e/alpha", label: "Alpha" }
        : null,
    );

    const chip = screen.getByText("relates_to");
    expect(chip.className).toContain("font-mono");
    expect(screen.getByRole("link", { name: "Alpha" })).toHaveAttribute(
      "href",
      "/d/eng/e/alpha",
    );
  });

  it("leaves an ordinary bullet whose first word precedes a link alone", async () => {
    // A word before an element is not a relation: the engine reads one only
    // where a `[[target]]` follows, so a chip here would claim a fact the
    // index does not hold.
    await renderMarkdown(
      ["- See [the guide](https://example.com/guide) first.", ""].join("\n"),
    );

    expect(screen.queryByText("See")).toBeNull();
    expect(screen.getByRole("link", { name: "the guide" })).toBeVisible();
  });

  it("leaves a bullet that is shaped like neither untouched", async () => {
    await renderMarkdown(["- Just a line.", ""].join("\n"));

    expect(screen.getByText("Just a line.")).toBeVisible();
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

  it("carries the document's name down to a diagram's download", async () => {
    // The whole thread in one test: the page names the document, the renderer
    // hands the name to the fence, and the fence's full window puts it on the
    // file. jsdom implements neither half of the object-URL pair, so both are
    // defined rather than spied on.
    const saved: string[] = [];
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: vi.fn(() => "blob:fake"),
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      configurable: true,
      value: vi.fn(),
    });
    const clicked = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(function (this: HTMLAnchorElement) {
        saved.push(this.download);
      });
    try {
      await renderMarkdown(
        ["```mermaid", "graph TD;", "  A-->B;", "```", ""].join("\n"),
        undefined,
        undefined,
        "alpha",
      );
      // The diagram and the overlay each arrive through a lazy import, so
      // both buttons are waited for rather than looked up.
      await userEvent.click(
        await screen.findByRole("button", { name: "Open in full window" }),
      );
      await userEvent.click(
        await screen.findByRole("button", { name: "Download source (.mmd)" }),
      );
      expect(saved[0]).toMatch(/^alpha-diagram-[0-9a-f]{4}\.mmd$/);
    } finally {
      clicked.mockRestore();
    }
  });
});

/**
 * Section anchors: the name every heading is reachable by, and the control
 * that hands that name over.
 *
 * What is pinned here is what a reader and an agent can rely on. Every heading
 * carries an id derived from its own text, so a URL written from the heading
 * text lands on the heading; the control beside it is a real button with a
 * real name, so a keyboard reaches it and a screen reader says what it is; and
 * arriving with a fragment points the section out rather than leaving somebody
 * to find it. A surface that does not opt in has none of it: no ids, no
 * buttons, nothing to trip over.
 */
describe("section anchors", () => {
  const PAGE = "https://kb.example.com/d/eng/e/alpha";
  const SOURCE = ["## API", "", "## API", "", "### Auth & Tokens", ""].join(
    "\n",
  );

  it("gives every heading an id from its text, numbered when repeated", async () => {
    const { container } = await renderMarkdown(
      SOURCE,
      undefined,
      undefined,
      undefined,
      { pageUrl: PAGE },
    );

    const ids = [...container.querySelectorAll("h2, h3")].map(
      (heading) => heading.id,
    );
    expect(ids).toEqual(["api", "api-1", "auth-tokens"]);
  });

  it("draws no ids and no link symbols without anchors", async () => {
    const { container } = await renderMarkdown(SOURCE);

    for (const heading of container.querySelectorAll("h2, h3")) {
      expect(heading.hasAttribute("id")).toBe(false);
    }
    expect(
      screen.queryByRole("button", { name: "Link to this section" }),
    ).toBeNull();
  });

  it("the link symbol is reachable by name and activates on Enter and Space", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>(() =>
      Promise.resolve(),
    );
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });

    await renderMarkdown(SOURCE, undefined, undefined, undefined, {
      pageUrl: PAGE,
    });

    // One per heading, and each one is found by its name rather than by its
    // position: a control with no text in it has nothing else to be known by.
    expect(
      screen.getAllByRole("button", { name: "Link to this section" }),
    ).toHaveLength(3);

    // In the tab order from the start, so it is reachable without a pointer
    // ever hovering the heading it belongs to.
    await userEvent.tab();
    expect(document.activeElement).toBe(
      screen.getAllByRole("button", { name: "Link to this section" })[0],
    );

    await userEvent.keyboard("{Enter}");
    expect(writeText).toHaveBeenLastCalledWith(`${PAGE}#api`);
    await userEvent.keyboard(" ");
    expect(writeText).toHaveBeenCalledTimes(2);
    expect(writeText).toHaveBeenLastCalledWith(`${PAGE}#api`);
  });

  it("says Link copied in the live region and beside the heading, then falls silent", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>(() =>
      Promise.resolve(),
    );
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    const { container } = await renderMarkdown(
      SOURCE,
      undefined,
      undefined,
      undefined,
      { pageUrl: PAGE },
    );
    const region = screen.getByRole("status", {
      name: "Section link result",
    });
    expect(region).toHaveTextContent("");

    vi.useFakeTimers();
    try {
      fireEvent.click(
        screen.getAllByRole("button", { name: "Link to this section" })[2]!,
      );
      await act(async () => {
        await Promise.resolve();
      });

      expect(region).toHaveTextContent("Link copied");
      // And beside the heading that was activated, which is the one a reader
      // watching the pointer is looking at rather than the foot of the page.
      expect(container.querySelector("#auth-tokens")).toHaveTextContent(
        "Link copied",
      );

      act(() => {
        vi.advanceTimersByTime(2000);
      });
      expect(region).toHaveTextContent("");
      expect(container.querySelector("#auth-tokens")).not.toHaveTextContent(
        "Link copied",
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("scrolls to and flashes the heading the hash names, and does nothing for a missing one", async () => {
    const scrolled = vi
      .spyOn(Element.prototype, "scrollIntoView")
      .mockImplementation(() => undefined);
    try {
      const { container, unmount } = await renderMarkdown(
        SOURCE,
        undefined,
        undefined,
        undefined,
        { pageUrl: PAGE },
        "/x#auth-tokens",
      );

      const target = container.querySelector("#auth-tokens");
      expect(scrolled).toHaveBeenCalledWith({ block: "start" });
      expect(scrolled.mock.instances[0]).toBe(target);
      expect(target).toHaveClass("section-flash");
      unmount();

      // A fragment naming nothing in this document opens the page at the top,
      // says nothing and throws nothing: an anchor is a hint, never a promise
      // the page has to keep.
      scrolled.mockClear();
      const missing = await renderMarkdown(
        SOURCE,
        undefined,
        undefined,
        undefined,
        { pageUrl: PAGE },
        "/x#nowhere",
      );
      expect(scrolled).not.toHaveBeenCalled();
      expect(missing.container.querySelector(".section-flash")).toBeNull();
    } finally {
      scrolled.mockRestore();
    }
  });

  it("names the heading by its own text, with the control still reachable", async () => {
    await renderMarkdown(SOURCE, undefined, undefined, undefined, {
      pageUrl: PAGE,
    });

    // A button inside a heading joins the heading's computed name in a real
    // browser, which would put "Link to this section" into every entry of a
    // screen reader's heading list and into the document outline. The heading
    // says what names it instead: a span around its own text and nothing else.
    //
    // Asserted through the wiring rather than through the computed name
    // alone: this environment's name computation does not descend into the
    // nested button, so the name reads correctly with or without the fix and
    // would not notice it going away. What cannot be faked is the reference
    // and what sits at the other end of it.
    const heading = screen.getByRole("heading", { name: "Auth & Tokens" });
    const labelledBy = heading.getAttribute("aria-labelledby");
    expect(labelledBy).not.toBeNull();
    const label = document.getElementById(labelledBy!);
    expect(label?.textContent).toBe("Auth & Tokens");
    expect(label?.querySelector("button")).toBeNull();
    expect(heading).toHaveAccessibleName("Auth & Tokens");
    // And the control keeps its own name, in the tab order, where it was.
    expect(
      screen.getAllByRole("button", { name: "Link to this section" }),
    ).toHaveLength(3);
  });

  it("stops washing the heading it left when a second hash arrives", async () => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: vi.fn(() => Promise.resolve()) },
      configurable: true,
    });
    const { container } = await renderMarkdown(
      SOURCE,
      undefined,
      undefined,
      undefined,
      { pageUrl: PAGE },
    );
    const buttons = screen.getAllByRole("button", {
      name: "Link to this section",
    });

    await userEvent.click(buttons[0]!);
    expect(container.querySelector("#api")).toHaveClass("section-flash");

    // The wash has to come off the one being left, not merely stop counting
    // down: a reader who asked for reduced motion gets the tint with no fade,
    // so a class left behind would stay on that heading for good.
    await userEvent.click(buttons[2]!);
    expect(container.querySelector("#api")).not.toHaveClass("section-flash");
    expect(container.querySelector("#auth-tokens")).toHaveClass(
      "section-flash",
    );
  });

  it("keeps the page's query string when a link symbol is activated", async () => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: vi.fn(() => Promise.resolve()) },
      configurable: true,
    });
    render(
      <MemoryRouter initialEntries={["/x?tab=graph"]}>
        <ThemeProvider>
          <Markdown source={SOURCE} anchors={{ pageUrl: PAGE }} />
          <LocationProbe />
        </ThemeProvider>
      </MemoryRouter>,
    );
    await waitFor(() => {
      expect(screen.queryByText("crystallizing")).toBeNull();
    });

    await userEvent.click(
      screen.getAllByRole("button", { name: "Link to this section" })[0]!,
    );

    // The fragment is added to where the reader stands rather than replacing
    // it: a page opened with a query string is still that page afterwards.
    expect(screen.getByTestId("location")).toHaveTextContent("?tab=graph#api");
  });
});
