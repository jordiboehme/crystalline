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
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useEffect, useState } from "react";
import { Link, MemoryRouter, useLocation } from "react-router";
import { describe, expect, it, vi } from "vitest";

import { REVEAL_EVENT } from "../markdown/callouts";
import { ThemeProvider } from "../theme/ThemeProvider";
import { readEngramDetail } from "../api/engram";
import { readGraph } from "../api/graph";
import type { WikilinkResolver } from "../wikilinks";
import { buildWikilinkResolver } from "../wikilinks";
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
    expect(
      screen.queryByRole("status", { name: "Loading the text" }),
    ).toBeNull();
  });
  return result;
}

/**
 * Render with source spans, the way the reflection page does: the same router
 * and theme as {@link renderMarkdown}, and the same wait for the chunk.
 */
async function renderSpans(source: string) {
  const result = render(
    <MemoryRouter>
      <ThemeProvider>
        <Markdown source={source} sourceSpans />
      </ThemeProvider>
    </MemoryRouter>,
  );
  await waitFor(() => {
    expect(
      screen.queryByRole("status", { name: "Loading the text" }),
    ).toBeNull();
  });
  return result;
}

/** Every stamped span reads exactly what the source holds at its offset. */
function expectSpansMatch(container: HTMLElement, source: string) {
  const stamped = Array.from(container.querySelectorAll("[data-src]"));
  for (const span of stamped) {
    const offset = Number(span.getAttribute("data-src"));
    expect(source.slice(offset, offset + (span.textContent ?? "").length)).toBe(
      span.textContent,
    );
  }
  return stamped;
}

