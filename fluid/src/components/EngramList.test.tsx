/**
 * The list every screen that shows engrams is made of.
 *
 * Two things have to hold whatever fills it. It only draws the rows that are on
 * screen, and it asks for the next page when the reader reaches the bottom -
 * which is the part a virtualized list gets wrong silently, by never asking and
 * leaving a collection that looks complete at fifty rows. And a retired engram
 * fades rather than disappears: the fade is the whole design of the retired
 * statuses, and hiding one would be a lie about what the domain holds.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import type { ReactElement } from "react";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";

import type { EngramPage, EngramRow } from "../api/engrams";
import { ENGRAM_ROW_HEIGHT, EngramList } from "./EngramList";

/** The nominal viewport `src/test/setup.ts` gives every element. */
const VIEWPORT = 600;

/** One page's worth of rows, which is more than fits on the fake viewport. */
const PAGE_SIZE = 20;

function row(index: number, overrides: Partial<EngramRow> = {}): EngramRow {
  return {
    domain: "eng",
    permalink: `alpha-${String(index)}`,
    title: `Alpha ${String(index)}`,
    type: "engram",
    status: "stable",
    tags: ["eng"],
    kind: "engram",
    line: null,
    snippet: null,
    ...overrides,
  };
}

function pageOf(page: number, rows: EngramRow[], total: number): EngramPage {
  return {
    mode: "text",
    total,
    page,
    limit: PAGE_SIZE,
    count: rows.length,
    hits: rows,
  };
}

/** A page of twenty rows, numbered from where the page starts. */
function numberedPage(page: number, total: number): EngramPage {
  const start = (page - 1) * PAGE_SIZE;
  return pageOf(
    page,
    Array.from({ length: PAGE_SIZE }, (_, i) => row(start + i)),
    total,
  );
}

