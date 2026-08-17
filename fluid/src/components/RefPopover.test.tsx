/**
 * The counted chip: what it costs before it is opened, what it does once it is,
 * and the two ways a list behind a search box goes wrong.
 *
 * The fetcher is a plain function here rather than a mocked transport, which is
 * the point of the primitive: it knows nothing about the API, so a test that
 * hands it deferred promises can pin the ordering rules exactly.
 */

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { describe, expect, it, vi } from "vitest";

import { RefPopover } from "./RefPopover";
import type { RefPageResult } from "./RefPopover";

/** A page of `count` rows, named after the filter that asked for them. */
function pageOf(
  count: number,
  total: number,
  page: number,
  label = "Row",
): RefPageResult {
  return {
    total,
    rows: Array.from({ length: count }, (_, index) => ({
      key: `${String(page)}:${String(index)}`,
      title: `${label} ${String(page)}.${String(index)}`,
      href: `/d/eng/e/row-${String(page)}-${String(index)}`,
      detail: `eng / row-${String(page)}-${String(index)}.md`,
    })),
    hasMore: page * count < total,
  };
}

function mount(fetchPage: (page: number, q: string) => Promise<RefPageResult>) {
  return render(
    <MemoryRouter>
      <RefPopover label="relates_to" count={42} fetchPage={fetchPage} />
    </MemoryRouter>,
  );
}

/** The chip, which is a real button. */
function chip(): HTMLElement {
  return screen.getByRole("button", { name: /relates_to/ });
}

describe("RefPopover", () => {
  it("shows the label and the count, and asks for nothing until it is opened", () => {
    const fetchPage = vi.fn(() => Promise.resolve(pageOf(2, 42, 1)));

    mount(fetchPage);

    const trigger = chip();
    expect(trigger).toHaveTextContent("relates_to");
    expect(trigger).toHaveTextContent("42");
    // The whole point of a counted chip: a thousand references cost one
    // number and no request until somebody wants them.
    expect(fetchPage).not.toHaveBeenCalled();
  });

  it("loads the first page when it opens", async () => {
    const fetchPage = vi.fn(() => Promise.resolve(pageOf(2, 42, 1)));

    mount(fetchPage);
    await userEvent.click(chip());

    expect(
      await screen.findByRole("link", { name: /Row 1\.0/ }),
    ).toHaveAttribute("href", "/d/eng/e/row-1-0");
    expect(fetchPage).toHaveBeenCalledTimes(1);
    expect(fetchPage).toHaveBeenCalledWith(1, "");
    // The total is of the whole set, not of the page on screen.
    expect(screen.getByText("42 references")).toBeInTheDocument();
  });

  it("filters on a settled query rather than on every keystroke", async () => {
    const fetchPage = vi.fn((page: number, q: string) =>
      Promise.resolve(
        pageOf(1, q === "" ? 42 : 1, page, q === "" ? "Row" : "Hit"),
      ),
    );

    mount(fetchPage);
    await userEvent.click(chip());
    await screen.findByRole("link", { name: /Row 1\.0/ });

    await userEvent.type(screen.getByRole("searchbox"), "beta");

    await waitFor(() => {
      expect(
        screen.getByRole("link", { name: /Hit 1\.0/ }),
      ).toBeInTheDocument();
    });
    const queries = fetchPage.mock.calls.map(([, q]) => q);
    // The opening request and one settled query, not one per letter.
    expect(queries).toEqual(["", "beta"]);
  });

  it("shows the answer to the query that is in the box, whatever order they arrive in", async () => {
    let releaseFirst = (result: RefPageResult) => {
      void result;
    };
    const fetchPage = vi.fn((page: number, q: string) => {
      if (q === "a") {
        return new Promise<RefPageResult>((resolve) => {
          releaseFirst = resolve;
        });
      }
      return Promise.resolve(pageOf(1, 1, page, q === "" ? "Row" : "Second"));
    });

    mount(fetchPage);
    await userEvent.click(chip());
    await screen.findByRole("link", { name: /Row 1\.0/ });

    // Two filters in a row: the first one's answer is still in flight when the
    // second one lands, and then arrives late.
    await userEvent.type(screen.getByRole("searchbox"), "a");
    await waitFor(() => {
      expect(fetchPage).toHaveBeenCalledWith(1, "a");
    });
    await userEvent.type(screen.getByRole("searchbox"), "b");
    await screen.findByRole("link", { name: /Second 1\.0/ });

    releaseFirst(pageOf(1, 1, 1, "Stale"));

    // The late answer to a query nobody is asking any more never reaches the
    // screen: what is listed matches what is in the box.
    await waitFor(() => {
      expect(screen.queryByRole("link", { name: /Stale/ })).toBeNull();
    });
    expect(
      screen.getByRole("link", { name: /Second 1\.0/ }),
    ).toBeInTheDocument();
  });

  it("adds a page at a time, and stops offering more when there is none", async () => {
    const fetchPage = vi.fn((page: number) =>
      Promise.resolve(pageOf(2, 4, page)),
    );

    mount(fetchPage);
    await userEvent.click(chip());
    await screen.findByRole("link", { name: /Row 1\.0/ });

    await userEvent.click(screen.getByRole("button", { name: "Load more" }));

    // The second page joins the first rather than replacing it.
    expect(await screen.findByRole("link", { name: /Row 2\.0/ })).toBeVisible();
    expect(screen.getByRole("link", { name: /Row 1\.0/ })).toBeVisible();
    expect(fetchPage).toHaveBeenLastCalledWith(2, "");
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
    });
  });

  it("says when nothing matches, without saying it failed", async () => {
    const fetchPage = vi.fn(() =>
      Promise.resolve({ total: 0, rows: [], hasMore: false }),
    );

    mount(fetchPage);
    await userEvent.click(chip());

    expect(await screen.findByText("Nothing here matches that.")).toBeVisible();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("prints what the fetcher said when it fails", async () => {
    const fetchPage = vi.fn(() =>
      Promise.reject(new Error("this account may not read eng")),
    );

    mount(fetchPage);
    await userEvent.click(chip());

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("this account may not read eng");
  });

  it("closes on Escape and gives the keyboard back to the chip", async () => {
    const fetchPage = vi.fn(() => Promise.resolve(pageOf(1, 1, 1)));

    mount(fetchPage);
    await userEvent.click(chip());
    await screen.findByRole("link", { name: /Row 1\.0/ });

    await userEvent.keyboard("{Escape}");

    await waitFor(() => {
      expect(screen.queryByRole("link", { name: /Row 1\.0/ })).toBeNull();
    });
    expect(chip()).toHaveFocus();
  });

  it("reopens on the unfiltered first page rather than on the last thing read", async () => {
    const fetchPage = vi.fn((page: number, q: string) =>
      Promise.resolve(pageOf(1, 1, page, q === "" ? "Row" : "Hit")),
    );

    mount(fetchPage);
    await userEvent.click(chip());
    await userEvent.type(screen.getByRole("searchbox"), "beta");
    await screen.findByRole("link", { name: /Hit/ });

    await userEvent.keyboard("{Escape}");
    await userEvent.click(chip());

    const box = await screen.findByRole("searchbox");
    expect(box).toHaveValue("");
    await screen.findByRole("link", { name: /Row/ });
    expect(fetchPage).toHaveBeenLastCalledWith(1, "");
  });
});