describe("source spans", () => {
  it("stamps positioned text with its source offset when asked, and only then", async () => {
    const source = "## Night deploys\n\nDeploys run **at** night.";
    const { container, unmount } = await renderSpans(source);
    const at = await screen.findByText("at");
    expect(at.getAttribute("data-src")).toBe("32");
    expectSpansMatch(container, source);
    const stampedHtml = container.innerHTML;
    unmount();
    const plain = await renderMarkdown(source);
    await screen.findByText("at");
    expect(plain.container.querySelector("[data-src]")).toBeNull();
    // Nothing but the stamps differs.
    expect(
      stampedHtml.replace(/<span data-src="\d+">([^<]*)<\/span>/g, "$1"),
    ).toBe(plain.container.innerHTML);
  });

  it("never renders a section id comment", async () => {
    const { container } = await renderSpans(
      "## A\n<!-- r:7f3a9c2e -->\n\nText.",
    );
    await screen.findByText("Text.");
    expect(container.innerHTML).not.toContain("r:7f3a9c2e");
  });

  it("keeps every stamp true to the source across the whole syntax", async () => {
    const source = [
      "## Rich section",
      "",
      "Plain *em* and **strong** and _under_ and ~~gone~~ and `code` here.",
      "A [link](https://example.com) and a ![picture](assets/x.png) too.",
      "Line one",
      "line two.",
      "",
      "> single quoted line",
      "",
      "> quoted line",
      "> second quoted line",
      "",
      "- item one",
      "- item **two**",
      "  continued",
      "",
      "1. first",
      "",
      "| a | b |",
      "| - | - |",
      "| *c* | d |",
      "",
      "Inline <b>raw</b> html.",
      "",
      "Tom &amp; Jerry.",
      "",
      "Use \\*stars\\* here.",
      "",
      "```js",
      "const x = 1;",
      "```",
      "",
    ].join("\n");
    const { container } = await renderSpans(source);
    await screen.findByText("Rich section");
    const stamped = expectSpansMatch(container, source);
    const texts = stamped.map((span) => span.textContent);
    expect(texts).toEqual(
      expect.arrayContaining([
        "em",
        "strong",
        "under",
        "gone",
        "code",
        "link",
        // A list item, an ordered one and a table cell.
        "item one",
        "first",
        "d",
        // A blockquote of one line, and each line of one of two.
        "single quoted line",
        "quoted line",
        "second quoted line",
        // The wrapped line of a list item, placed after its indent.
        "continued",
      ]),
    );
    // Unplaced on purpose: a run whose words differ from the source (an
    // entity, an escape) is not one piece of it.
    for (const words of ["Tom & Jerry.", "Use *stars* here."]) {
      expect(screen.getByText(words).hasAttribute("data-src")).toBe(false);
    }
    expect(texts.some((t) => /Tom|stars/.test(t ?? ""))).toBe(false);
    // A fence at the margin is stamped after the highlighter, token by
    // token, and every token reads the source at its offset.
    const fence = Array.from(container.querySelectorAll("pre [data-src]"));
    expect(fence.length).toBeGreaterThan(1);
    expect(fence.map((span) => span.textContent).join("")).toBe(
      "const x = 1;\n",
    );
  });

  it("stamps a fence only where its text is one piece of the source", async () => {
    const source = [
      "## Code",
      "",
      "```",
      "plain fence",
      "```",
      "",
      "- in a list:",
      "",
      "  ```sh",
      "  indented one",
      "  two",
      "  ```",
      "",
      "~~~py",
      "print('tilde')",
      "~~~",
      "",
    ].join("\n");
    const { container } = await renderSpans(source);
    await screen.findByText("Code");
    const stamped = expectSpansMatch(container, source);
    const words = stamped.map((span) => span.textContent).join("");
    expect(words).toContain("plain fence");
    expect(words).toContain("tilde");
    // Its lines carry the list's indent, which the block's text has not.
    expect(words).not.toContain("indented one");
  });
});

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

  it("never draws an HTML comment, and leaves code and other HTML as text", async () => {
    const { container } = await renderMarkdown(
      [
        "# Notes",
        "",
        "<!-- sources: the harbor master's log -->",
        "",
        "A line <!-- an aside --> that goes on.",
        "",
        "<!--",
        "  a note over",
        "  three lines",
        "-->",
        "",
        "- An item <!-- in a list -->",
        "",
        "> Quoted <!-- in a quote -->",
        "",
        "<!-- before --> <span>after</span>",
        "",
        "Code `<!-- in a span -->` stays.",
        "",
        "```html",
        "<!-- in a fence -->",
        "```",
        "",
        "<div>not a comment</div>",
        "",
      ].join("\n"),
    );
    const text = container.textContent ?? "";
    for (const gone of [
      "sources:",
      "an aside",
      "a note over",
      "in a list",
      "in a quote",
      "before",
    ]) {
      expect(text).not.toContain(gone);
    }
    expect(screen.getByText(/A line\s+that goes on\./)).toBeVisible();
    // What is not a comment in a raw block stays, as its text.
    expect(text).toContain("<span>after</span>");
    // Code is code: the comment in it is what was written.
    expect(text).toContain("<!-- in a span -->");
    expect(container.querySelector("pre")).toHaveTextContent(
      "<!-- in a fence -->",
    );
    // Any other raw HTML is drawn as text, as before.
    expect(text).toContain("<div>not a comment</div>");
  });

  it("marks a short list item and a short table for the print sheet", async () => {
    const { container } = await renderMarkdown(
      [
        "- Check the lamp.",
        "- Hand it over.",
        "  - Sign the log.",
        "",
        "| a |",
        "| - |",
        "| 1 |",
        "",
      ].join("\n"),
    );
    // `.measured li[data-short]` keeps an item whole on paper: the mark has
    // to reach the drawn element, or the rule is lost without a word.
    const items = Array.from(container.querySelectorAll("li"));
    expect(items.map((li) => li.hasAttribute("data-short"))).toEqual([
      true,
      false,
      true,
    ]);
    expect(container.querySelector("table")).toHaveAttribute("data-short", "");
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

  it("shows the display text of a wikilink as the link", async () => {
    // The real resolver, so the label the page shows is the one it builds.
    const resolve = buildWikilinkResolver(
      readEngramDetail(
        {
          domain: "eng",
          permalink: "home",
          title: "Home",
          content: "",
          links: [
            {
              line: 1,
              resolved: true,
              target: { domain: null, target: "Alpha" },
            },
          ],
          relations: [],
        },
        "eng",
        "home",
      ),
      readGraph({
        nodes: [
          {
            id: 1,
            domain: "eng",
            permalink: "alpha",
            title: "Alpha",
            status: "stable",
            type: "engram",
          },
        ],
        edges: [],
        truncated: false,
      }),
    );
    await renderMarkdown("See [[Alpha|the first one]] for the rest.", resolve);

    const link = screen.getByRole("link", { name: "the first one" });
    expect(link).toHaveAttribute("href", "/d/eng/e/alpha");
    expect(screen.queryByText(/\[\[Alpha/)).toBeNull();
  });

  it("leaves an unresolved wikilink with display text as written", async () => {
    await renderMarkdown("See [[Ghost|the ghost]] for the rest.", () => ({
      kind: "unresolved",
    }));

    expect(screen.getByTitle("not resolved")).toHaveTextContent(
      "[[Ghost|the ghost]]",
    );
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("opens the engram a relative note link lands on", async () => {
    // The resolver is asked with the path the server parsed: decoded and
    // without the fragment, which the route keeps.
    await renderMarkdown(
      "See [the pump](../pumps/p%2012.md#start) and [a file](notes.txt).",
      (inner) =>
        inner === "../pumps/p 12.md"
          ? { kind: "resolved", href: "/d/plant/e/kit/pump", label: "P-12" }
          : null,
    );

    expect(screen.getByRole("link", { name: "the pump" })).toHaveAttribute(
      "href",
      "/d/plant/e/kit/pump#start",
    );
    expect(screen.getByRole("link", { name: "a file" })).not.toHaveAttribute(
      "href",
      expect.stringContaining("/d/plant/e/"),
    );
  });

  it("leaves a relative note link as written when nothing resolves it", async () => {
    await renderMarkdown("See [the pump](../pumps/p-12.md).");
    expect(
      screen.getByRole("link", { name: "the pump" }).getAttribute("href"),
    ).toMatch(/pumps\/p-12\.md$/);
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
    // The line itself stays beside it, and its tag is an accent chip.
    expect(
      screen.getByText(/An unsigned handover is not a handover/),
    ).toBeVisible();
    const tag = screen.getByText("#protocol");
    expect(tag.className).toContain("bg-accent-100");
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

  it("a fragment link stays on the page it is written on", async () => {
    // The anchor resolves against the page address the browser shows, not
    // against the router's entry, so the test moves the window itself.
    window.history.pushState({}, "", "/d/eng/e/notes/a");
    try {
      await renderMarkdown("[top](#observations)");

      expect(
        screen.getByRole("link", { name: "top" }).getAttribute("href"),
      ).toBe("/d/eng/e/notes/a#observations");
    } finally {
      window.history.pushState({}, "", "/");
    }
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
    expect(region.textContent).toBe("");

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
      expect(region.textContent).toBe("");
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
      expect(
        screen.queryByRole("status", { name: "Loading the text" }),
      ).toBeNull();
    });

    await userEvent.click(
      screen.getAllByRole("button", { name: "Link to this section" })[0]!,
    );

    // The fragment is added to where the reader stands rather than replacing
    // it: a page opened with a query string is still that page afterwards.
    expect(screen.getByTestId("location")).toHaveTextContent("?tab=graph#api");
  });
});

describe("callouts", () => {
  it("renders a callout as a named note with its title", async () => {
    await renderMarkdown(
      "> [!WARNING] Data loss\n> The restore drops writes.\n",
    );
    const note = screen.getByRole("note", { name: "Warning: Data loss" });
    expect(note).toHaveTextContent("The restore drops writes.");
    expect(note).not.toHaveTextContent("[!WARNING]");
  });

  it("names a callout without a title by its kind, and keeps a title's emphasis out of the body", async () => {
    await renderMarkdown(
      "> [!TIP]\n> Run it twice.\n\n> [!IMPORTANT] Read **this**\n> Then go.\n",
    );
    expect(screen.getByRole("note", { name: "Tip" })).toHaveTextContent(
      "Run it twice.",
    );
    const important = screen.getByRole("note", {
      name: "Important: Read this",
    });
    expect(important.querySelector("p")).toHaveTextContent(/^Then go\.$/);
  });

  it("folds a callout closed, and an unknown kind stays a plain quote", async () => {
    const { container } = await renderMarkdown(
      "> [!DETAILS]- Rollback steps\n> 1. Stop the job\n\n> [!DANGER] Hot\n> body\n",
    );
    const details = container.querySelector("details");
    expect(details).not.toBeNull();
    expect(details?.open).toBe(false);
    expect(screen.getByText("Rollback steps")).toBeVisible();
    expect(container.querySelector("blockquote")).toHaveTextContent(
      "[!DANGER] Hot",
    );
  });

  it("folds a callout open with a plus", async () => {
    const { container } = await renderMarkdown(
      "> [!NOTE]+ More\n> Shown at first.\n",
    );
    expect(container.querySelector("details")?.open).toBe(true);
    expect(screen.getByText("Shown at first.")).toBeVisible();
  });

  it("keeps source spans inside a callout", async () => {
    const source = "> [!NOTE] Heads up\n> Deploys run **at** night.\n";
    const { container } = await renderSpans(source);
    const stamped = expectSpansMatch(container, source);
    // The words after the head share its text node, whose start the
    // transform moves past the head line and the next line's `> `.
    const deploys = stamped.find((span) =>
      span.textContent?.startsWith("Deploys run"),
    );
    expect(deploys?.getAttribute("data-src")).toBe(
      String(source.indexOf("Deploys run")),
    );
  });

  it("keeps a second line's words out of the title when emphasis spans the break", async () => {
    await renderMarkdown("> [!NOTE] Heads **up\n> now** please\n");
    const note = screen.getByRole("note", { name: "Note: Heads" });
    expect(note.querySelector("p strong")).toHaveTextContent(/^up\s+now$/);
  });

  it("leaves an escaped marker a plain quote", async () => {
    const { container } = await renderMarkdown("> \\[!NOTE] Not a callout\n");
    expect(screen.queryByRole("note")).toBeNull();
    expect(container.querySelector("blockquote")).toHaveTextContent(
      "[!NOTE] Not a callout",
    );
  });

  it("does not reopen a fold the reader closed when the text changes", async () => {
    const source = "> [!DETAILS]- Steps\n>\n> ### Restore\n>\n> Run it.\n";
    const anchors = { pageUrl: "https://kb.example.com/d/eng/e/alpha" };
    const tree = (text: string) => (
      <MemoryRouter initialEntries={["/#restore"]}>
        <ThemeProvider>
          <Markdown source={text} anchors={anchors} />
        </ThemeProvider>
      </MemoryRouter>
    );
    const { container, rerender } = render(tree(source));
    await waitFor(() => {
      expect(container.querySelector("details")?.open).toBe(true);
    });
    const details = container.querySelector("details")!;
    details.open = false;
    rerender(tree(source.replace("Run it.", "Run it twice.")));
    await screen.findByText("Run it twice.");
    expect(container.querySelector("details")?.open).toBe(false);
  });

  it("opens a folded callout when the location points inside it", async () => {
    const source = "> [!DETAILS]- Steps\n>\n> ### Restore\n>\n> Run it.\n";
    const anchors = { pageUrl: "https://kb.example.com/d/eng/e/alpha" };
    const viaRouter = await renderMarkdown(
      source,
      undefined,
      undefined,
      undefined,
      anchors,
      "/#restore",
    );
    expect(viaRouter.container.querySelector("details")?.open).toBe(true);
    viaRouter.unmount();

    const { container } = await renderMarkdown(
      source,
      undefined,
      undefined,
      undefined,
      anchors,
    );
    const details = container.querySelector("details");
    expect(details?.open).toBe(false);
    try {
      window.location.hash = "#restore";
      await waitFor(() => {
        expect(details?.open).toBe(true);
      });
    } finally {
      window.history.replaceState(null, "", window.location.pathname);
    }
  });

  it("opens the fold for the same fragment on the next document", async () => {
    const pages: Record<string, string> = {
      "/e/a": "> [!DETAILS]- Steps\n>\n> ### Restore\n>\n> Run A.\n",
      "/e/b": "> [!DETAILS]- Steps\n>\n> ### Restore\n>\n> Run B.\n",
    };
    function Page() {
      const { pathname } = useLocation();
      return (
        <>
          <Link to="/e/b#restore">Next page</Link>
          <Markdown
            source={pages[pathname] ?? ""}
            anchors={{ pageUrl: `https://kb.example.com/d/eng${pathname}` }}
          />
        </>
      );
    }
    const { container } = render(
      <MemoryRouter initialEntries={["/e/a#restore"]}>
        <ThemeProvider>
          <Page />
        </ThemeProvider>
      </MemoryRouter>,
    );
    await waitFor(() => {
      expect(container.querySelector("details")?.open).toBe(true);
    });
    // The reader closed it again before following the link.
    container.querySelector("details")!.open = false;
    await userEvent.click(screen.getByRole("link", { name: "Next page" }));
    await screen.findByText("Run B.");
    await waitFor(() => {
      expect(container.querySelector("details")?.open).toBe(true);
    });
  });

  it("opens the next document's fold when its text arrives after the path", async () => {
    // The engram page's way: the router names the next page at once, and the
    // last page's text stays on screen until the next one has loaded.
    const pages: Record<string, string> = {
      "/e/a": "> [!DETAILS]- Steps\n>\n> ### Restore\n>\n> Run A.\n",
      "/e/b": "> [!DETAILS]- Steps\n>\n> ### Restore\n>\n> Run B.\n",
    };
    function Page() {
      const { pathname } = useLocation();
      const [shown, setShown] = useState(pathname);
      useEffect(() => {
        const timer = setTimeout(() => {
          setShown(pathname);
        }, 50);
        return () => {
          clearTimeout(timer);
        };
      }, [pathname]);
      return (
        <>
          <Link to="/e/b#restore">Next page</Link>
          <Markdown
            source={pages[shown] ?? ""}
            anchors={{ pageUrl: `https://kb.example.com/d/eng${shown}` }}
          />
        </>
      );
    }
    const { container } = render(
      <MemoryRouter initialEntries={["/e/a#restore"]}>
        <ThemeProvider>
          <Page />
        </ThemeProvider>
      </MemoryRouter>,
    );
    await waitFor(() => {
      expect(container.querySelector("details")?.open).toBe(true);
    });
    container.querySelector("details")!.open = false;
    await userEvent.click(screen.getByRole("link", { name: "Next page" }));
    // Still the first page: the fold the reader closed stays closed.
    expect(screen.getByText("Run A.")).toBeInTheDocument();
    expect(container.querySelector("details")?.open).toBe(false);
    await screen.findByText("Run B.");
    await waitFor(() => {
      expect(container.querySelector("details")?.open).toBe(true);
    });
  });

  it("opens a folded callout when something inside it is revealed", async () => {
    const { container } = await renderMarkdown(
      "> [!CAUTION]- Old data\n> The quote is in here.\n",
    );
    const details = container.querySelector("details")!;
    expect(details.open).toBe(false);
    const words = screen.getByText("The quote is in here.");
    act(() => {
      words.dispatchEvent(
        new CustomEvent(REVEAL_EVENT, {
          bubbles: true,
          detail: { target: words },
        }),
      );
    });
    expect(details.open).toBe(true);
  });
});

describe("status labels and other directives", () => {
  it("renders a status label as its text in a coloured pill", async () => {
    await renderMarkdown(
      "| Item | State |\n| --- | --- |\n| Login | :status[In review]{blue} |\n",
    );
    const label = screen.getByText("In review");
    expect(label.closest("[data-status]")?.getAttribute("data-status")).toBe(
      "blue",
    );
  });

  it("leaves a label of nothing but spaces as its text", async () => {
    const { container } = await renderMarkdown("Now :status[ ]{blue} here.\n");
    expect(container.querySelector("[data-status]")).toBeNull();
    expect(container).toHaveTextContent("Now :status[ ]{blue} here.");
  });

  it("leaves every other colon in prose exactly as typed", async () => {
    const lines = [
      "Meet at 10:30, see note:this and https://example.com/a:b.",
      "A :word[here] and ::leaf and :::box stay text.",
      ":status without brackets stays text.",
      ":status[Odd]{orange} falls back to grey.",
    ];
    await renderMarkdown(lines.join("\n\n"));
    expect(screen.getByText(/Meet at 10:30, see note:this/)).toBeVisible();
    expect(
      screen.getByText(/A :word\[here\] and ::leaf and :::box stay text\./),
    ).toBeVisible();
    expect(
      screen.getByText(/:status without brackets stays text\./),
    ).toBeVisible();
    expect(
      screen
        .getByText("Odd")
        .closest("[data-status]")
        ?.getAttribute("data-status"),
    ).toBe("grey");
  });

  it("names a colour only when the braces hold exactly one palette word", async () => {
    await renderMarkdown(
      ":status[A]{orange blue} :status[B]{blue green} :status[C]{red=1} :status[D]{ Purple }\n",
    );
    const colour = (text: string) =>
      screen
        .getByText(text)
        .closest("[data-status]")
        ?.getAttribute("data-status");
    expect(colour("A")).toBe("grey");
    expect(colour("B")).toBe("grey");
    expect(colour("C")).toBe("grey");
    expect(colour("D")).toBe("purple");
  });

  it("keeps a status label with no words as text", async () => {
    const { container } = await renderMarkdown("Empty :status[]{blue} here.\n");
    expect(container.querySelector("[data-status]")).toBeNull();
    expect(screen.getByText("Empty :status[]{blue} here.")).toBeVisible();
  });

  it("renders inline markdown beside a word:name exactly as without directives", async () => {
    const { container } = await renderMarkdown(
      [
        "See foo:bar[link](https://x.example) now.",
        "",
        "A term:x[^1] with a note.",
        "",
        "Some a:b[**x**] y.",
        "",
        "Ref a:b[t][ref] here.",
        "",
        "![a:b[c]](x.png)",
        "",
        "[ref]: https://r.example",
        "[^1]: The note.",
      ].join("\n"),
    );
    expect(screen.getByRole("link", { name: "link" })).toHaveAttribute(
      "href",
      "https://x.example",
    );
    expect(
      container.querySelector("section[data-footnotes]"),
    ).toHaveTextContent("The note.");
    expect(container.querySelector("sup a")).not.toBeNull();
    expect(container.querySelector("strong")).toHaveTextContent(/^x$/);
    expect(screen.getByRole("link", { name: "t" })).toHaveAttribute(
      "href",
      "https://r.example",
    );
    expect(container.querySelector("img")?.getAttribute("alt")).toBe("a:b[c]");
  });

  it("keeps a ::name line inside a paragraph and a list item", async () => {
    const { container } = await renderMarkdown(
      "First line\n::x\nlast line\n\n- one\n::y\n- two\n",
    );
    const paragraph = container.querySelector("p");
    expect(paragraph).toHaveTextContent("First line ::x last line");
    expect(container.querySelectorAll("ul")).toHaveLength(1);
    const items = screen.getAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent("one ::y");
  });

  it("reads the colour the way the core lint does", async () => {
    await renderMarkdown(
      ":status[Done]{Green} and :status[Open] and :status[Gone]{.red}\n",
    );
    const colour = (text: string) =>
      screen
        .getByText(text)
        .closest("[data-status]")
        ?.getAttribute("data-status");
    expect(colour("Done")).toBe("green");
    expect(colour("Open")).toBe("grey");
    expect(colour("Gone")).toBe("grey");
  });

  it("keeps a wikilink that names its domain one link", async () => {
    await renderMarkdown("See [[moon:Crater Base]] now.", (inner) =>
      inner === "moon:Crater Base"
        ? { kind: "resolved", href: "/d/moon/e/crater", label: "Crater Base" }
        : null,
    );
    expect(screen.getByRole("link", { name: "Crater Base" })).toBeVisible();
  });

  it("draws a swatch for a colour code and not for other code", async () => {
    const { container } = await renderMarkdown(
      "Brand `#0969DA`, and `rgb(9, 105, 218)`, not `#define`.\n",
    );
    expect(container.querySelectorAll("[data-swatch]")).toHaveLength(2);
  });

  it("draws a swatch for a colour code on a page with source spans", async () => {
    const { container } = await renderSpans("Brand `#0969DA` here.\n");
    expect(container.querySelectorAll("[data-swatch]")).toHaveLength(1);
  });

  it("keeps a line-start container or leaf directive as text around rendered content", async () => {
    const { container } = await renderMarkdown(
      [
        ":::note",
        "Some **bold** text",
        ":::",
        "",
        "::toc",
        "",
        ":::warning",
        "Never closed **here**",
        "",
        "## After",
      ].join("\n"),
    );
    expect(container.querySelectorAll("strong")).toHaveLength(2);
    for (const marker of [":::note", "::toc", ":::warning"]) {
      expect(container).toHaveTextContent(marker);
    }
    expect(
      screen.getByRole("heading", { name: "After", level: 2 }),
    ).toBeVisible();
  });

  it("keeps a heading inside an unclosed container reachable by its anchor", async () => {
    await renderMarkdown(
      ":::warning\nNever closed\n\n## After\n\n- one\n- two\n",
      undefined,
      undefined,
      undefined,
      { pageUrl: "/d/x" },
    );
    const heading = screen.getByRole("heading", { name: /After/, level: 2 });
    expect(heading.id).toBe("after");
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
  });

  it("stamps directive text and a status label at their source offsets", async () => {
    const source = [
      "Login is :status[In review]{blue} and :word[x]{y} today.",
      "",
      ":::note[Title]",
      "Inside **here**",
      ":::",
      "",
      "::toc",
    ].join("\n");
    const { container } = await renderSpans(source);
    const stamped = expectSpansMatch(container, source);
    const texts = stamped.map((span) => span.textContent);
    expect(texts).toContain("In review");
    expect(container).toHaveTextContent(":word[x]{y} today.");
    expect(container).toHaveTextContent(":::note[Title]");
    expect(container).toHaveTextContent("::toc");
  });
});

describe("math", () => {
  it("renders the safe math forms and leaves dollar amounts alone", async () => {
    const { container } = await renderMarkdown(
      [
        "The loss is $`L = -\\sum y`$ here.",
        "",
        "$$",
        "E = mc^2",
        "$$",
        "",
        "```math",
        "a^2 + b^2",
        "```",
        "",
        "It costs $5 and $10, set $HOME.",
      ].join("\n"),
    );
    await waitFor(() =>
      expect(container.querySelectorAll(".katex")).toHaveLength(3),
    );
    expect(screen.getByText("It costs $5 and $10, set $HOME.")).toBeVisible();
  });

  it("keeps the words around inline math and never reads a single dollar pair", async () => {
    const { container } = await renderMarkdown(
      "Before $`x^2`$ after, and $a$ stays $b$ text.",
    );
    await waitFor(() =>
      expect(container.querySelectorAll(".katex")).toHaveLength(1),
    );
    expect(container).toHaveTextContent("Before");
    expect(container).toHaveTextContent("$a$ stays $b$ text.");
  });
});

describe("math edges", () => {
  it("sets two inline formulas that touch", async () => {
    const { container } = await renderMarkdown("Both $`x`$$`y`$ here.");
    await waitFor(() =>
      expect(container.querySelectorAll(".katex")).toHaveLength(2),
    );
    expect(container).not.toHaveTextContent("$");
  });

  it("reads a dollar after an escaped backslash as a math opener and an escaped dollar as text", async () => {
    const { container } = await renderMarkdown(
      "Path \\\\$`x`$ and price \\$`y`$ end.",
    );
    await waitFor(() =>
      expect(container.querySelectorAll(".katex")).toHaveLength(1),
    );
    expect(container).toHaveTextContent("price $");
  });

  it("keeps the TeX on screen when the setter throws", async () => {
    const katex = (await import("katex")).default;
    const render = vi.spyOn(katex, "render").mockImplementation(() => {
      throw new Error("boom");
    });
    try {
      const { container } = await renderMarkdown("Broken $`\\frac{1`$ here.");
      await waitFor(() => expect(container).toHaveTextContent("\\frac{1"));
    } finally {
      render.mockRestore();
    }
  });
});

describe("footnotes and code copy", () => {
  it("styles footnotes at the end with links both ways", async () => {
    const { container } = await renderMarkdown("Claim.[^1]\n\n[^1]: Source.\n");
    const ref = container.querySelector("sup a");
    expect(ref?.getAttribute("href")).toMatch(/#user-content-.*fn-1$/);
    const notes = container.querySelector("section[data-footnotes]");
    expect(notes).toHaveTextContent("Source.");
    expect(notes).toHaveClass("border-t", "border-hair", "text-[13px]");
    expect(container.querySelector("sup")).toHaveClass("text-[0.75em]");
    const label = notes?.querySelector("h2");
    expect(label).toHaveTextContent("Footnotes");
    expect(label).not.toHaveClass("text-prose-h2");
    const back = notes?.querySelector('a[href*="fnref-1"]');
    expect(back?.getAttribute("href")).toMatch(/#user-content-.*fnref-1$/);
    for (const link of [ref, back]) {
      const id = link?.getAttribute("href")?.split("#")[1] ?? "";
      expect(container.querySelector(`[id="${id}"]`)).not.toBeNull();
    }
  });

  it("keeps a heading the writer called Footnote label a heading", async () => {
    const { container } = await renderMarkdown(
      "## Footnote label\n\nClaim.[^1]\n\n[^1]: Source.\n",
      undefined,
      undefined,
      undefined,
      { pageUrl: "https://kb.example.com/d/eng/e/alpha" },
    );
    const own = screen.getByRole("heading", {
      level: 2,
      name: "Footnote label",
    });
    expect(own).toHaveClass("text-prose-h2");
    expect(
      own.querySelector('button[aria-label="Link to this section"]') ??
        within(own).queryByRole("button", { name: "Link to this section" }),
    ).not.toBeNull();
    const label = container.querySelector("section[data-footnotes] h2");
    expect(label).toHaveTextContent("Footnotes");
    expect(label).not.toHaveClass("text-prose-h2");
  });

  it("keeps footnote ids unique across documents on one page", async () => {
    const { container } = render(
      <MemoryRouter>
        <ThemeProvider>
          <Markdown source={"One.[^1]\n\n[^1]: First.\n"} />
          <Markdown source={"Two.[^1]\n\n[^1]: Second.\n"} />
        </ThemeProvider>
      </MemoryRouter>,
    );
    await waitFor(() => {
      expect(
        container.querySelectorAll("section[data-footnotes]"),
      ).toHaveLength(2);
    });
    const ids = Array.from(container.querySelectorAll("[id]")).map((e) => e.id);
    expect(new Set(ids).size).toBe(ids.length);
    const refs = Array.from(container.querySelectorAll("sup a"));
    const notes = Array.from(
      container.querySelectorAll("section[data-footnotes]"),
    );
    expect(refs).toHaveLength(2);
    refs.forEach((ref, i) => {
      const id = ref.getAttribute("href")?.split("#")[1] ?? "";
      expect(notes[i]?.querySelector(`[id="${id}"]`)).not.toBeNull();
    });
  });

  it("copies a code block", async () => {
    const write = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: write },
    });
    await renderMarkdown("```sh\ncargo test\n```\n");
    await userEvent.click(screen.getByRole("button", { name: "Copy code" }));
    expect(write).toHaveBeenCalledWith("cargo test");
    expect(await screen.findByText("Copied")).toBeVisible();
  });

  it("puts no copy button on a diagram or a formula", async () => {
    await renderMarkdown(
      "```mermaid\ngraph TD; A-->B\n```\n\n```math\nx^2\n```\n",
    );
    expect(screen.queryByRole("button", { name: "Copy code" })).toBeNull();
  });
});

describe("task boxes", () => {
  /** Render with the tick props a reading page passes. */
  async function renderTasks(
    source: string,
    tick: {
      onTick?: (offset: number, next: "[ ]" | "[x]") => void;
      canTick?: boolean | string;
      foldTitle?: string;
      sourceSpans?: boolean;
    },
  ) {
    const result = render(
      <MemoryRouter>
        <ThemeProvider>
          <Markdown source={source} {...tick} />
        </ThemeProvider>
      </MemoryRouter>,
    );
    await waitFor(() => {
      expect(
        screen.queryByRole("status", { name: "Loading the text" }),
      ).toBeNull();
    });
    return result;
  }

  it("draws a not-applicable item as a struck box that is no checkbox", async () => {
    const onTick = vi.fn<(offset: number, next: "[ ]" | "[x]") => void>();
    await renderTasks("- [~] skipped\n- [ ] open\n", { onTick, canTick: true });
    const box = screen.getByRole("img", { name: "Not applicable: skipped" });
    expect(box).toBeVisible();
    expect(screen.queryByRole("checkbox", { name: /skipped/ })).toBeNull();
    expect(screen.queryByText(/\[~\]/)).toBeNull();
    expect(screen.getByText("skipped")).toBeVisible();
    await userEvent.click(box);
    expect(onTick).not.toHaveBeenCalled();
  });

  it("names a box by the words a reader sees, without wikilink brackets", async () => {
    await renderTasks(
      "- [ ] read [[Deploy Guide]] and [[ops:Rollback|the rollback]]\n",
      { onTick: vi.fn(), canTick: true },
    );
    expect(
      screen.getByRole("checkbox", {
        name: "read Deploy Guide and the rollback",
      }),
    ).toBeVisible();
  });

  it("ticks an open box through onTick with the offset of its bracket", async () => {
    const onTick = vi.fn<(offset: number, next: "[ ]" | "[x]") => void>();
    await renderTasks("- [ ] a\n- [x] b\n", { onTick, canTick: true });
    const open = screen.getByRole("checkbox", { name: "a" });
    expect(open).toBeEnabled();
    expect(open).not.toBeChecked();
    await userEvent.click(open);
    expect(onTick).toHaveBeenCalledWith(2, "[x]");
    await userEvent.click(screen.getByRole("checkbox", { name: "b" }));
    expect(onTick).toHaveBeenLastCalledWith(10, "[ ]");
  });

  it("counts the offset into the whole file, frontmatter and folded title included", async () => {
    const source = [
      "---",
      "title: Alpha",
      "---",
      "",
      "# Alpha",
      "",
      "- [ ] first",
      "  - [ ] nested",
      "10. [ ] numbered",
      "",
    ].join("\n");
    const onTick = vi.fn<(offset: number, next: "[ ]" | "[x]") => void>();
    await renderTasks(source, { onTick, canTick: true, foldTitle: "Alpha" });
    await userEvent.click(screen.getByRole("checkbox", { name: "first" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "nested" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "numbered" }));
    expect(onTick.mock.calls.map(([at]) => source.slice(at, at + 4))).toEqual([
      "[ ] ",
      "[ ] ",
      "[ ] ",
    ]);
    expect(onTick.mock.calls.map(([at]) => at)).toEqual([
      source.indexOf("[ ] first"),
      source.indexOf("[ ] nested"),
      source.indexOf("[ ] numbered"),
    ]);
  });

  it("finds the box in a loose list and in a quote", async () => {
    const source = "- [ ] loose one\n\n- [x] loose two\n\n> - [ ] quoted\n";
    const onTick = vi.fn<(offset: number, next: "[ ]" | "[x]") => void>();
    await renderTasks(source, { onTick, canTick: true });
    await userEvent.click(screen.getByRole("checkbox", { name: "loose one" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "loose two" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "quoted" }));
    expect(onTick.mock.calls).toEqual([
      [source.indexOf("[ ] loose"), "[x]"],
      [source.indexOf("[x] loose"), "[ ]"],
      [source.indexOf("[ ] quoted"), "[x]"],
    ]);
  });

  it("disables the box with the reason when the reader may not change the page", async () => {
    const onTick = vi.fn<(offset: number, next: "[ ]" | "[x]") => void>();
    await renderTasks("- [ ] a\n", {
      onTick,
      canTick: "You can read this page but not change it",
    });
    const box = screen.getByRole("checkbox", { name: "a" });
    expect(box).toBeDisabled();
    expect(box.closest("[title]")).toHaveAttribute(
      "title",
      "You can read this page but not change it",
    );
    expect(box).toHaveAccessibleDescription(
      "You can read this page but not change it",
    );
  });

  it("keeps source spans reading the source on a not-applicable item", async () => {
    const source = "- [~] skipped here\n";
    const { container } = render(
      <MemoryRouter>
        <ThemeProvider>
          <Markdown source={source} sourceSpans />
        </ThemeProvider>
      </MemoryRouter>,
    );
    await waitFor(() => {
      expect(screen.getByText("skipped here")).toBeVisible();
    });
    const stamped = container.querySelectorAll("[data-src]");
    expect(stamped.length).toBeGreaterThan(0);
    for (const span of stamped) {
      const at = Number(span.getAttribute("data-src"));
      expect(source.slice(at, at + (span.textContent ?? "").length)).toBe(
        span.textContent,
      );
    }
  });
});
