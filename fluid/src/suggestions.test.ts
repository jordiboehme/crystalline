/**
 * The guidance is only worth showing while it stays complete and stays one
 * list: a word offered with nothing said about it teaches nobody anything, and
 * two lists would let the filter bar and the editor recommend different
 * vocabularies for the same field.
 */

import { describe, expect, it } from "vitest";

import { SUGGESTED_STATUSES, SUGGESTED_TYPES } from "./filters";
import {
  STATUS_SUGGESTIONS,
  TYPE_SUGGESTIONS,
  withHouseCounts,
} from "./suggestions";

describe("the recommended vocabulary", () => {
  it("offers exactly what the filters offer, in the same order", () => {
    expect(TYPE_SUGGESTIONS.map((each) => each.name)).toEqual(SUGGESTED_TYPES);
    expect(STATUS_SUGGESTIONS.map((each) => each.name)).toEqual(
      SUGGESTED_STATUSES,
    );
  });

  it("says what every recommended word is for", () => {
    for (const suggestion of [...TYPE_SUGGESTIONS, ...STATUS_SUGGESTIONS]) {
      expect(suggestion.gloss, `no gloss for ${suggestion.name}`).toBeTruthy();
    }
  });

  it("keeps the alias, and says it is one", () => {
    // `current` is what domains written before the rename carry, so it stays
    // offered; the line beside it is what keeps it from reading as a second
    // canonical word.
    const alias = STATUS_SUGGESTIONS.find((each) => each.name === "current");
    expect(alias?.gloss).toContain("stable");
  });
});

describe("the words a domain already uses", () => {
  it("gives a recommended word the count the domain has for it", () => {
    const merged = withHouseCounts(
      [{ name: "guide", gloss: "how to do something" }, { name: "decision" }],
      [{ name: "guide", count: 4 }],
    );

    expect(merged).toStrictEqual([
      { name: "guide", gloss: "how to do something", count: 4 },
      { name: "decision" },
    ]);
  });

  it("appends the domain's own words, commonest first, with no gloss", () => {
    const merged = withHouseCounts(
      [{ name: "engram", gloss: "a unit of knowledge" }],
      [
        { name: "sketch", count: 1 },
        { name: "playbook", count: 9 },
        { name: "essay", count: 1 },
      ],
    );

    expect(merged).toStrictEqual([
      { name: "engram", gloss: "a unit of knowledge" },
      { name: "playbook", count: 9 },
      { name: "essay", count: 1 },
      { name: "sketch", count: 1 },
    ]);
  });

  it("is the recommendations themselves when the domain adds nothing", () => {
    expect(withHouseCounts(TYPE_SUGGESTIONS, [])).toStrictEqual([
      ...TYPE_SUGGESTIONS,
    ]);
    expect(withHouseCounts(STATUS_SUGGESTIONS, [])).toStrictEqual([
      ...STATUS_SUGGESTIONS,
    ]);
  });
});
