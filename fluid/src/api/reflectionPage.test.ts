import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "./client";
import { readDoc, readSearch, readTree } from "./reflection";
import {
  fetchReflectionClock,
  readClockAnswer,
  readPageView,
  reflectionClockKey,
  restoreReflection,
  retireReflection,
  setReflectionSettings,
  settingsChanges,
} from "./reflectionPage";
import { fetchMissedPages, readMissedPages } from "./reflectionMissed";
import { reflectionKey } from "./reflectionDomains";

vi.mock("./client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./client")>();
  return { ...actual, api: vi.fn() };
});
const apiMock = vi.mocked(api);

beforeEach(() => {
  apiMock.mockReset();
});

const PAGE_VIEW = {
  freshness: "1d",
  follows: null,
  status: null,
  owner: {
    path: "arch/overview.md",
    title: "Architecture overview",
    exists: true,
  },
  retired: null,
  chain: [{ path: "arch/overview.md", title: "Architecture overview" }],
  pacts: ["PACT.md", "arch/overview.pact.md"],
  pact: { path: "arch/overview/db.pact.md", exists: false },
  findings: [
    {
      page: "arch/overview/db.md",
      key: "freshness",
      value: "5m",
      reason: "shorter than 1h",
    },
  ],
};

describe("the page view", () => {
  it("reads settings, owner, chain, pacts and findings", () => {
    expect(readPageView(PAGE_VIEW)).toEqual({
      freshness: "1d",
      follows: null,
      status: null,
      owner: {
        path: "arch/overview.md",
        title: "Architecture overview",
        exists: true,
      },
      retired: null,
      chain: [{ path: "arch/overview.md", title: "Architecture overview" }],
      pacts: ["PACT.md", "arch/overview.pact.md"],
      pact: { path: "arch/overview/db.pact.md", exists: false },
      findings: [
        {
          page: "arch/overview/db.md",
          key: "freshness",
          value: "5m",
          reason: "shorter than 1h",
        },
      ],
    });
  });

  it("reads a retirement whose time and actor are unknown", () => {
    const view = readPageView({
      ...PAGE_VIEW,
      owner: null,
      retired: { at: null, by: null, was_owned_by: null },
    });
    expect(view?.retired).toEqual({ at: null, by: null, wasOwnedBy: null });
    const owned = readPageView({
      ...PAGE_VIEW,
      retired: {
        at: "2026-11-20T10:00:00Z",
        by: "ed",
        was_owned_by: {
          path: "arch/overview.md",
          title: "Overview",
          exists: false,
        },
      },
    });
    expect(owned?.retired).toEqual({
      at: "2026-11-20T10:00:00Z",
      by: "ed",
      wasOwnedBy: {
        path: "arch/overview.md",
        title: "Overview",
        exists: false,
      },
    });
  });

  it("is null for a pact file, which has no page view", () => {
    expect(readPageView(undefined)).toBeNull();
    expect(readPageView(null)).toBeNull();
    expect(readDoc({ path: "PACT.md", markdown: "" }).page).toBeNull();
  });

  it("rides the page read", () => {
    const doc = readDoc({ path: "a.md", markdown: "", page: PAGE_VIEW });
    expect(doc.page?.freshness).toBe("1d");
    expect(doc.page?.chain).toHaveLength(1);
  });
});

describe("retired pages in the tree and in search", () => {
  it("marks a retired page and its owner in the tree", () => {
    const tree = readTree({
      domain: "eng",
      pact: { path: "PACT.md", title: "PACT.md", exists: true },
      nodes: [
        {
          kind: "reflection",
          name: "old.md",
          path: "old.md",
          title: "Old",
          retired: true,
          owner: "arch/overview.md",
        },
        { kind: "reflection", name: "new.md", path: "new.md", title: "New" },
      ],
    });
    expect(tree.nodes[0]).toMatchObject({
      retired: true,
      owner: "arch/overview.md",
    });
    expect(tree.nodes[1]).toMatchObject({ retired: false, owner: null });
  });

  it("marks a retired search hit", () => {
    const search = readSearch({
      hits: [
        { path: "old.md", title: "Old", retired: true },
        { path: "new.md", title: "New" },
      ],
    });
    expect(search.hits.map((h) => h.retired)).toEqual([true, false]);
  });
});

