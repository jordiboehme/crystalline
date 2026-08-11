/**
 * The backlinks panel: what it asks the server for, when it asks at all, and
 * what one chip opens onto.
 *
 * The transport is mocked the way every other screen test here mocks it, so the
 * paths are part of what is pinned: the panel must ask for the summary once and
 * only fetch references when a chip is opened.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import { BacklinksPanel } from "./BacklinksPanel";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn() };
});

const apiMock = vi.mocked(api);

/** The relation summary the panel draws its chips from. */
function summary() {
  return {
    total: 9,
    page: 1,
    limit: 1,
    count: 1,
    types: [
      { rel: "relates_to", count: 7 },
      { rel: "links_to", count: 2 },
    ],
    hits: [],
  };
}

/** One page of references, in the engine's own shape. */
function referencePage(
  page: number,
  limit: number,
  total: number,
  rel: string,
  titles: string[],
) {
  return {
    total,
    page,
    limit,
    count: titles.length,
    types: summary().types,
    hits: titles.map((title) => ({
      domain: "eng",
      permalink: `notes/${title.toLowerCase()}`,
      title,
      path: `notes/${title.toLowerCase()}.md`,
      rel,
      status: "current",
    })),
  };
}

function mount(inboundCount = 9) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <BacklinksPanel
          domain="eng"
          permalink="alpha"
          inboundCount={inboundCount}
        />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** Every path asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => String(call[0]));
}

describe("BacklinksPanel", () => {
  beforeEach(() => {
    apiMock.mockReset();
  });

  it("draws one chip per relation type, counted across the whole index", async () => {
    apiMock.mockResolvedValue(summary());

    mount();

    const panel = await screen.findByRole("region", { name: "Backlinks" });
    const chip = await within(panel).findByRole("button", {
      name: /relates_to/,
    });
    expect(chip).toHaveTextContent("7");
    expect(
      within(panel).getByRole("button", { name: /links_to/ }),
    ).toHaveTextContent("2");
    // One request, and a small one: the panel is chips, not references.
    expect(requested()).toEqual(["/domains/eng/inbound/alpha?page=1&limit=1"]);
  });

  it("asks for nothing at all when the detail payload counted no references", async () => {
    apiMock.mockResolvedValue(summary());

    mount(0);

    expect(
      await screen.findByText("Nothing links here yet."),
    ).toBeInTheDocument();
    // The count is already exact and already on the page. A request to be told
    // zero a second time is a request on every engram nobody links to yet.
    expect(requested()).toEqual([]);
  });

  it("loads one relation's references when its chip is opened", async () => {
    apiMock.mockImplementation((path: string) => {
      if (path.includes("rel=relates_to")) {
        return Promise.resolve(
          referencePage(1, 20, 7, "relates_to", ["Beta", "Gamma"]),
        );
      }
      return Promise.resolve(summary());
    });

    mount();

    const panel = await screen.findByRole("region", { name: "Backlinks" });
    await userEvent.click(
      await within(panel).findByRole("button", { name: /relates_to/ }),
    );

    const link = await screen.findByRole("link", { name: /Beta/ });
    expect(link).toHaveAttribute("href", "/d/eng/e/notes/beta");
    expect(screen.getByText("7 references")).toBeInTheDocument();
    expect(requested()).toContain(
      "/domains/eng/inbound/alpha?page=1&limit=20&rel=relates_to",
    );
  });

  it("sends the filter to the server rather than filtering the page it holds", async () => {
    apiMock.mockImplementation((path: string) => {
      if (!path.includes("rel=")) {
        return Promise.resolve(summary());
      }
      return Promise.resolve(
        path.includes("q=gam")
          ? referencePage(1, 20, 1, "relates_to", ["Gamma"])
          : referencePage(1, 20, 7, "relates_to", ["Beta", "Gamma"]),
      );
    });

    mount();

    const panel = await screen.findByRole("region", { name: "Backlinks" });
    await userEvent.click(
      await within(panel).findByRole("button", { name: /relates_to/ }),
    );
    await screen.findByRole("link", { name: /Beta/ });

    await userEvent.type(screen.getByRole("searchbox"), "gam");

    await waitFor(() => {
      expect(screen.queryByRole("link", { name: /Beta/ })).toBeNull();
    });
    expect(screen.getByRole("link", { name: /Gamma/ })).toBeInTheDocument();
    expect(requested()).toContain(
      "/domains/eng/inbound/alpha?page=1&limit=20&rel=relates_to&q=gam",
    );
  });

  it("says what the server said when the summary could not be read", async () => {
    apiMock.mockRejectedValue(
      new ApiProblem(403, "forbidden", "this account may not read eng"),
    );

    mount();

    const panel = await screen.findByRole("region", { name: "Backlinks" });
    const alert = await within(panel).findByRole("alert");
    // The server's own words, the way every other error surface in this app
    // shows them.
    expect(alert).toHaveTextContent("this account may not read eng");
  });

  it("says plainly when the engram was retired out from under the panel", async () => {
    apiMock.mockRejectedValue(
      new ApiProblem(404, "not found", "no engram 'alpha' in domain 'eng'"),
    );

    mount();

    const panel = await screen.findByRole("region", { name: "Backlinks" });
    expect(await within(panel).findByRole("alert")).toHaveTextContent(
      "no engram 'alpha' in domain 'eng'",
    );
  });
});
