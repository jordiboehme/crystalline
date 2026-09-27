/**
 * The level select's pure parts: the rows built from the listing, order,
 * filter, the current row, window, selection.
 */
import { describe, expect, it } from "vitest";

import {
  LEVEL_ROWS,
  type Level,
  filterLevels,
  hereKey,
  levelWindow,
  levelsOf,
  sortLevels,
  stepSelection,
} from "./levels";

/** One listing row: a local name, and optionally what else it answers to. */
function domain(
  name: string,
  canonicalName: string | null = null,
  aliases: string[] = [],
  shadowed = false,
) {
  return { name, canonicalName, aliases, shadowed };
}

/** The labels of these rows, in order. */
const labels = (levels: readonly Level[]) => levels.map((l) => l.label);

describe("sortLevels (C12)", () => {
  it("sorts by name ignoring case, then by code point", () => {
    expect(sortLevels(["eng", "beta", "alpha", "Beta"])).toEqual([
      "alpha",
      "Beta",
      "beta",
      "eng",
    ]);
  });
});

describe("filterLevels (C12)", () => {
  const sorted = ["alpha", "Beta", "platform-eng"];
  it("matches a trimmed, case-insensitive substring", () => {
    expect(filterLevels(sorted, " BE ")).toEqual(["Beta"]);
    expect(filterLevels(sorted, "eng")).toEqual(["platform-eng"]);
    expect(filterLevels(sorted, "")).toEqual(sorted);
    expect(filterLevels(sorted, "zz")).toEqual([]);
  });
});

describe("levelsOf", () => {
  it("labels a row by its local name, or its canonical name beside it when they differ", () => {
    expect(
      labels(
        levelsOf([
          domain("eng"),
          domain("ops", "ops"),
          domain("infra", "platform"),
        ]),
      ),
    ).toEqual(["eng", "ops", "platform (infra)"]);
  });

  it("keys a row by the local name, so the jump always goes by it", () => {
    const [level] = levelsOf([domain("infra", "platform", ["old-infra"])]);
    expect(level?.key).toBe("infra");
  });

  it("finds a row by its local name, its canonical name or any alias", () => {
    const levels = sortLevels(
      levelsOf([
        domain("eng"),
        domain("infra", "platform", ["tooling", "old-infra"]),
      ]),
      (l) => l.label,
    );
    const find = (query: string) =>
      filterLevels(levels, query, (l) => l.terms).map((l) => l.key);
    expect(find("infra")).toEqual(["infra"]);
    expect(find("PLATFORM")).toEqual(["infra"]);
    expect(find(" tool ")).toEqual(["infra"]);
    expect(find("en")).toEqual(["eng"]);
  });

  it("orders the rows by the label they show", () => {
    const levels = sortLevels(
      levelsOf([domain("b-local", "zeta"), domain("c"), domain("a")]),
      (l) => l.label,
    );
    expect(labels(levels)).toEqual(["a", "c", "zeta (b-local)"]);
  });
});

describe("hereKey", () => {
  it("resolves the current domain by its local name, canonical name or alias", () => {
    const levels = levelsOf([
      domain("eng"),
      domain("infra", "platform", ["old-infra"]),
    ]);
    expect(hereKey(levels, "eng")).toBe("eng");
    expect(hereKey(levels, "infra")).toBe("infra");
    expect(hereKey(levels, "platform")).toBe("infra");
    expect(hereKey(levels, "old-infra")).toBe("infra");
    expect(hereKey(levels, "gone")).toBeNull();
  });

  // One domain's local name is another's canonical name: filtering shows
  // both, but the current one is the domain that holds it locally. Matching
  // every term in one pass would pick the first row that holds it, here the
  // shadowed one listed first.
  it("marks one row only when a local name is another domain's canonical name", () => {
    const levels = levelsOf([domain("sat", "moon", [], true), domain("moon")]);
    expect(
      filterLevels(levels, "moon", (l) => l.terms).map((l) => l.key),
    ).toEqual(["sat", "moon"]);
    expect(hereKey(levels, "moon")).toBe("moon");
  });

  // A shadowed canonical name is never taken as the current row, even when
  // no local name matches; ignoring the shadowed flag would mark the
  // shadowed row here instead of the alias holder.
  it("skips a shadowed canonical name", () => {
    const levels = levelsOf([
      domain("sat", "moon", [], true),
      domain("lune", null, ["moon"]),
    ]);
    expect(hereKey(levels, "moon")).toBe("lune");
  });
});

describe("levelWindow (C15)", () => {
  it("shows everything that fits, else LEVEL_ROWS around the selection", () => {
    expect(levelWindow(0, 0)).toEqual({ start: 0, end: 0 });
    expect(levelWindow(3, 2)).toEqual({ start: 0, end: 3 });
    expect(levelWindow(200, 0)).toEqual({ start: 0, end: LEVEL_ROWS });
    expect(levelWindow(200, 150)).toEqual({ start: 145, end: 155 });
    expect(levelWindow(200, 199)).toEqual({ start: 190, end: 200 });
  });
});

describe("stepSelection (C13)", () => {
  it("clamps at both ends and never wraps", () => {
    expect(stepSelection(0, 5, 1)).toBe(0);
    expect(stepSelection(3, 0, -1)).toBe(0);
    expect(stepSelection(3, 2, 1)).toBe(2);
    expect(stepSelection(3, 7, 0)).toBe(2);
    expect(stepSelection(3, 0, 1)).toBe(1);
  });
});
