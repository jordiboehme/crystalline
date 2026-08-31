/**
 * The three rules the share dialog's checkboxes run on, tested where they
 * live rather than through the dialog: which paths are generated listings,
 * which listings a selection drags along, and which boxes open ticked.
 *
 * The dialog's own tests pin the wiring - that a tick reaches the request and
 * a hint reaches the screen. These pin the arithmetic behind them, which is
 * where the edge cases are: a deletion nobody is attributed for, a delta with
 * no authors in it at all, a folder whose last file was just unticked.
 */

import { describe, expect, it } from "vitest";

import type { ShareChange } from "../api/admin";
import {
  isFolderIndex,
  ownedPhrase,
  preselect,
  ridingIndexes,
  shareBadgeCount,
} from "./changes";

/** One change, with the author the plan named for it. */
function change(
  path: string,
  lastAuthor: string | null = null,
  kind = "modified",
): ShareChange {
  return { path, kind, lastAuthor };
}

describe("folder listings", () => {
  it("reads a listing off its filename at any depth", () => {
    expect(isFolderIndex("index.md")).toBe(true);
    expect(isFolderIndex("notes/index.md")).toBe(true);
    expect(isFolderIndex("notes/deep/index.md")).toBe(true);
    expect(isFolderIndex("notes/indexes.md")).toBe(false);
    expect(isFolderIndex("index.md.bak")).toBe(false);
  });

  it("carries every listing while everything is ticked", () => {
    const changes = [
      change("notes/a.md"),
      change("guides/g.md"),
      change("index.md"),
      change("notes/index.md"),
      change("guides/index.md"),
    ];
    const all = new Set(["notes/a.md", "guides/g.md"]);
    // No file list goes over the wire at all in this shape, so the share is
    // the whole delta and the count is the delta's own.
    expect(ridingIndexes(changes, all)).toBe(3);
  });

  it("carries only the chosen files' own folders once something is unticked", () => {
    const changes = [
      change("notes/a.md"),
      change("guides/g.md"),
      change("index.md"),
      change("notes/index.md"),
      change("guides/index.md"),
    ];
    expect(ridingIndexes(changes, new Set(["notes/a.md"]))).toBe(1);
    // And nothing at all when the selection is empty: there is no folder for
    // a listing to belong to.
    expect(ridingIndexes(changes, new Set())).toBe(0);
  });
});

describe("preselection", () => {
  it("ticks this account's own work and says what it left", () => {
    const changes = [
      change("notes/mine.md", "human:ada"),
      change("notes/theirs.md", "human:bob"),
      change("notes/nobodys.md", null),
      change("notes/gone.md", null, "deleted"),
      change("notes/index.md", "human:ada"),
    ];
    const preset = preselect(changes, "ada");
    // The listing is never a box, so it is never preselected either.
    expect(preset.paths).toEqual(["notes/mine.md"]);
    expect(preset.hint).toBe(
      "Preselected your 1 change - 3 more from others left unticked.",
    );
  });

  it("ticks everything when nothing here is this account's", () => {
    const changes = [
      change("notes/a.md", "human:bob"),
      change("notes/b.md", null),
    ];
    const preset = preselect(changes, "ada");
    expect(preset.paths).toEqual(["notes/a.md", "notes/b.md"]);
    // Nothing was left out, so there is nothing to explain.
    expect(preset.hint).toBeNull();
  });

  it("ticks everything for a session with no account and a plan with no authors", () => {
    const changes = [change("notes/a.md"), change("notes/b.md")];
    expect(preselect(changes, null).paths).toEqual([
      "notes/a.md",
      "notes/b.md",
    ]);
    expect(preselect(changes, "ada").hint).toBeNull();
  });

  it("says nothing when everything already belongs to this account", () => {
    const changes = [
      change("notes/a.md", "human:ada"),
      change("notes/b.md", "human:ada"),
    ];
    const preset = preselect(changes, "ada");
    expect(preset.paths).toEqual(["notes/a.md", "notes/b.md"]);
    expect(preset.hint).toBeNull();
  });
});

describe("the owned share of what is waiting", () => {
  it("draws this account's own count where there is one", () => {
    expect(shareBadgeCount(2, 5)).toBe(2);
    // None of it is this account's, so the badge is back to how much is
    // waiting: a zero badge would read as "nothing to share" on a button
    // that is about to open a dialog full of changes.
    expect(shareBadgeCount(0, 5)).toBe(5);
    // The server did not say, which is the shape every older report has.
    expect(shareBadgeCount(null, 5)).toBe(5);
  });

  it("says how much of the waiting work is yours, or nothing at all", () => {
    expect(ownedPhrase(2, 5, "unshared changes")).toBe(
      "2 of 5 unshared changes are yours",
    );
    expect(ownedPhrase(0, 3, "pending changes")).toBe(
      "0 of 3 pending changes are yours",
    );
    // Null is "this report does not say", which is not a sentence anybody
    // can be shown: the surfaces fall back to what they always said.
    expect(ownedPhrase(null, 5, "unshared changes")).toBeNull();
  });
});
