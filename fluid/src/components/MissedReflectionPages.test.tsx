import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { MemoryRouter } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import {
  MissedPagesBanner,
  MissedPagesList,
  missedLine,
} from "./MissedReflectionPages";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn() };
});
const apiMock = vi.mocked(api);

const HOUR = 3600;

function page(n: number, overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    path: `status/p${String(n)}.md`,
    freshness: "1h",
    last_checked_secs: 6 * HOUR,
    missed_secs: 3 * HOUR,
    waiting: false,
    ...overrides,
  };
}

function wrap(children: ReactNode) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>{children}</MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the pages a reflect run missed", () => {
  it("says since when, and the last check, or that the page waits", () => {
    expect(
      missedLine({
        domain: "eng",
        path: "a.md",
        freshness: "1h",
        lastCheckedSecs: 6 * HOUR,
        missedSecs: 3 * HOUR,
        waiting: false,
      }),
    ).toBe("missed since 3 hours, last checked 6 hours ago");
    expect(
      missedLine({
        domain: "eng",
        path: "a.md",
        freshness: "1h",
        lastCheckedSecs: 6 * HOUR,
        missedSecs: 3 * HOUR,
        waiting: true,
      }),
    ).toBe("missed since 3 hours. Waiting: open in the editor");
  });

  it("the banner names each page with its domain, and links it", async () => {
    apiMock.mockResolvedValue({ pages: [page(1), page(2, { domain: "ops" })] });
    wrap(<MissedPagesBanner />);
    const banner = await screen.findByRole("status", {
      name: "Reflection pages",
    });
    expect(banner).toHaveTextContent(
      "A reflect run missed 2 Reflection pages.",
    );
    expect(
      within(banner).getByRole("link", { name: "eng: status/p1.md" }),
    ).toHaveAttribute("href", "/d/eng/reflection/status/p1.md");
    expect(
      within(banner).getByRole("link", { name: "ops: status/p2.md" }),
    ).toBeVisible();
    expect(banner).toHaveTextContent(
      "missed since 3 hours, last checked 6 hours ago",
    );
    expect(apiMock).toHaveBeenCalledWith("/reflection/missed");
  });

  it("the banner shows five and sends the rest to Maintenance", async () => {
    apiMock.mockResolvedValue({
      pages: [1, 2, 3, 4, 5, 6, 7].map((n) => page(n)),
    });
    wrap(<MissedPagesBanner />);
    const banner = await screen.findByRole("status", {
      name: "Reflection pages",
    });
    expect(within(banner).getAllByRole("link")).toHaveLength(6);
    expect(
      within(banner).getByRole("link", { name: "2 more in Maintenance" }),
    ).toHaveAttribute("href", "/maintenance");
  });

  it("the banner draws nothing when no page was missed or the read fails", async () => {
    apiMock.mockResolvedValueOnce({ pages: [] });
    const first = wrap(<MissedPagesBanner />);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(first.container).toBeEmptyDOMElement();
    first.unmount();
    apiMock.mockRejectedValueOnce(new ApiProblem(500, "boom", ""));
    const second = wrap(<MissedPagesBanner />);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(second.container).toBeEmptyDOMElement();
  });

  it("the maintenance list names every page, with its freshness", async () => {
    apiMock.mockResolvedValue({
      pages: [1, 2, 3, 4, 5, 6, 7].map((n) => page(n)),
    });
    wrap(<MissedPagesList />);
    const list = await screen.findByRole("region", {
      name: "Reflection pages a reflect run missed",
    });
    expect(within(list).getAllByRole("listitem")).toHaveLength(7);
    expect(within(list).getAllByText(/Freshness 1h/)).toHaveLength(7);
  });
});
