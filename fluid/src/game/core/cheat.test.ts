/**
 * The level cheat's word, fed one code and one tick at a time (C1, C2).
 */
import { describe, expect, it } from "vitest";

import {
  CHEAT_GAP_TICKS,
  IDCLEV,
  createCheatReader,
  type CheatReader,
} from "./cheat";

/** The codes of a word's letters. */
const codes = (word: string) => [...word].map((c) => `Key${c.toUpperCase()}`);

/** Feeds a word at one tick and returns what each letter answered. */
function feedAll(
  word: string,
  tick = 0,
  reader: CheatReader = createCheatReader(),
): boolean[] {
  return codes(word).map((code) => reader.feed(code, tick));
}

const NO5 = [false, false, false, false, false];

describe("the level cheat's word", () => {
  it("spells idclev by code", () => {
    expect([...IDCLEV]).toEqual(codes("idclev"));
  });

  it("matches the word typed at once, on its V and nowhere else", () => {
    expect(feedAll("idclev")).toEqual([...NO5, true]);
  });

  it("treats the word's E like any other letter", () => {
    expect(feedAll("e")).toEqual([false]);
    expect(feedAll("idcle")).toEqual(NO5);
    expect(feedAll("idclee")).toEqual([...NO5, false]);
    expect(feedAll("idcleev")).not.toContain(true);
  });

  it("starts again at an I in the middle of the word", () => {
    expect(feedAll("ididclev").at(-1)).toBe(true);
    expect(feedAll("idcidclev").at(-1)).toBe(true);
    expect(feedAll("idclidclev").at(-1)).toBe(true);
  });

  it("breaks the word on any other key between its letters", () => {
    expect(feedAll("idwclev")).not.toContain(true);
    expect(feedAll("idclexv")).not.toContain(true);
  });

  it("forgets a word paused for longer than the gap", () => {
    const kept = createCheatReader();
    feedAll("idc", 0, kept);
    expect(feedAll("lev", CHEAT_GAP_TICKS, kept)).toEqual([false, false, true]);
    const lost = createCheatReader();
    feedAll("idc", 0, lost);
    expect(feedAll("lev", CHEAT_GAP_TICKS + 1, lost)).toEqual([
      false,
      false,
      false,
    ]);
  });

  it("matches again after a match, and forgets a half word on reset", () => {
    const reader = createCheatReader();
    feedAll("idclev", 0, reader);
    expect(feedAll("idclev", 1, reader).at(-1)).toBe(true);
    feedAll("idcl", 2, reader);
    reader.reset();
    expect(feedAll("ev", 2, reader)).toEqual([false, false]);
  });
});
