/**
 * The deletion check (D5, D6): a deletion that would stop a neighbouring
 * pair from parsing is repaired with one named repair or refused, on every
 * delete path, solo and in a room, with marks hidden and shown.
 */

import {
  deleteGroupBackward,
  deleteGroupForward,
  deleteLineBoundaryBackward,
} from "@codemirror/commands";
import { EditorSelection, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  typeText,
  undoOnce,
} from "../../test/markKit";
import type { DeleteOutcome } from "./deleteCheck";
import { CANNOT_JOIN, deletionOutcome } from "./deleteCheck";

afterEach(cleanupMarkViews);

type Mode = "hidden" | "show";

const CASES = (["hidden", "show"] as Mode[]).flatMap((mode) =>
  [false, true].map((room) => ({ mode, room })),
);

/** A view on `x\n` + `line`, hearing outcomes and announcements. */
function page(line: string, mode: Mode, room: boolean) {
  const view = markView(`x\n${line}`, { mode, room });
  const heard: { outcome: DeleteOutcome | null; said: string[] } = {
    outcome: null,
    said: [],
  };
  view.dispatch({
    effects: StateEffect.appendConfig.of(
      EditorView.updateListener.of((update) => {
        for (const tr of update.transactions) {
          for (const effect of tr.effects) {
            if (effect.is(deletionOutcome)) heard.outcome = effect.value;
            if (effect.is(EditorView.announce)) heard.said.push(effect.value);
          }
        }
      }),
    ),
  });
  return { view, heard, text: () => bytes(view.state).slice(2) };
}

describe.each(CASES)(
  "the deletion check, $mode mode, room $room",
  ({ mode, room }) => {
    it("(a) moves the opening mark across the space Delete leaves", () => {
      const { view, heard, text } = page("**A b**", mode, room);
      setCaret(view, 4);
      keys(view, "Delete");
      expect(text()).toBe(" **b**");
      expect(heard.outcome).toBe("moved");
      undoOnce(view);
      expect(text()).toBe("**A b**");
    });

    it("(a) moves the closing mark across the space Backspace leaves", () => {
      const { view, heard, text } = page("**a b**", mode, room);
      setCaret(view, 7);
      keys(view, "Backspace");
      expect(text()).toBe("**a** ");
      expect(heard.outcome).toBe("moved");
    });

    it("(b) merges two bold words whose space Delete takes", () => {
      const { view, heard, text } = page("**a** **b**", mode, room);
      // In show mode the closing marks are text: the caret goes past them.
      setCaret(view, mode === "show" ? 7 : 5);
      keys(view, "Delete");
      expect(text()).toBe("**ab**");
      expect(heard.outcome).toBe("merged");
      undoOnce(view);
      expect(text()).toBe("**a** **b**");
    });

    it("(b) merges italics and code spans the same way", () => {
      const italic = page("*a* *b*", mode, room);
      setCaret(italic.view, mode === "show" ? 5 : 4);
      keys(italic.view, "Delete");
      expect(italic.text()).toBe("*ab*");
      const code = page("`a`/`b`", mode, room);
      setCaret(code.view, 5);
      keys(code.view, "Delete");
      expect(code.text()).toBe("`ab`");
      expect(code.heard.outcome).toBe("merged");
    });

    it("(c) refuses the space after a closer that follows punctuation", () => {
      const { view, heard, text } = page("**a.** b", mode, room);
      setCaret(view, 8);
      const at = caret(view);
      keys(view, "Delete");
      expect(text()).toBe("**a.** b");
      expect(heard.outcome).toBe("refused");
      expect(heard.said).toEqual([CANNOT_JOIN]);
      expect(caret(view)).toBe(at);
    });

    it("(c) refuses the space before an opener that precedes punctuation", () => {
      const { view, heard, text } = page("a *__b__*", mode, room);
      // In show mode the opening mark is text: the caret stays before it.
      setCaret(view, mode === "show" ? 4 : 5);
      keys(view, "Backspace");
      expect(text()).toBe("a *__b__*");
      expect(heard.outcome).toBe("refused");
    });

    it("checks a selection deleted: a merge and a refusal", () => {
      const merge = page("**a** **b** c", mode, room);
      merge.view.dispatch({ selection: EditorSelection.range(7, 8) });
      keys(merge.view, "Backspace");
      expect(merge.text()).toBe("**ab** c");
      const refuse = page("**a.** b", mode, room);
      refuse.view.dispatch({ selection: EditorSelection.range(8, 9) });
      keys(refuse.view, "Backspace");
      expect(refuse.text()).toBe("**a.** b");
      expect(refuse.heard.outcome).toBe("refused");
    });

    it("checks the word and line deletes", () => {
      const word = page("**a.** b", mode, room);
      setCaret(word.view, 9);
      deleteGroupBackward(word.view);
      // Whatever the word delete took, the bold that is left still parses.
      expect(word.text()).not.toMatch(/\*\*a\.\*\*b/);
      const line = page("q **a.** b", mode, room);
      setCaret(line.view, 11);
      deleteLineBoundaryBackward(line.view);
      expect(line.text()).toBe("b");
    });

    it("keeps the exact deletion where nothing breaks", () => {
      const { view, heard, text } = page("a **bold** c", mode, room);
      setCaret(view, 10);
      keys(view, "Backspace");
      expect(text()).toBe("a **bol** c");
      expect(heard.outcome).toBeNull();
    });

    it("lets a letter of a shortcut reference go (D7)", () => {
      const { view, heard, text } = page("[ref] x\n\n[ref]: u", mode, room);
      setCaret(view, mode === "show" ? 3 : 2);
      keys(view, "Delete");
      expect(text()).toBe("[ef] x\n\n[ref]: u");
      expect(heard.outcome).toBeNull();
    });

    it("removes a code span whose last letter Backspace takes (D6)", () => {
      const { view, text } = page("`x`**y** and ~~s~~*t*.", mode, room);
      setCaret(view, 4);
      keys(view, "Backspace");
      expect(text()).toBe("**y** and ~~s~~*t*.");
    });
  },
);

