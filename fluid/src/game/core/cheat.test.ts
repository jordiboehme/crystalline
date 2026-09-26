/**
 * The level cheat's word, fed one code and one tick at a time (C1, C2, C7).
 */
import { describe, expect, it } from "vitest";

import {
  CHEAT_GAP_TICKS,
  IDCLEV,
  createCheatReader,
  type CheatReader,
  type CheatStep,
} from "./cheat";

/** The codes of a word's letters. */
const codes = (word: string) => [...word].map((c) => `Key${c.toUpperCase()}`);

/** Feeds a word at one tick and returns every step. */
function feedAll(
  word: string,
  tick = 0,
  reader: CheatReader = createCheatReader(),
): CheatStep[] {
  return codes(word).map((code) => reader.feed(code, tick));
}

const NONE4: CheatStep[] = ["none", "none", "none", "none"];

describe("the level cheat's word", () => {
  it("spells idclev by code", () => {
    expect([...IDCLEV]).toEqual(codes("idclev"));
  });

  it("matches the word typed at once, swallowing its E", () => {
    expect(feedAll("idclev")).toEqual([...NONE4, "swallow", "match"]);
  });

  it("swallows an E only right after i d c l", () => {
    expect(feedAll("e")).toEqual(["none"]);
    expect(feedAll("idce")).toEqual(NONE4);
    expect(feedAll("idclxe")).toEqual([...NONE4, "none", "none"]);
    expect(feedAll("idclee")).toEqual([...NONE4, "swallow", "none"]);
  });

  it("starts again at an I in the middle of the word", () => {
    expect(feedAll("ididclev").at(-1)).toBe("match");
    expect(feedAll("idcidclev").at(-1)).toBe("match");
    expect(feedAll("idclidclev").at(-1)).toBe("match");
  });

  it("breaks the word on any other key between its letters", () => {
    expect(feedAll("idwclev")).not.toContain("match");
    expect(feedAll("idclexv")).not.toContain("match");
  });

  it("forgets a word paused for longer than the gap", () => {
    const kept = createCheatReader();
    feedAll("idc", 0, kept);
    expect(feedAll("lev", CHEAT_GAP_TICKS, kept)).toEqual([
      "none",
      "swallow",
      "match",
    ]);
    const lost = createCheatReader();
    feedAll("idc", 0, lost);
    expect(feedAll("lev", CHEAT_GAP_TICKS + 1, lost)).toEqual([
      "none",
      "none",
      "none",
    ]);
  });

  it("matches again after a match, and forgets a half word on reset", () => {
    const reader = createCheatReader();
    feedAll("idclev", 0, reader);
    expect(feedAll("idclev", 1, reader).at(-1)).toBe("match");
    feedAll("idcl", 2, reader);
    reader.reset();
    expect(feedAll("ev", 2, reader)).toEqual(["none", "none"]);
  });
});
