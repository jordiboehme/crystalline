import { describe, expect, it } from "vitest";

import type { PageClock } from "./api/reflectionPage";
import { freshnessLine, noFreshnessNote, skippedNote } from "./reflectionClock";

const HOUR = 3600;

function clock(over: Partial<PageClock> = {}): PageClock {
  return {
    path: "a.md",
    freshness: "3h",
    lastReconciled: "2026-11-20T10:00:00Z",
    lastCheckedSecs: 2 * HOUR,
    dueInSecs: HOUR,
    stale: false,
    overdueSecs: null,
    missedSecs: null,
    loopSlowerThanFreshness: false,
    waiting: false,
    partOf: null,
    clockOf: "a.md",
    ...over,
  };
}

describe("the freshness line", () => {
  it("says when the page was checked and when it is due", () => {
    expect(freshnessLine(clock(), 0)).toEqual({
      kind: "plain",
      text: "Last checked 2 hours ago, due again in 1 hour",
    });
  });

  it("ages with the time since the clock was read", () => {
    expect(freshnessLine(clock(), 30 * 60)?.text).toBe(
      "Last checked 2 hours ago, due again in 30 minutes",
    );
    expect(freshnessLine(clock(), HOUR - 10)?.text).toBe(
      "Last checked 2 hours ago, due again in less than a minute",
    );
    expect(freshnessLine(clock(), HOUR + 5)?.text).toBe(
      "Last checked 3 hours ago, due now",
    );
  });

  it("says just now for a check under a minute ago", () => {
    expect(freshnessLine(clock({ lastCheckedSecs: 20 }), 0)?.text).toBe(
      "Last checked just now, due again in 1 hour",
    );
  });

  it("says due now for a page that is due but not yet overdue", () => {
    expect(
      freshnessLine(clock({ stale: true, dueInSecs: null }), 0)?.text,
    ).toBe("Last checked 2 hours ago, due now");
  });

  it("says since when a page is overdue, in the warning colour", () => {
    expect(
      freshnessLine(
        clock({
          stale: true,
          dueInSecs: null,
          lastCheckedSecs: 5 * HOUR,
          overdueSecs: 3 * HOUR,
        }),
        0,
      ),
    ).toEqual({
      kind: "overdue",
      text: "Overdue since 3 hours, last checked 5 hours ago",
      hint: null,
    });
  });

  it("adds the hint when the loop runs less often than the freshness", () => {
    expect(
      freshnessLine(
        clock({
          stale: true,
          dueInSecs: null,
          overdueSecs: HOUR,
          loopSlowerThanFreshness: true,
        }),
        0,
      ),
    ).toMatchObject({
      kind: "overdue",
      hint: "The reflect loop runs less often than this page's freshness.",
    });
  });

  it("says waiting instead while the page is open in the editor", () => {
    expect(
      freshnessLine(
        clock({
          stale: true,
          dueInSecs: null,
          overdueSecs: HOUR,
          waiting: true,
        }),
        0,
      ),
    ).toEqual({ kind: "waiting", text: "Waiting: open in the editor" });
  });

  it("gives an owned page its owner's last check and no due time", () => {
    expect(
      freshnessLine(
        clock({
          path: "a/b.md",
          clockOf: "a.md",
          partOf: { path: "a.md", title: "A" },
          dueInSecs: null,
        }),
        0,
      ),
    ).toEqual({ kind: "plain", text: "Last checked 2 hours ago" });
  });

  it("has nothing for an owned page whose owner has no freshness", () => {
    const owned = clock({
      freshness: null,
      lastReconciled: null,
      lastCheckedSecs: null,
      dueInSecs: null,
      clockOf: null,
      partOf: { path: "a.md", title: "Architecture overview" },
    });
    expect(freshnessLine(owned, 0)).toBeNull();
    expect(noFreshnessNote(owned)).toBe(
      "Architecture overview has no freshness, so this page is not checked on a schedule",
    );
    expect(noFreshnessNote(clock())).toBeNull();
    expect(freshnessLine(null, 0)).toBeNull();
  });
});

describe("the skipped note", () => {
  it("says a reflect run skipped the page", () => {
    expect(skippedNote(clock({ missedSecs: 600 }))).toBe(
      "A reflect run skipped this page.",
    );
    expect(skippedNote(clock())).toBeNull();
    expect(skippedNote(clock({ missedSecs: 600, waiting: true }))).toBeNull();
  });
});