describe.each([false, true])("a new pair, room %s", (room) => {
  it("is refused on a line with no pair yet where marks are hidden (D10)", () => {
    const { view, heard, text } = page("q * a* r", "hidden", room);
    setCaret(view, 6);
    keys(view, "Backspace");
    expect(text()).toBe("q * a* r");
    expect(heard.outcome).toBe("refused");
    expect(heard.said).toEqual([CANNOT_JOIN]);
  });

  it("is left to today's keys on a shown line with no pair, beside a pair on another line (D11)", () => {
    // The paragraph holds a pair, but not on the line the key touched.
    const below = page("x **b** y\nq * a* r", "show", room);
    setCaret(below.view, 16);
    keys(below.view, "Backspace");
    expect(below.text()).toBe("x **b** y\nq *a* r");
    expect(below.heard.outcome).toBeNull();
    const above = page("q * a* r\nx **b** y", "show", room);
    setCaret(above.view, 6);
    keys(above.view, "Backspace");
    expect(above.text()).toBe("q *a* r\nx **b** y");
    expect(above.heard.outcome).toBeNull();
    // Where marks are hidden the same key is refused (D10).
    const hidden = page("x **b** y\nq * a* r", "hidden", room);
    setCaret(hidden.view, 16);
    keys(hidden.view, "Backspace");
    expect(hidden.text()).toBe("x **b** y\nq * a* r");
    expect(hidden.heard.outcome).toBe("refused");
  });

  it("is checked on a shown line that a wrapped pair spans", () => {
    // No mark on the touched line, but a bold runs across it.
    const { view, heard, text } = page("**b\nq * a* r\nc**", "show", room);
    setCaret(view, 10);
    keys(view, "Backspace");
    expect(text()).toBe("**b\nq * a* r\nc**");
    expect(heard.outcome).toBe("refused");
  });

  it("is left to today's keys when a shown mark is deleted (D9)", () => {
    // The first backtick goes: the spans after it pair anew, as typed.
    const { view, heard, text } = page("`a` and `b`", "show", room);
    setCaret(view, 3);
    keys(view, "Backspace");
    expect(text()).toBe("a` and `b`");
    expect(heard.outcome).toBeNull();
  });

  it("is refused in show mode when the new pair reaches a line whose marks are hidden (D23)", () => {
    // `*a r\nb*` would pair: the opener shows, the closer does not, and the
    // `b` below would turn italic with no mark on screen.
    const { view, heard, text } = page("q * a r\nb* c", "show", room);
    setCaret(view, 6);
    keys(view, "Backspace");
    expect(text()).toBe("q * a r\nb* c");
    expect(heard.outcome).toBe("refused");
    expect(heard.said).toEqual([CANNOT_JOIN]);
  });

  it("is refused in show mode when a key takes a shown mark and a normal character, and a hidden line changes (D28)", () => {
    // A word delete after `q *` takes the space and the backtick: the space
    // makes `*\`a\` r\nb*` pair, so `b` below would turn italic.
    const word = page("q * `a` r\nb* c", "show", room);
    setCaret(word.view, 5);
    deleteGroupForward(word.view);
    expect(word.text()).toBe("q * `a` r\nb* c");
    expect(word.heard.outcome).toBe("refused");
    expect(word.heard.said).toEqual([CANNOT_JOIN]);
    // A selection over the same two characters, the same.
    const picked = page("q * `a` r\nb* c", "show", room);
    picked.view.dispatch({ selection: EditorSelection.range(5, 7) });
    keys(picked.view, "Backspace");
    expect(picked.text()).toBe("q * `a` r\nb* c");
    expect(picked.heard.outcome).toBe("refused");
  });

  it("is left to today's keys in show mode when a shown mark goes, though its partner's line hides its marks (D25)", () => {
    // The opener of a bold that wraps: the person edits the Markdown they
    // see, and `b` below loses its bold, as typed (D9).
    const { view, heard, text } = page("**a\nb** c", "show", room);
    setCaret(view, 4);
    keys(view, "Backspace");
    expect(text()).toBe("*a\nb** c");
    expect(heard.outcome).toBeNull();
    expect(heard.said).toEqual([]);
  });
});

