import { act, render, screen } from "@testing-library/react";
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
