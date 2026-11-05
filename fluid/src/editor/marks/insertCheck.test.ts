/**
 * Paste, drop, completion and typing over a selection at a mark edge (I2):
 * the insert is checked against the pairs around it, the way a deletion is
 * (D5). It lands where it keeps every pair, with whitespace at its edges
 * moved outside the marks it would sit inside, or on the other side of the
 * run. A paste is never refused: where no place keeps the pairs it lands as
 * it was put.
 */

import { EditorSelection } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import {
  bytes,
  caret,
  keys,
  cleanupMarkViews,
  markPeers,
  markView,
  setCaret,
  typeText,
  undoOnce,
} from "../../test/markKit";

afterEach(cleanupMarkViews);

/** A paste of `text` over the selection, as CodeMirror's own paste writes it. */
function paste(view: EditorView, text: string, event = "input.paste"): void {
  view.dispatch(view.state.replaceSelection(text), {
    userEvent: event,
    scrollIntoView: true,
  });
}

function select(view: EditorView, from: number, to: number): void {
  view.dispatch({ selection: EditorSelection.range(from, to) });
}

describe.each([false, true])("an insert at a mark edge, room %s", (room) => {
  it("pastes a word with a space after bold: the space goes after the marks", () => {
    const view = markView("a **bold** c", { room });
    setCaret(view, 10);
    expect(caret(view)).toBe(8);
    paste(view, "word ");
    expect(bytes(view.state)).toBe("a **boldword**  c");
    undoOnce(view);
    expect(bytes(view.state)).toBe("a **bold** c");
  });

  it("pastes a space after a bold that ends on punctuation", () => {
    const view = markView("**a.** b", { room });
    setCaret(view, 6);
    paste(view, " ");
    expect(bytes(view.state)).toBe("**a.**  b");
  });

  it("pastes a leading space before bold at a line start", () => {
    const view = markView("**b** c", { room });
    setCaret(view, 0);
    paste(view, " x");
    expect(bytes(view.state)).toBe(" **xb** c");
  });

  it("types over the space after a bold that ends on punctuation", () => {
    const view = markView("**a.** b", { room });
    select(view, 6, 7);
    typeText(view, "x");
    expect(bytes(view.state)).toBe("**a.x**b");
    undoOnce(view);
    expect(bytes(view.state)).toBe("**a.** b");
  });

  it("types a space over the one bold letter: the emptied pair goes", () => {
    const view = markView("a **b** c", { room });
    select(view, 4, 5);
    typeText(view, " ");
    expect(bytes(view.state)).toBe("a   c");
  });

  it("puts the caret after the pasted text as it shows", () => {
    const view = markView("a **bold** c", { room });
    setCaret(view, 10);
    paste(view, "word ");
    typeText(view, "Z");
    expect(bytes(view.state)).toBe("a **boldword** Z c");
  });

  it("checks a drop and a completion the same way", () => {
    const drop = markView("**a.** b", { room });
    setCaret(drop, 6);
    paste(drop, " ", "input.drop");
    expect(bytes(drop.state)).toBe("**a.**  b");
    const complete = markView("**a.** b", { room });
    setCaret(complete, 6);
    paste(complete, " ", "input.complete");
    expect(bytes(complete.state)).toBe("**a.**  b");
  });

  it("never refuses a paste: where no place keeps the pairs it lands as put", () => {
    const view = markView("a **b** c", { room });
    setCaret(view, 5);
    paste(view, "**");
    expect(bytes(view.state)).toBe("a **b**** c");
  });

  it("pastes lines after bold: the pairs keep, the lines go after the marks (D49)", () => {
    const lines = markView("a **bold** c", { room });
    setCaret(lines, 10);
    paste(lines, "x\n\ny");
    expect(bytes(lines.state)).toBe("a **bold**x\n\ny c");
    // A line copied with its line break, as a triple click takes it.
    const line = markView("a **bold** c", { room });
    setCaret(line, 10);
    paste(line, "word\n");
    // Never split across the run (D55): the whole line goes after it.
    expect(bytes(line.state)).toBe("a **bold**word\n c");
    undoOnce(line);
    expect(bytes(line.state)).toBe("a **bold** c");
  });

  it("leaves a paste that breaks nothing as it is", () => {
    const view = markView("a **bold** c", { room });
    setCaret(view, 6);
    paste(view, "x y");
    expect(bytes(view.state)).toBe("a **box yld** c");
  });
});

