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
    expect(bytes(line.state)).toBe("a **boldword**\n c");
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
