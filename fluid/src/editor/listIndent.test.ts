/**
 * Tab and Shift+Tab on list items: nested under the previous sibling, lifted
 * to the parent's level, the whole item with it, and everywhere else left to
 * the browser's focus move. Every claim is on the bytes.
 */

import { EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  undoOnce,
} from "../test/markKit";

afterEach(cleanupMarkViews);

type Mode = "hidden" | "show";
const MODES: Mode[] = ["hidden", "show"];

/** `key` pressed with the caret right after `after` (first match). */
function press(
  doc: string,
  after: string,
  key: string,
  options: { mode?: Mode; raw?: boolean } = {},
): { view: EditorView; handled: boolean } {
  const view = markView(doc, options);
  const at = doc.indexOf(after);
  if (at < 0) throw new Error(`no ${after} in the document`);
  setCaret(view, at + after.length);
  return { view, handled: keys(view, key) };
}

function tab(doc: string, after: string, key = "Tab", mode?: Mode): string {
  const { view, handled } = press(doc, after, key, mode ? { mode } : {});
  expect(handled).toBe(true);
  return bytes(view.state);
}

describe("Tab on a list item", () => {
  for (const mode of MODES) {
    it(`nests it under the previous sibling (${mode})`, () => {
      expect(tab("- a\n- b", "- b", "Tab", mode)).toBe("- a\n  - b");
      expect(tab("* a\n* b", "* b", "Tab", mode)).toBe("* a\n  * b");
      expect(tab("+ a\n+ b", "+ b", "Tab", mode)).toBe("+ a\n  + b");
      expect(tab("1. a\n2. b", "2. b", "Tab", mode)).toBe("1. a\n   2. b");
      expect(tab("1) a\n2) b", "2) b", "Tab", mode)).toBe("1) a\n   2) b");
      expect(tab("10. a\n11. b", "11. b", "Tab", mode)).toBe(
        "10. a\n    11. b",
      );
    });
  }

  it("nests a task item under the content column of the one before", () => {
    expect(tab("- [ ] a\n- [x] b", "b")).toBe("- [ ] a\n  - [x] b");
    expect(tab("- a\n- [ ] b", "b")).toBe("- a\n  - [ ] b");
  });

  it("takes the sibling's own spacing after its marker", () => {
    expect(tab("-   a\n-   b", "b")).toBe("-   a\n    -   b");
    // Five spaces and more are the marker plus one, the rest is code.
    expect(tab("-      a\n- b", "- b")).toBe("-      a\n  - b");
  });

  it("nests under the sibling at the same level, past its children", () => {
    expect(tab("- a\n  - a1\n- b", "- b")).toBe("- a\n  - a1\n  - b");
    expect(tab("- a\n  - a1\n  - a2", "a2")).toBe("- a\n  - a1\n    - a2");
  });

  it("moves the whole item: continuation lines and children", () => {
    expect(tab("- a\n- b\n  more\n  - c\n    - d\n- e", "- b")).toBe(
      "- a\n  - b\n    more\n    - c\n      - d\n- e",
    );
  });

  it("does not touch blank lines inside the item", () => {
    expect(tab("- a\n- b\n\n  more\n- c", "- b")).toBe(
      "- a\n  - b\n\n    more\n- c",
    );
  });

  it("keeps the numbers of an ordered list", () => {
    expect(tab("1. a\n2. b\n3. c", "2. b")).toBe("1. a\n   2. b\n3. c");
  });

  it("takes the key and changes nothing on a first item", () => {
    expect(tab("- a\n- b", "- a")).toBe("- a\n- b");
    expect(tab("- a\n  - b", "  - b")).toBe("- a\n  - b");
    // A bullet after an ordered list starts a list of its own.
    expect(tab("1. a\n- b", "- b")).toBe("1. a\n- b");
  });

  it("keeps the caret on the same text", () => {
    const { view } = press("- a\n- bc", "- b", "Tab");
    expect(bytes(view.state)).toBe("- a\n  - bc");
    expect(view.state.sliceDoc(caret(view), caret(view) + 1)).toBe("c");
  });

  it("keeps a caret at the line start, in show mode, before the marker", () => {
    const view = markView("- a\n- b", { mode: "show" });
    setCaret(view, 4);
    expect(keys(view, "Tab")).toBe(true);
    expect(bytes(view.state)).toBe("- a\n  - b");
    expect(view.state.sliceDoc(caret(view), caret(view) + 3)).toBe("- b");
  });

  it("nests several selected items together", () => {
    const view = markView("- a\n- b\n- c\n- d");
    view.dispatch({ selection: EditorSelection.range(6, 11) });
    expect(keys(view, "Tab")).toBe(true);
    expect(bytes(view.state)).toBe("- a\n  - b\n  - c\n- d");
    expect(view.state.sliceDoc(view.state.selection.main.from)).toMatch(
      /^b\n {2}- c/,
    );
  });

  it("moves a selected item and its selected child once", () => {
    const view = markView("- a\n- b\n  - c");
    view.dispatch({ selection: EditorSelection.range(6, 13) });
    keys(view, "Tab");
    expect(bytes(view.state)).toBe("- a\n  - b\n    - c");
  });

  it("does not count a line the selection ends at the start of", () => {
    const view = markView("- a\n- b\n\nplain");
    view.dispatch({ selection: EditorSelection.range(4, 8) });
    expect(keys(view, "Tab")).toBe(true);
    expect(bytes(view.state)).toBe("- a\n  - b\n\nplain");
  });

  it("works in Raw too", () => {
    const { view, handled } = press("- a\n- b", "- b", "Tab", { raw: true });
    expect(handled).toBe(true);
    expect(bytes(view.state)).toBe("- a\n  - b");
  });

  it("is one undo step, and two presses are two", () => {
    const view = markView("- a\n  - b\n- c");
    setCaret(view, view.state.doc.length);
    keys(view, "Tab");
    expect(bytes(view.state)).toBe("- a\n  - b\n  - c");
    keys(view, "Tab");
    expect(bytes(view.state)).toBe("- a\n  - b\n    - c");
    undoOnce(view);
    expect(bytes(view.state)).toBe("- a\n  - b\n  - c");
    undoOnce(view);
    expect(bytes(view.state)).toBe("- a\n  - b\n- c");
  });

  it("is one undo step in a room", () => {
    const view = markView("- a\n- b", { room: true });
    setCaret(view, 7);
    keys(view, "Tab");
    expect(bytes(view.state)).toBe("- a\n  - b");
    undoOnce(view);
    expect(bytes(view.state)).toBe("- a\n- b");
  });
});