/** A cut as the browser sends it; what reached the clipboard comes back. */
function cut(view: EditorView): string {
  const data = new Map<string, string>();
  const event = new Event("cut", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    value: {
      clearData: () => {
        data.clear();
      },
      setData: (type: string, value: string) => data.set(type, value),
      getData: (type: string) => data.get(type) ?? "",
    },
  });
  view.contentDOM.dispatchEvent(event);
  return data.get("text/plain") ?? "";
}

describe.each(CASES)("round 3, $mode mode, room $room", ({ mode, room }) => {
  // In show mode the closing marks are text: the caret goes past them.
  const past = (hidden: number, shown: number) =>
    mode === "show" ? shown : hidden;

  it("(c) across a line break is refused", () => {
    const { view, heard, text } = page("**a\nb.** c", mode, room);
    setCaret(view, past(8, 10));
    keys(view, "Delete");
    expect(text()).toBe("**a\nb.** c");
    expect(heard.outcome).toBe("refused");
  });

  it("(a) across a line break moves the opening mark", () => {
    const { view, heard, text } = page("**A spread\nbold**", mode, room);
    setCaret(view, 4);
    keys(view, "Delete");
    expect(text()).toBe(" **spread\nbold**");
    expect(heard.outcome).toBe("moved");
  });

  it("(b) across a line break merges the two pairs", () => {
    const { view, heard, text } = page("**one\nmore** **two**", mode, room);
    setCaret(view, past(12, 14));
    keys(view, "Delete");
    expect(text()).toBe("**one\nmoretwo**");
    expect(heard.outcome).toBe("merged");
  });

  it("keeps the caret where it shows after a move", () => {
    // "a " with the caret after the space: Z shows "a Z".
    const back = page("**a b**", mode, room);
    setCaret(back.view, 7);
    keys(back.view, "Backspace");
    typeText(back.view, "Z");
    expect(back.text()).toBe("**a** Z");
    // " bold" with the caret before the space: Z shows "Z bold".
    const forward = page("**A bold**", mode, room);
    setCaret(forward.view, 4);
    keys(forward.view, "Delete");
    typeText(forward.view, "Z");
    expect(forward.text()).toBe("Z **bold**");
  });

  it("checks a cut the same way (D12)", () => {
    const { view, heard, text } = page("**a.** b", mode, room);
    view.focus();
    view.dispatch({ selection: EditorSelection.range(8, 9) });
    expect(cut(view)).toBe(" ");
    expect(text()).toBe("**a.** b");
    expect(heard.outcome).toBe("refused");
  });

  it("leaves a delete alone while a composition is active", () => {
    const { view, heard, text } = page("**a.** b", mode, room);
    Object.defineProperty(view, "composing", { get: () => true });
    view.dispatch({
      changes: { from: 8, to: 9 },
      userEvent: "delete.forward",
    });
    expect(text()).toBe("**a.**b");
    expect(heard.outcome).toBeNull();
  });
});

describe.each(CASES)(
  "a delete on a fence line (M2), $mode mode, room $room",
  ({ mode, room }) => {
    it("refuses to turn an unclosed fence's code into bold text", () => {
      const { view, heard, text } = page("```math\nx^2 **y**", mode, room);
      setCaret(view, 2);
      keys(view, "Delete");
      expect(text()).toBe("```math\nx^2 **y**");
      expect(heard.outcome).toBe("refused");
      expect(heard.said).toContain(CANNOT_JOIN);
    });

    it("refuses to turn a closed fence's code into bold text", () => {
      // In a quote, where no code box hides the fence lines.
      const { view, heard, text } = page("> ```\n> x **y**\n> ```", mode, room);
      setCaret(view, 4);
      keys(view, "Delete");
      expect(text()).toBe("> ```\n> x **y**\n> ```");
      expect(heard.outcome).toBe("refused");
    });

    it("lets a fence line change when no code line would read differently", () => {
      const { view, text } = page("> ```\n> plain words\n> ```", mode, room);
      setCaret(view, 4);
      keys(view, "Delete");
      expect(text()).toBe("> ``\n> plain words\n> ```");
    });
  },
);

describe("a fence typed by hand and taken back (M2 as ruled, D42)", () => {
  it("refuses the Backspace that would bring the text below back, and one undo does", () => {
    const { view, heard, text } = page("a\n\n**b** c", "hidden", false);
    setCaret(view, 4);
    typeText(view, "```");
    expect(text()).toBe("a\n```\n**b** c");
    keys(view, "Backspace");
    // The text below the fence would turn bold again: refused (D10).
    expect(text()).toBe("a\n```\n**b** c");
    expect(heard.outcome).toBe("refused");
    undoOnce(view);
    expect(text()).toBe("a\n\n**b** c");
  });
});
