/**
 * The guidance is only worth showing while it stays complete and stays one
 * list: a word offered with nothing said about it teaches nobody anything, and
 * two lists would let the filter bar and the editor recommend different
 * vocabularies for the same field.
 */

import { describe, expect, it } from "vitest";

import { SUGGESTED_STATUSES, SUGGESTED_TYPES } from "./filters";
import { STATUS_SUGGESTIONS, TYPE_SUGGESTIONS } from "./suggestions";

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