function mount(ui: ReactElement) {
  // Retries off: a test that stubs a failure wants the failure, not a wait.
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>{ui}</MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The scrolling box the virtualizer measures: the list's own parent. */
function scrollerOf(label: string): HTMLElement {
  const list = screen.getByRole("list", { name: label });
  const scroller = list.parentElement;
  if (!scroller) {
    throw new Error("the list has no scrolling parent");
  }
  return scroller;
}

/**
 * Put the reader at the bottom, the way a browser would say it: the offset the
 * virtualizer reads back off the element, then the event it listens for.
 */
function scrollToEnd(scroller: HTMLElement, rowCount: number) {
  act(() => {
    scroller.scrollTop = rowCount * ENGRAM_ROW_HEIGHT - VIEWPORT;
    scroller.dispatchEvent(new Event("scroll"));
  });
}

describe("the engram list", () => {
  it("renders the rows of the first page, linked to their engrams", async () => {
    const loadPage = vi.fn((page: number) =>
      Promise.resolve(numberedPage(page, 40)),
    );

    mount(
      <EngramList
        queryKey={["test", "rows"]}
        loadPage={loadPage}
        label="Engrams"
        emptyMessage="Nothing here."
      />,
    );

    expect(
      await screen.findByRole("link", { name: /Alpha 0/ }),
    ).toHaveAttribute("href", "/d/eng/e/alpha-0");
    // Virtualized: the twenty rows of the page are not all in the document,
    // only the ones the viewport plus its overscan reaches.
    expect(screen.queryByRole("link", { name: /Alpha 19/ })).toBeNull();
    expect(loadPage).toHaveBeenCalledTimes(1);
    expect(loadPage).toHaveBeenCalledWith(1);
  });

  it("asks for the next page when the reader reaches the bottom", async () => {
    const loadPage = vi.fn((page: number) =>
      Promise.resolve(numberedPage(page, 40)),
    );

    mount(
      <EngramList
        queryKey={["test", "paging"]}
        loadPage={loadPage}
        label="Engrams"
        emptyMessage="Nothing here."
      />,
    );
    await screen.findByRole("link", { name: /Alpha 0/ });
    expect(loadPage).toHaveBeenCalledTimes(1);

    scrollToEnd(scrollerOf("Engrams"), PAGE_SIZE);

    await waitFor(() => {
      expect(loadPage).toHaveBeenCalledWith(2);
    });
    expect(await screen.findByRole("link", { name: /Alpha 20/ })).toBeVisible();
  });

  it("stops asking once the last page is in", async () => {
    const loadPage = vi.fn((page: number) =>
      Promise.resolve(numberedPage(page, PAGE_SIZE)),
    );

    mount(
      <EngramList
        queryKey={["test", "one-page"]}
        loadPage={loadPage}
        label="Engrams"
        emptyMessage="Nothing here."
      />,
    );
    await screen.findByRole("link", { name: /Alpha 0/ });

    scrollToEnd(scrollerOf("Engrams"), PAGE_SIZE);

    // The envelope said twenty of twenty, so the bottom is the bottom.
    await waitFor(() => {
      expect(screen.getByText(/20 of 20/)).toBeVisible();
    });
    expect(loadPage).toHaveBeenCalledTimes(1);
  });

  it("fades a retired engram rather than hiding it", async () => {
    const loadPage = vi.fn(() =>
      Promise.resolve(
        pageOf(
          1,
          [
            row(0),
            row(1, { status: "superseded" }),
            row(2, { status: "deprecated" }),
            row(3, { status: "archived" }),
            row(4, { status: "legacy" }),
          ],
          5,
        ),
      ),
    );

    mount(
      <EngramList
        queryKey={["test", "retired"]}
        loadPage={loadPage}
        label="Engrams"
        emptyMessage="Nothing here."
      />,
    );

    const current = (await screen.findByRole("link", { name: /Alpha 0/ }))
      .parentElement;
    expect(current).not.toHaveClass("opacity-60");
    for (const [index, status] of [
      "superseded",
      "deprecated",
      "archived",
      "legacy",
    ].entries()) {
      const link = screen.getByRole("link", {
        name: new RegExp(`Alpha ${String(index + 1)}\\b`),
      });
      // Still on screen, and still saying what it is.
      expect(link).toHaveTextContent(status);
      expect(link.parentElement).toHaveClass("opacity-60");
    }
  });

  it("wears the one chip primitive: status filled by meaning, type neutral", async () => {
    mount(
      <EngramList
        queryKey={["test", "chips"]}
        loadPage={() => Promise.resolve(pageOf(1, [row(0)], 1))}
        label="Engrams"
        emptyMessage="Nothing here."
      />,
    );

    // The same mapping the details panel uses: a recognized lifecycle value
    // gets its semantic fill, and `type` is a fact rather than a judgement.
    expect((await screen.findByText("stable")).className).toContain("emerald");
    expect(screen.getByText("engram").className).toContain("slate");
  });

  it("reads a snippet as the sentence it is, not as its markdown source", async () => {
    mount(
      <EngramList
        queryKey={["test", "snippet"]}
        loadPage={() =>
          Promise.resolve(
            pageOf(
              1,
              [
                row(0, {
                  snippet: "## Relations - relates_to [[Lantern Protocol]]",
                }),
              ],
              1,
            ),
          )
        }
        label="Engrams"
        emptyMessage="Nothing here."
      />,
    );

    const link = await screen.findByRole("link", { name: /Alpha 0/ });
    expect(link).toHaveTextContent("Relations - relates_to Lantern Protocol");
    expect(link.textContent).not.toContain("[[");
    expect(link.textContent).not.toContain("##");
  });

  it("lets a caller's summary own the line that counts the rows", async () => {
    mount(
      <EngramList
        queryKey={["test", "summary"]}
        loadPage={() => Promise.resolve(numberedPage(1, PAGE_SIZE))}
        label="Engrams"
        emptyMessage="Nothing here."
        summary={(page, shown) => (
          <p>{`${String(shown)} of ${String(page.total)} results, ranked by text`}</p>
        )}
      />,
    );

    expect(
      await screen.findByText("20 of 20 results, ranked by text"),
    ).toBeVisible();
    // One line rather than two saying near enough the same thing.
    expect(screen.queryByText(/shown/)).toBeNull();
  });

  it("says so when there is nothing to list", async () => {
    mount(
      <EngramList
        queryKey={["test", "empty"]}
        loadPage={() => Promise.resolve(pageOf(1, [], 0))}
        label="Engrams"
        emptyMessage="No engram matches these filters."
      />,
    );

    expect(
      await screen.findByText("No engram matches these filters."),
    ).toBeVisible();
  });

  it("leads with the match: a well-tagged row still shows its snippet", async () => {
    const tags = Array.from({ length: 8 }, (_, i) => `tag-${String(i)}`);
    mount(
      <EngramList
        queryKey={["test", "many-tags"]}
        loadPage={() =>
          Promise.resolve(
            pageOf(
              1,
              [row(0, { tags, snippet: "The rule of thumb here." })],
              1,
            ),
          )
        }
        label="Engrams"
        emptyMessage="Nothing here."
      />,
    );

    const link = await screen.findByRole("link", { name: /Alpha 0/ });
    // The reason the row matched survives the tags rather than being pushed
    // off the end of the line by them.
    expect(link).toHaveTextContent("The rule of thumb here.");
    expect(screen.getByText("#tag-0")).toBeVisible();
    expect(screen.getByText("#tag-1")).toBeVisible();
    expect(screen.queryByText("#tag-7")).toBeNull();

    // What is hidden is said, and what it hides is one hover away.
    const overflow = screen.getByText("+6");
    expect(overflow).toHaveAttribute("title", tags.join(" "));
  });

  it("caps the row tags at two, and counts nothing it did not hide", async () => {
    mount(
      <EngramList
        queryKey={["test", "tag-cap"]}
        loadPage={() =>
          Promise.resolve(
            pageOf(
              1,
              [
                row(0, { tags: [] }),
                row(1, { tags: ["one", "two"] }),
                row(2, { tags: ["one", "two", "three"] }),
              ],
              3,
            ),
          )
        }
        label="Engrams"
        emptyMessage="Nothing here."
      />,
    );

    const none = await screen.findByRole("link", { name: /Alpha 0/ });
    expect(none.textContent).not.toContain("#");

    const two = screen.getByRole("link", { name: /Alpha 1/ });
    expect(two).toHaveTextContent("#one");
    expect(two).toHaveTextContent("#two");
    expect(two.textContent).not.toContain("+");

    const three = screen.getByRole("link", { name: /Alpha 2/ });
    expect(three).toHaveTextContent("#one");
    expect(three).toHaveTextContent("#two");
    expect(three.textContent).not.toContain("#three");
    expect(screen.getByText("+1")).toHaveAttribute("title", "one two three");
  });

  it("says which domain a hit lives in only when it is asked to", async () => {
    const page = () => Promise.resolve(pageOf(1, [row(0)], 1));

    const { unmount } = mount(
      <EngramList
        queryKey={["test", "no-domain"]}
        loadPage={page}
        label="Engrams"
        emptyMessage="Nothing here."
      />,
    );
    expect(await screen.findByText("alpha-0")).toBeVisible();
    expect(screen.queryByText("eng/alpha-0")).toBeNull();
    unmount();

    mount(
      <EngramList
        queryKey={["test", "with-domain"]}
        loadPage={page}
        label="Engrams"
        emptyMessage="Nothing here."
        showDomain
      />,
    );
    expect(await screen.findByText("eng/alpha-0")).toBeVisible();
  });

  it("marks the searched words in a title the way it marks them in a snippet", async () => {
    mount(
      <EngramList
        queryKey={["test", "title-mark"]}
        loadPage={() =>
          Promise.resolve(
            pageOf(1, [row(0, { title: "The lantern protocol" })], 1),
          )
        }
        label="Engrams"
        emptyMessage="Nothing here."
        highlight={["lantern"]}
      />,
    );

    const link = await screen.findByRole("link", { name: /lantern protocol/ });
    const marked = link.querySelector("mark");
    expect(marked).not.toBeNull();
    expect(marked).toHaveTextContent("lantern");
  });

  it("offers the way out a caller hands it, beside the empty message", async () => {
    mount(
      <EngramList
        queryKey={["test", "empty-actions"]}
        loadPage={() => Promise.resolve(pageOf(1, [], 0))}
        label="Engrams"
        emptyMessage="No engram matches these filters."
        emptyActions={<button type="button">Clear filters</button>}
      />,
    );

    await screen.findByText("No engram matches these filters.");
    expect(screen.getByRole("button", { name: "Clear filters" })).toBeVisible();
  });
});
