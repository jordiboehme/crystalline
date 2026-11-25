import { act, cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { PageClock, PageView } from "../api/reflectionPage";
import { ReflectionPageStatus } from "./ReflectionPageStatus";

const PAGE: PageView = {
  freshness: "3h",
  follows: null,
  status: null,
  owner: null,
  retired: null,
  refreshRequested: null,
  chain: [],
  pacts: ["PACT.md"],
  pact: { path: "a.pact.md", exists: false },
  findings: [],
};

const CLOCK: PageClock = {
  path: "a.md",
  freshness: "3h",
  lastReconciled: "2026-11-20T10:00:00Z",
  lastCheckedSecs: 7200,
  dueInSecs: 3600,
  stale: false,
  overdueSecs: null,
  missedSecs: null,
  loopSlowerThanFreshness: false,
  waiting: false,
  partOf: null,
  clockOf: "a.md",
  refreshRequested: null,
};

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("the status lines", () => {
  it("age on their own minute tick, from when the clock was read", () => {
    const readAt = Date.now();
    render(
      <MemoryRouter>
        <ReflectionPageStatus
          domain="eng"
          page={PAGE}
          clock={CLOCK}
          readAt={readAt}
          curator={false}
        />
      </MemoryRouter>,
    );
    expect(
      screen.getByText("Last checked 2 hours ago, due again in 1 hour"),
    ).toBeVisible();
    act(() => {
      vi.advanceTimersByTime(30 * 60_000);
    });
    expect(
      screen.getByText("Last checked 2 hours ago, due again in 30 minutes"),
    ).toBeVisible();
  });
});

describe("the pacts line", () => {
  function pactsLine(page: PageView): HTMLElement {
    render(
      <MemoryRouter>
        <ReflectionPageStatus
          domain="eng"
          page={page}
          clock={null}
          readAt={0}
          curator={false}
        />
      </MemoryRouter>,
    );
    return screen.getByText(/^Pacts:/);
  }

  it("names each pact file by its file name and links to it, the page's own pact last", () => {
    const line = pactsLine({
      ...PAGE,
      chain: [{ path: "arch/overview.md", title: "Architecture overview" }],
      pacts: ["PACT.md", "arch/overview.pact.md", "arch/overview/db.pact.md"],
      pact: { path: "arch/overview/db.pact.md", exists: true },
    });
    expect(line).toHaveTextContent(
      "Pacts: PACT.md > overview.pact.md > db.pact.md",
    );
    expect(
      screen.getByRole("link", { name: "overview.pact.md" }),
    ).toHaveAttribute("href", "/d/eng/reflection/arch/overview.pact.md");
    expect(screen.getByRole("link", { name: "db.pact.md" })).toHaveAttribute(
      "href",
      "/d/eng/reflection/arch/overview/db.pact.md",
    );
  });

  it("leaves out the page's own pact while it does not exist", () => {
    const line = pactsLine({
      ...PAGE,
      chain: [{ path: "arch/overview.md", title: "Architecture overview" }],
      pacts: ["PACT.md", "arch/overview.pact.md"],
      pact: { path: "arch/overview/db.pact.md", exists: false },
    });
    expect(line).toHaveTextContent(/^Pacts: PACT\.md > overview\.pact\.md$/);
    expect(screen.queryByRole("link", { name: "db.pact.md" })).toBeNull();
  });

  it("is left out when no pact file exists", () => {
    render(
      <MemoryRouter>
        <ReflectionPageStatus
          domain="eng"
          page={{ ...PAGE, pacts: [] }}
          clock={null}
          readAt={0}
          curator={false}
        />
      </MemoryRouter>,
    );
    expect(screen.queryByText(/^Pacts:/)).toBeNull();
  });
});

describe("the not fresh line", () => {
  function status(page: PageView, clock: PageClock | null): void {
    render(
      <MemoryRouter>
        <ReflectionPageStatus
          domain="eng"
          page={page}
          clock={clock}
          readAt={0}
          curator={false}
        />
      </MemoryRouter>,
    );
  }

  it("says who marked the page, by display name, from the clock", () => {
    status(PAGE, {
      ...CLOCK,
      refreshRequested: {
        at: "2026-11-20T10:00:00Z",
        by: "ann",
        byDisplay: "Ann Lee",
      },
    });
    expect(screen.getByText(/^Not fresh: marked by Ann Lee at /)).toBeVisible();
  });

  it("falls back to the page view while the clock loads, and to the name", () => {
    status(
      {
        ...PAGE,
        refreshRequested: {
          at: "2026-11-20T10:00:00Z",
          by: "ann",
          byDisplay: null,
        },
      },
      null,
    );
    expect(screen.getByText(/^Not fresh: marked by ann at /)).toBeVisible();
  });

  it("shows a mark whose record names nobody, without a name", () => {
    status(PAGE, {
      ...CLOCK,
      refreshRequested: {
        at: "2026-11-20T10:00:00Z",
        by: null,
        byDisplay: null,
      },
    });
    expect(screen.getByText(/^Not fresh: marked at /)).toBeVisible();
    expect(screen.queryByText(/marked by/)).toBeNull();
  });

  it("is gone once the clock shows no mark, and on a retired page", () => {
    const mark = {
      at: "2026-11-20T10:00:00Z",
      by: "ann",
      byDisplay: null,
    };
    status({ ...PAGE, refreshRequested: mark }, CLOCK);
    expect(screen.queryByText(/Not fresh/)).toBeNull();
    cleanup();
    status(
      {
        ...PAGE,
        refreshRequested: mark,
        retired: { at: null, by: null, wasOwnedBy: null },
      },
      { ...CLOCK, refreshRequested: mark },
    );
    expect(screen.queryByText(/Not fresh/)).toBeNull();
  });
});