describe("the page clock", () => {
  it("reads a clock and the checksum beside it", () => {
    expect(
      readClockAnswer({
        clock: {
          path: "a.md",
          freshness: "1h",
          last_reconciled: "2026-11-20T10:00:00Z",
          last_checked_secs: 7200,
          due_in_secs: null,
          stale: true,
          overdue_secs: 10800,
          missed_secs: 600,
          loop_slower_than_freshness: true,
          waiting: false,
          part_of: null,
          clock_of: "a.md",
        },
        checksum: "abc",
      }),
    ).toEqual({
      checksum: "abc",
      clock: {
        path: "a.md",
        freshness: "1h",
        lastReconciled: "2026-11-20T10:00:00Z",
        lastCheckedSecs: 7200,
        dueInSecs: null,
        stale: true,
        overdueSecs: 10800,
        missedSecs: 600,
        loopSlowerThanFreshness: true,
        waiting: false,
        partOf: null,
        clockOf: "a.md",
      },
    });
  });

  it("reads no clock and no checksum as nulls", () => {
    expect(readClockAnswer({ clock: null })).toEqual({
      clock: null,
      checksum: null,
    });
  });

  it("asks the clock route with the path as a query", async () => {
    apiMock.mockResolvedValueOnce({ clock: null, checksum: "x" });
    await fetchReflectionClock("my eng", "a b/c.md");
    expect(apiMock).toHaveBeenCalledWith(
      "/domains/my%20eng/reflection/clock?path=a+b%2Fc.md",
    );
  });

  it("keys the clock under the domain's Reflection", () => {
    expect(reflectionClockKey("eng", "a.md").slice(0, 2)).toEqual(
      reflectionKey("eng"),
    );
  });
});

describe("page settings", () => {
  it("sends only what changed, and an empty value to clear", () => {
    expect(
      settingsChanges(
        { freshness: "1d", follows: "a.md" },
        { freshness: "1d", follows: "" },
      ),
    ).toEqual({ follows: "" });
    expect(
      settingsChanges({ freshness: null, follows: null }, { freshness: "6h" }),
    ).toEqual({ freshness: "6h" });
    expect(settingsChanges({ freshness: "6h" }, { freshness: "6h" })).toEqual(
      {},
    );
    // Nothing set and nothing asked: no key is sent, never a clear.
    expect(settingsChanges({ freshness: null }, { freshness: "" })).toEqual({});
  });

  it("patches the settings route with the path in the body", async () => {
    apiMock.mockResolvedValueOnce({
      domain: "eng",
      path: "a.md",
      checksum: "c",
      page: PAGE_VIEW,
    });
    const receipt = await setReflectionSettings("eng", "a.md", {
      freshness: "6h",
    });
    expect(apiMock).toHaveBeenCalledWith("/domains/eng/reflection/settings", {
      method: "PATCH",
      body: JSON.stringify({ path: "a.md", freshness: "6h" }),
    });
    expect(receipt.page?.freshness).toBe("1d");
  });

  it("retires and restores, under the owner when asked", async () => {
    apiMock.mockResolvedValue({});
    await retireReflection("eng", "a.md");
    expect(apiMock).toHaveBeenLastCalledWith("/domains/eng/reflection/retire", {
      method: "POST",
      body: JSON.stringify({ path: "a.md" }),
    });
    await restoreReflection("eng", "a.md");
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/reflection/restore",
      { method: "POST", body: JSON.stringify({ path: "a.md" }) },
    );
    await restoreReflection("eng", "a.md", "arch/overview.md");
    expect(apiMock).toHaveBeenLastCalledWith(
      "/domains/eng/reflection/restore",
      {
        method: "POST",
        body: JSON.stringify({ path: "a.md", owner: "arch/overview.md" }),
      },
    );
  });
});

describe("the missed pages", () => {
  it("reads the pages a reflect run missed", async () => {
    const answer = {
      pages: [
        {
          domain: "eng",
          path: "status/week.md",
          freshness: "1h",
          last_checked_secs: 21900,
          missed_secs: 10800,
          waiting: false,
        },
        { domain: "eng" },
      ],
    };
    expect(readMissedPages(answer)).toEqual([
      {
        domain: "eng",
        path: "status/week.md",
        freshness: "1h",
        lastCheckedSecs: 21900,
        missedSecs: 10800,
        waiting: false,
      },
    ]);
    apiMock.mockResolvedValueOnce(answer);
    expect(await fetchMissedPages()).toHaveLength(1);
    expect(apiMock).toHaveBeenCalledWith("/reflection/missed");
  });
});