describe("an insert where the line shows its marks", () => {
  it("is left as the person wrote it", () => {
    const view = markView("a **bold** c", { mode: "show" });
    setCaret(view, 8);
    paste(view, "word ");
    expect(bytes(view.state)).toBe("a **boldword ** c");
  });
});

describe("an insert in a room with two peers", () => {
  it("reaches the other peer as written, and one undo takes it back", () => {
    const { local, peer, sync } = markPeers("**a.** b");
    setCaret(local, 6);
    paste(local, " ");
    sync();
    expect(bytes(peer.state)).toBe("**a.**  b");
    undoOnce(local);
    sync();
    expect(bytes(peer.state)).toBe("**a.** b");
  });
});

describe.each([false, true])(
  "Enter at the end of a bold line, room %s",
  (room) => {
    it("puts the line break after the closing marks", () => {
      const view = markView("ab**cd**", { room });
      setCaret(view, 8);
      keys(view, "Enter");
      expect(bytes(view.state)).toBe("ab**cd**\n");
      undoOnce(view);
      expect(bytes(view.state)).toBe("ab**cd**");
    });

    it("keeps a bold that a line break inside it splits, as the page reads it", () => {
      const view = markView("**ab cd**", { room });
      setCaret(view, 5);
      keys(view, "Enter");
      expect(bytes(view.state)).toBe("**ab \ncd**");
    });
  },
);

/**
 * A paste of text that holds marks into a pair, in hidden and show mode: the
 * pasted marks of a format the place already has are left out, so `**q**`
 * pasted into bold is a bold `q` and the pair around it stays one pair.
 */
describe.each(
  (["hidden", "show"] as const).flatMap((mode) =>
    [false, true].map((room) => ({ mode, room })),
  ),
)(
  "a paste with marks into a pair, $mode mode, room $room",
  ({ mode, room }) => {
    it.each([
      ["**word a** z", 2, "**q**", "**qword a** z"],
      ["**word a** z", 4, "**q**", "**woqrd a** z"],
      ["**word a** z", 8, "**q**", "**word aq** z"],
      ["*word a* z", 1, "*q*", "*qword a* z"],
      ["~~word a~~ z", 2, "~~q~~", "~~qword a~~ z"],
      ["**word a** z", 4, "**q** r", "**woq rrd a** z"],
      ["**word a** z", 4, "*q*", "**wo*q*rd a** z"],
      ["a word z", 2, "**q**", "a **q**word z"],
      // A picture's or a link's own marks are its bytes, kept as copied.
      [
        "**word a** z",
        4,
        "![**x**](assets/a.png)",
        "**wo![**x**](assets/a.png)rd a** z",
      ],
      [
        "**word a** z",
        4,
        "[**x**](https://u.v)",
        "**wo[**x**](https://u.v)rd a** z",
      ],
    ])("%j, at %i, %j", (text, at, pasted, want) => {
      const view = markView(text, { mode, room });
      setCaret(view, at);
      view.dispatch({ selection: EditorSelection.cursor(at) });
      paste(view, pasted);
      expect(bytes(view.state)).toBe(want);
      undoOnce(view);
      expect(bytes(view.state)).toBe(text);
    });
  },
);

/**
 * Enter on a bold line (D55): the line break is never split across a run.
 * It goes whole after the closing marks at an item's end, and before the
 * format's marks at the text's start, so the new line starts the same
 * block (bullet, number, task box, quote) as Enter where the marks show.
 */
