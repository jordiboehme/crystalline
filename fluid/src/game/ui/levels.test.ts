/** The level select's pure parts: order, filter, window, selection. */
import { describe, expect, it } from "vitest";

import {
  LEVEL_ROWS,
  filterLevels,
  levelWindow,
  sortLevels,
  stepSelection,
} from "./levels";

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
