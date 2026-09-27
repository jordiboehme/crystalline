import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { EngramChange } from "../api/events";
import { RECENT_TTL_MS, RecentChanges } from "./recent";

function change(overrides: Partial<EngramChange> = {}): EngramChange {
  return {
    domain: "eng",
    permalink: "alpha",
    path: "alpha.md",
    kind: "modified",
    from: null,
    checksum: "9f",
    actor: "ada",
    draftOf: null,
    ...overrides,
  };
}

describe("the recent changes store", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-28T10:00:00Z"));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("an add or a modify is an update, with the actor and the time it arrived", () => {
    const recent = new RecentChanges();
    recent.note(change());
    expect(recent.get("eng", "alpha")).toEqual({
      kind: "updated",
      actor: "ada",
      at: Date.now(),
      to: null,
    });
    recent.note(change({ permalink: "beta", kind: "added", actor: null }));
    expect(recent.get("eng", "beta")).toMatchObject({
      kind: "updated",
      actor: null,
    });
    expect(recent.get("other", "alpha"), "keyed by domain too").toBeNull();
  });

  it("a move is moved_here at the new address and moved_away, with where to, at the old one", () => {
    const recent = new RecentChanges();
    recent.note(
      change({
        kind: "moved",
        permalink: "topics/alpha",
        from: { path: "alpha.md", permalink: "alpha" },
      }),
    );
    expect(recent.get("eng", "topics/alpha")).toMatchObject({
      kind: "moved_here",
      actor: "ada",
      to: null,
    });
    expect(recent.get("eng", "alpha")).toMatchObject({
      kind: "moved_away",
      actor: "ada",
      to: "topics/alpha",
    });
  });

  it("a delete removes the entry, so the page shows its not-found face and no line", () => {
    const recent = new RecentChanges();
    recent.note(change());
    recent.note(change({ kind: "deleted", checksum: null }));
    expect(recent.get("eng", "alpha")).toBeNull();
  });

  it("tells its subscribers on every note, and stops once they leave", () => {
    const recent = new RecentChanges();
    const listener = vi.fn();
    const leave = recent.subscribe(listener);
    recent.note(change());
    expect(listener).toHaveBeenCalledTimes(1);
    leave();
    recent.note(change());
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("drops what is older than the status line's minute on the next note, so the store cannot grow without bound", () => {
    // Catches entries that are never pruned: a tab open for days on a busy
    // instance would keep one per engram ever changed.
    const recent = new RecentChanges();
    for (let i = 0; i < 50; i++) {
      recent.note(change({ permalink: `old-${String(i)}` }));
    }
    vi.advanceTimersByTime(RECENT_TTL_MS - 1);
    recent.note(change({ permalink: "fresh" }));
    expect(recent.size, "nothing is a minute old yet").toBe(51);
    vi.advanceTimersByTime(1);
    recent.note(change({ permalink: "newest" }));
    expect(recent.get("eng", "old-0")).toBeNull();
    expect(recent.get("eng", "fresh")).not.toBeNull();
    expect(recent.size).toBe(2);
  });
});