const ENTER: [string, string, number | "end", string][] = [
  ["a bullet, at its end", "- **a**", "end", "- **a**\n- "],
  ["a bullet, at its text start", "- **ab**", 2, "- \n- **ab**"],
  ["an ordered item, at its end", "1. **a**", "end", "1. **a**\n2. "],
  ["an ordered item, at its text start", "1. **ab**", 3, "1. \n2. **ab**"],
  [
    "a nested item, at its end",
    "- a\n  - **b**",
    "end",
    "- a\n  - **b**\n  - ",
  ],
  [
    "a nested item, at its text start",
    "- a\n  - **bc**",
    8,
    "- a\n  - \n  - **bc**",
  ],
  ["a task, at its end", "- [ ] **a**", "end", "- [ ] **a**\n- [ ] "],
  ["a task, at its text start", "- [ ] **ab**", 6, "- [ ] \n- [ ] **ab**"],
  ["a quoted item, at its end", "> - **a**", "end", "> - **a**\n> - "],
  ["a quoted item, at its text start", "> - **ab**", 4, "> - \n> - **ab**"],
  ["a plain line, at its end", "**ab**", "end", "**ab**\n"],
  ["a plain line, at its start", "**ab**", 0, "\n**ab**"],
  ["a heading, at its end", "## **h**", "end", "## **h**\n"],
];

describe.each(
  (["hidden", "show"] as const).flatMap((mode) =>
    [false, true].map((room) => ({ mode, room })),
  ),
)("Enter on a bold line, $mode mode, room $room", ({ mode, room }) => {
  it.each(ENTER)("%s", (_name, text, at, want) => {
    const view = markView(text, { mode, room });
    setCaret(view, at === "end" ? text.length : at);
    keys(view, "Enter");
    expect(bytes(view.state)).toBe(want);
    undoOnce(view);
    expect(bytes(view.state)).toBe(text);
  });
});

/**
 * Enter at the end of a format, with the caret before its closing marks
 * (where the caret is drawn in hidden mode): the line break goes after the
 * marks. A single `*` moved to the next line would start a list item there,
 * so no place that makes a block mark out of a format mark is taken.
 */
describe.each(
  (["hidden", "show"] as const).flatMap((mode) =>
    [false, true].map((room) => ({ mode, room })),
  ),
)("Enter at the end of a format, $mode mode, room $room", ({ mode, room }) => {
  it.each([
    ["*a word* z", 7, "*a word*\n z"],
    ["_a word_ z", 7, "_a word_\n z"],
    ["**a word** z", 8, "**a word**\n z"],
    ["~~a word~~ z", 8, "~~a word~~\n z"],
    ["x *a word*", 9, "x *a word*\n"],
    ["- *a word* z", 9, "- *a word*\n-  z"],
  ])("%j, Enter at %i", (text, at, want) => {
    const view = markView(text, { mode, room });
    setCaret(view, at);
    keys(view, "Enter");
    expect(bytes(view.state)).toBe(want);
    undoOnce(view);
    expect(bytes(view.state)).toBe(text);
  });
});

/**
 * A real list line still continues as a list when Enter is judged (P48),
 * with marks on it or none, in both modes.
 */
describe.each(
  (["hidden", "show"] as const).flatMap((mode) =>
    [false, true].map((room) => ({ mode, room })),
  ),
)(
  "Enter at the end of a list line, $mode mode, room $room",
  ({ mode, room }) => {
    it.each([
      ["* item", "* item\n* "],
      ["- item", "- item\n- "],
      ["1. item", "1. item\n2. "],
      ["* *item*", "* *item*\n* "],
      ["- **item**", "- **item**\n- "],
      ["1. ~~item~~", "1. ~~item~~\n2. "],
    ])("%j", (text, want) => {
      const view = markView(text, { mode, room });
      setCaret(view, text.length);
      keys(view, "Enter");
      expect(bytes(view.state)).toBe(want);
      undoOnce(view);
      expect(bytes(view.state)).toBe(text);
    });
  },
);

/**
 * Enter between two formats that touch (D61): the run there closes one
 * format and opens the next. The line break goes between the two, so each
 * keeps its pair.
 */
describe.each([false, true])(
  "Enter between touching formats, room %s",
  (room) => {
    it.each([
      ["**a***b*", "a", "**a**\n*b*"],
      ["**a**_b_", "a", "**a**\n_b_"],
      ["*a***b**", "a", "*a*\n**b**"],
      ["**a***b*", "b", "**a**\n*b*"],
    ])("%s, Enter at %s's edge", (text, letter, want) => {
      const view = markView(text, { room });
      const at = text.indexOf(letter);
      setCaret(view, letter === "a" ? at + 1 : at);
      keys(view, "Enter");
      expect(bytes(view.state)).toBe(want);
      undoOnce(view);
      expect(bytes(view.state)).toBe(text);
    });
  },
);
