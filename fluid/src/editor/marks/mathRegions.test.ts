/**
 * Math the page draws holds no Markdown marks (D41): the lines of a `$$`
 * block are formula source, and an unclosed `$$` runs to the end of the
 * document, as the page reads it. A backslash there is LaTeX, not an escape.
 */

import { afterEach, describe, expect, it } from "vitest";

import {
  bytes,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
} from "../../test/markKit";
import { marksOnLine } from "./hiddenMarks";
import { parsedState } from "../../test/parse";

afterEach(cleanupMarkViews);

describe.each(["hidden", "show"] as const)("math, %s mode", (mode) => {
  it("reads no marks inside a closed $$ block", () => {
    const view = markView("$$\nx **y** z\n$$\n\n**b**", { mode });
    // The caret on the opening line: in show mode only that line shows.
    setCaret(view, 0);
    expect(marksOnLine(parsedState(view.state), 2)).toEqual([]);
    expect(marksOnLine(parsedState(view.state), 2, { always: true })).toEqual(
      [],
    );
    expect(marksOnLine(parsedState(view.state), 5)).toHaveLength(2);
  });

  it("reads an unclosed $$ to the end of the document", () => {
    const view = markView("$$\nx_1 **y**\n\nlater **para**", { mode });
    setCaret(view, 0);
    expect(marksOnLine(parsedState(view.state), 2)).toEqual([]);
    expect(marksOnLine(parsedState(view.state), 4)).toEqual([]);
    expect(view.contentDOM.textContent).toContain("later **para**");
  });

  it("reads marks again once a $$ is closed below", () => {
    const view = markView("$$\nx **y**\n\nlater\n$$\n\n**z**", { mode });
    setCaret(view, 0);
    expect(marksOnLine(parsedState(view.state), 2)).toEqual([]);
    expect(marksOnLine(parsedState(view.state), 7)).toHaveLength(2);
  });

  it("takes no backslash with the character after it", () => {
    const view = markView("$$\n\\{x\n$$", { mode });
    setCaret(view, 5);
    keys(view, "Backspace");
    expect(bytes(view.state)).toBe("$$\n\\x\n$$");
  });
});

describe("math on a page read section by section", () => {
  it("ends an unclosed $$ at the next section heading", () => {
    const view = markView("## A\n\n$$\nx **y**\n\n## B\n\n**z**", {
      adapter: "reflection",
    });
    expect(marksOnLine(parsedState(view.state), 4)).toEqual([]);
    expect(marksOnLine(parsedState(view.state), 8)).toHaveLength(2);
  });
});

describe("math in a quote", () => {
  it("keeps the quote marks of a $$ block's lines", () => {
    const view = markView("> [!NOTE]\n> $$\n> x **y**\n> $$\n\nEnd", {
      mode: "show",
    });
    // The caret inside the formula: its source shows, its quote bar stays.
    setCaret(view, 16);
    for (const line of [3, 4]) {
      expect(
        marksOnLine(parsedState(view.state), line).map((mark) => mark.kind),
      ).toEqual(line === 3 ? [] : ["quote"]);
    }
    expect(marksOnLine(parsedState(view.state), 3, { always: true })).toEqual([
      expect.objectContaining({ kind: "quote" }),
    ]);
  });

  it("keeps the quote marks of an unclosed $$ in a quote", () => {
    const view = markView("> $$\n> x **y**\n\n**z**", { mode: "hidden" });
    expect(
      marksOnLine(parsedState(view.state), 2).map((mark) => mark.kind),
    ).toEqual(["quote"]);
    expect(marksOnLine(parsedState(view.state), 4)).toHaveLength(2);
  });
});
