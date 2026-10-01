import { describe, expect, it } from "vitest";
import { CUT_MARK, fitRows, hardWrap, plainInline, wordWrap } from "./textFlow";

const SENTENCE =
  "the quick brown fox jumps over the lazy dog while seven brave knights ride past the old mill and every bell in the valley rings out loud at dawn so that the whole village wakes up early today and walks down to the river to watch the boats";

describe("fitRows", () => {
  it("shows a link as its text", () => {
    // Mutation caught: the URL kept (the link not reduced by plainInline).
    expect(
      fitRows(["[VISION.md](https://github.com/example/VISION.md)"], 20, 3),
    ).toEqual(["VISION.md"]);
  });

  it("wraps at word boundaries and cuts the last row with the mark", () => {
    // Mutation caught: a row wider than `columns`, no mark on the cut row,
    // or a first row cut mid-word where a space exists.
    expect(SENTENCE.length).toBeGreaterThan(200);
    const rows = fitRows([SENTENCE], 20, 4);
    expect(rows).toHaveLength(4);
    for (const row of rows) expect(row.length).toBeLessThanOrEqual(20);
    const words = new Set(SENTENCE.split(" "));
    for (const row of rows.slice(0, 3)) {
      for (const word of row.split(" ")) expect(words.has(word)).toBe(true);
    }
    expect(rows[3]?.endsWith(CUT_MARK)).toBe(true);
    expect(rows.slice(0, 3).some((r) => r.includes(CUT_MARK))).toBe(false);
  });

  it("cuts the last row at its last space when that lies in the last third", () => {
    // Mutation caught: always cutting at columns - 1, which splits a word
    // when a space is near the end of the row.
    expect(fitRows(["aaaa bbbb cccc dddd eeee ffff"], 20, 1)).toEqual([
      `aaaa bbbb cccc${CUT_MARK}`,
    ]);
  });

  it("cuts mid-word when the row's last space lies early", () => {
    // Mutation caught: cutting at any space, which would leave the row
    // nearly empty.
    expect(fitRows(["ab cdefghijklmnopqrs tuv"], 20, 1)).toEqual([
      `ab cdefghijklmnopqr${CUT_MARK}`,
    ]);
  });

  it("breaks a word longer than a row", () => {
    // Mutation caught: a long word left whole, wider than the row.
    expect(fitRows(["abcdefghijklmnopqrstuvwxyz"], 10, 5)).toEqual([
      "abcdefghij",
      "klmnopqrst",
      "uvwxyz",
    ]);
  });

  it("drops empty and blank lines and handles no lines or no rows", () => {
    // Mutation caught: an empty line kept as a row, or rows 0 giving a row.
    expect(fitRows(["a", "", "   ", "b"], 20, 5)).toEqual(["a", "b"]);
    expect(fitRows([], 20, 3)).toEqual([]);
    expect(fitRows(["abc"], 20, 0)).toEqual([]);
  });

  it("gives no mark when the text fits exactly", () => {
    // Mutation caught: `>=` for `>` in the overflow test.
    expect(fitRows(["one two three"], 7, 2)).toEqual(["one two", "three"]);
    expect(fitRows(["one", "two"], 20, 2)).toEqual(["one", "two"]);
    expect(fitRows(["one", "two", "three"], 20, 2)).toEqual([
      "one",
      `two${CUT_MARK}`,
    ]);
  });
});

describe("the moved rules", () => {
  it("keep their behaviour", () => {
    // Mutation caught: a rule changed in the move.
    expect(plainInline("a [[Target]] and **bold** and `code`")).toBe(
      "a Target and bold and code",
    );
    expect(hardWrap("abcdef", 4)).toEqual(["abcd", "ef"]);
    expect(wordWrap("aa bb cc", 5)).toEqual(["aa bb", "cc"]);
  });
});