describe("Shift+Tab on a list item", () => {
  for (const mode of MODES) {
    it(`lifts it to the parent's level (${mode})`, () => {
      expect(tab("- a\n  - b", "- b", "Shift-Tab", mode)).toBe("- a\n- b");
      expect(tab("1. a\n   2. b", "2. b", "Shift-Tab", mode)).toBe(
        "1. a\n2. b",
      );
      expect(tab("- a\n  - b\n    - c", "- c", "Shift-Tab", mode)).toBe(
        "- a\n  - b\n  - c",
      );
    });
  }

  it("lifts the whole item with its children", () => {
    expect(tab("- a\n  - b\n    more\n    - c\n- d", "- b", "Shift-Tab")).toBe(
      "- a\n- b\n  more\n  - c\n- d",
    );
  });

  it("makes the siblings after it its children, as CommonMark reads it", () => {
    expect(tab("- a\n  - b\n  - c", "- b", "Shift-Tab")).toBe(
      "- a\n- b\n  - c",
    );
  });

  it("takes the key and changes nothing at the top level", () => {
    expect(tab("- a\n- b", "- b", "Shift-Tab")).toBe("- a\n- b");
  });

  it("is undone in one step", () => {
    const view = markView("- a\n  - b");
    setCaret(view, view.state.doc.length);
    keys(view, "Shift-Tab");
    expect(bytes(view.state)).toBe("- a\n- b");
    undoOnce(view);
    expect(bytes(view.state)).toBe("- a\n  - b");
  });
});

describe("Tab off a list item", () => {
  it("is left to the browser on a plain line, so the focus moves", () => {
    const { view, handled } = press("Plain\n- a", "Plain", "Tab");
    expect(handled).toBe(false);
    expect(bytes(view.state)).toBe("Plain\n- a");
    expect(press("Plain", "Plain", "Shift-Tab").handled).toBe(false);
  });

  it("is left to the browser when one selected line is no list item", () => {
    const view = markView("- a\n- b\nplain");
    view.dispatch({ selection: EditorSelection.range(5, 12) });
    expect(keys(view, "Tab")).toBe(false);
    expect(bytes(view.state)).toBe("- a\n- b\nplain");
  });

  it("is left to the browser on a list item in a quote", () => {
    expect(press("> - a\n> - b", "> - b", "Tab").handled).toBe(false);
  });

  it("is left to the browser in a fenced code block", () => {
    expect(press("```\n- a\n- b\n```", "- b", "Tab").handled).toBe(false);
  });

  it("is left to the browser in the frontmatter's YAML list", () => {
    const doc = "---\ntags:\n  - a\n  - b\n---\n\nText";
    const { view, handled } = press(doc, "  - b", "Tab");
    expect(handled).toBe(false);
    expect(bytes(view.state)).toBe(doc);
  });

  it("is left to the browser on a read-only page", () => {
    const view = markView("- a\n- b");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    setCaret(view, 7);
    expect(keys(view, "Tab")).toBe(false);
    expect(bytes(view.state)).toBe("- a\n- b");
  });
});

describe("Escape and then Tab", () => {
  it("leaves a list item to the browser, so the focus can leave the editor", () => {
    const view = markView("- a\n- b");
    setCaret(view, 7);
    view.focus();
    const down = (key: string, keyCode: number) => {
      const event = new KeyboardEvent("keydown", {
        key,
        keyCode,
        bubbles: true,
        cancelable: true,
      });
      view.contentDOM.dispatchEvent(event);
      return event.defaultPrevented;
    };
    expect(down("Escape", 27)).toBe(false);
    expect(down("Tab", 9)).toBe(false);
    expect(bytes(view.state)).toBe("- a\n- b");
  });

  it("nests the item when no Escape came before", () => {
    const view = markView("- a\n- b");
    setCaret(view, 7);
    view.focus();
    const event = new KeyboardEvent("keydown", {
      key: "Tab",
      keyCode: 9,
      bubbles: true,
      cancelable: true,
    });
    view.contentDOM.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(bytes(view.state)).toBe("- a\n  - b");
  });
});
