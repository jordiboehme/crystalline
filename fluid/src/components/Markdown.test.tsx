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
async function renderMarkdown(
  source: string,
  wikilinks?: WikilinkResolver,
  foldTitle?: string,
) {
  const result = render(
    <MemoryRouter>
      <ThemeProvider>
        <Markdown
          source={source}
          {...(wikilinks ? { wikilinks } : {})}
          {...(foldTitle === undefined ? {} : { foldTitle })}
        />
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
});
