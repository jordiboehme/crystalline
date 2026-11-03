/**
 * Footnotes drawn as numbers: a reference as a small button, a definition's
 * `[^label]:` as its number. The numbers are the reading page's (the order of
 * first reference), the bytes never change, and the jump is a caret move.
 */

import { undoDepth } from "@codemirror/commands";
import { EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  undoOnce,
} from "../test/markKit";
import { footnoteScans } from "./footnotes";
import { markSegment } from "./marks/caret";

afterEach(cleanupMarkViews);

const DOC = "a[^n] b[^m]\n\n[^m]: M\n[^n]: N";

const MODES = [{ mode: "hidden" as const }, { mode: "show" as const }];

function refs(view: EditorView): string[] {
  return [...view.dom.querySelectorAll(".cm-footnote-ref")].map(
    (el) => el.textContent ?? "",
  );
}

function labels(view: EditorView): string[] {
  return [...view.dom.querySelectorAll(".cm-footnote-label")].map(
    (el) => el.textContent ?? "",
  );
}

function button(view: EditorView, index = 0): HTMLElement {
  const found = view.dom.querySelectorAll<HTMLElement>(".cm-footnote-button");
  const el = found[index];
  if (!el) throw new Error(`no footnote reference ${index}`);
  return el;
}

describe("the numbers", () => {
  it("draws the references and the definition labels as numbers", () => {
    const view = markView(DOC);
    setCaret(view, DOC.length);
    expect(refs(view)).toEqual(["1", "2"]);
    expect(labels(view)).toEqual(["2", "1"]);
    expect(view.dom.textContent).not.toContain("[^");
    expect(bytes(view.state)).toBe(DOC);
  });

  it("names the button and the label for a reader", () => {
    const view = markView(DOC);
    setCaret(view, DOC.length);
    expect(button(view, 0).getAttribute("aria-label")).toBe("Footnote 1");
    expect(
      [...view.dom.querySelectorAll(".cm-footnote-label")].map((el) =>
        el.getAttribute("aria-label"),
      ),
    ).toEqual(["Footnote 2 text", "Footnote 1 text"]);
  });

  it("gives a reference used twice the same number", () => {
    const view = markView("a[^x] b[^y] c[^x]\n\n[^y]: Y\n[^x]: X");
    setCaret(view, 25);
    expect(refs(view)).toEqual(["1", "2", "1"]);
    expect(labels(view)).toEqual(["2", "1"]);
  });

  it("draws a definition nobody refers to as its text, as the page omits it", () => {
    const text = "plain\n\n[^z]: Z";
    const view = markView(text);
    setCaret(view, 0);
    expect(labels(view)).toEqual([]);
    expect(view.dom.textContent).toContain("[^z]: Z");
  });

  it("draws a reference with no definition as its text", () => {
    const text = "a[^q] b";
    const view = markView(text);
    setCaret(view, text.length);
    expect(refs(view)).toEqual([]);
    expect(view.dom.textContent).toContain("a[^q] b");
  });

  it("treats labels that differ only in case as one footnote", () => {
    const view = markView("a[^A] b[^a]\n\n[^a]: x");
    setCaret(view, 19);
    expect(refs(view)).toEqual(["1", "1"]);
    expect(labels(view)).toEqual(["1"]);
  });

  it("draws nothing for a reference in code", () => {
    const text = "x[^a] `[^a]`\n\n```\n[^b]\n```\n\n[^a]: A\n[^b]: B";
    const view = markView(text);
    setCaret(view, text.length);
    expect(refs(view)).toEqual(["1"]);
    expect(labels(view)).toEqual(["1"]);
    expect(view.dom.textContent).toContain("[^b]: B");
    expect(bytes(view.state)).toBe(text);
  });

  it("numbers a reference inside a note after the body's, as the page does", () => {
    const view = markView(
      "a[^1] b[^2]\n\n[^1]: one[^3]\n[^2]: two\n[^3]: three",
    );
    setCaret(view, 0);
    expect(refs(view)).toEqual(["1", "2", "3"]);
    expect(labels(view)).toEqual(["1", "2", "3"]);
  });

  it("renumbers when a reference is added before the others", () => {
    const view = markView(DOC);
    setCaret(view, 0);
    view.dispatch({
      changes: { from: 0, insert: "[^m]" },
      userEvent: "input.type",
    });
    expect(bytes(view.state)).toBe(`[^m]${DOC}`);
    expect(refs(view)).toEqual(["1", "2", "1"]);
    expect(labels(view)).toEqual(["1", "2"]);
  });

  it("draws in a Reflection too", () => {
    const view = markView(DOC, { adapter: "reflection" });
    setCaret(view, DOC.length);
    expect(refs(view)).toEqual(["1", "2"]);
  });
});

describe.each(MODES)("modes: $mode", ({ mode }) => {
  it("shows the source on the line the caret touches only in show mode", () => {
    const view = markView(DOC, { mode });
    setCaret(view, 2);
    if (mode === "show") {
      expect(refs(view)).toEqual([]);
      expect(view.dom.textContent).toContain("a[^n] b[^m]");
      expect(labels(view)).toEqual(["2", "1"]);
    } else {
      expect(refs(view)).toEqual(["1", "2"]);
    }
    expect(bytes(view.state)).toBe(DOC);
  });
});

describe("the jump", () => {
  it("puts the caret at the start of the definition's text and writes nothing", () => {
    const view = markView(DOC);
    setCaret(view, DOC.length);
    button(view, 0).click();
    expect(caret(view)).toBe(DOC.indexOf("N", DOC.indexOf("[^n]:")));
    expect(bytes(view.state)).toBe(DOC);
    expect(undoDepth(view.state)).toBe(0);
  });

  it("answers Enter on the button the same way", () => {
    const view = markView(DOC);
    setCaret(view, DOC.length);
    button(view, 1).dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    );
    expect(caret(view)).toBe(DOC.indexOf("M", DOC.indexOf("[^m]:")));
    expect(bytes(view.state)).toBe(DOC);
  });

  it("scrolls the definition into view", () => {
    const view = markView(DOC);
    setCaret(view, DOC.length);
    const spy = vi.spyOn(view, "dispatch");
    button(view, 0).click();
    const spec = spy.mock.calls[0]?.[0] as { scrollIntoView?: boolean };
    expect(spec.scrollIntoView).toBe(true);
  });

  it("works on a read-only view", () => {
    const view = markView(DOC);
    setCaret(view, DOC.length);
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    button(view, 0).click();
    expect(caret(view)).toBe(DOC.indexOf("N", DOC.indexOf("[^n]:")));
    expect(bytes(view.state)).toBe(DOC);
  });

  it("does nothing in a table cell, whose definition is outside the cell", () => {
    const text = "| a[^n] |\n| --- |\n\n[^n]: N";
    const segment = { from: 2, to: 8 };
    const view = markView(text);
    view.dispatch({
      effects: StateEffect.appendConfig.of(
        markSegment.of((_state, pos) =>
          pos >= segment.from && pos <= segment.to ? segment : null,
        ),
      ),
    });
    setCaret(view, 4);
    expect(refs(view)).toEqual(["1"]);
    button(view).click();
    button(view).dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
    );
    expect(caret(view)).toBe(4);
    expect(bytes(view.state)).toBe(text);
  });
});

describe("editing next to a drawn reference", () => {
  it("takes the whole reference with Backspace, as one undo step", () => {
    const text = "a[^n] b\n\n[^n]: N";
    const view = markView(text);
    setCaret(view, 5);
    expect(caret(view)).toBe(5);
    keys(view, "Backspace");
    expect(bytes(view.state)).toBe("a b\n\n[^n]: N");
    expect(caret(view)).toBe(1);
    undoOnce(view);
    expect(bytes(view.state)).toBe(text);
  });

  it("takes the whole reference with Delete from before it", () => {
    const text = "a[^n] b\n\n[^n]: N";
    const view = markView(text);
    setCaret(view, 1);
    keys(view, "Delete");
    expect(bytes(view.state)).toBe("a b\n\n[^n]: N");
  });

  it("steps over the reference with the arrow keys", () => {
    const text = "a[^n] b\n\n[^n]: N";
    const view = markView(text);
    setCaret(view, 1);
    keys(view, "ArrowRight");
    expect(caret(view)).toBe(5);
    keys(view, "ArrowLeft");
    expect(caret(view)).toBe(1);
  });

  it("keeps the definition's text editable", () => {
    const text = "a[^n] b\n\n[^n]: N";
    const view = markView(text);
    setCaret(view, text.length);
    view.dispatch({
      changes: { from: text.length, insert: "!" },
      userEvent: "input.type",
    });
    expect(bytes(view.state)).toBe(`${text}!`);
    expect(labels(view)).toEqual(["1"]);
  });
});

describe("copying", () => {
  it("gives the source text over drawn references", () => {
    const view = markView(DOC);
    view.focus();
    view.dispatch({
      selection: EditorSelection.range(0, DOC.length),
      userEvent: "select",
    });
    const data = new Map<string, string>();
    const event = new Event("copy", { bubbles: true, cancelable: true });
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
    expect(data.get("text/plain")).toBe(DOC);
  });
});

describe("the cost", () => {
  const long = (() => {
    const lines: string[] = [];
    for (let n = 0; n < 1800; n += 1) {
      lines.push(
        n < 200 ? `Line ${n} has a note.[^${n}]` : `Plain line ${n} of prose.`,
      );
    }
    lines.push("");
    for (let n = 0; n < 200; n += 1) {
      lines.push(`[^${n}]: Note ${n}.`);
    }
    return lines.join("\n");
  })();

  it("numbers 200 footnotes in a long document within a generous bound", () => {
    const started = performance.now();
    const view = markView(long);
    const elapsed = performance.now() - started;
    expect(long.split("\n").length).toBeGreaterThanOrEqual(2000);
    expect(elapsed).toBeLessThan(5000);
    expect(bytes(view.state)).toBe(long);
    expect(refs(view).length).toBeGreaterThan(0);
    expect(footnoteScans()).toBeGreaterThan(0);
  });

  it("does not scan again for a keystroke that cannot change a note", () => {
    const view = markView(long);
    const at = long.indexOf("Plain line 1000") + 5;
    setCaret(view, at);
    const before = footnoteScans();
    for (const char of "abc") {
      view.dispatch({
        changes: { from: caret(view), insert: char },
        selection: EditorSelection.cursor(caret(view) + 1),
        userEvent: "input.type",
      });
    }
    expect(footnoteScans()).toBe(before);
    expect(view.state.doc.sliceString(at, at + 3)).toBe("abc");
  });

  it("scans again when a keystroke touches a reference", () => {
    const view = markView(long);
    const before = footnoteScans();
    view.dispatch({
      changes: { from: 0, insert: "[^1]" },
      userEvent: "input.type",
    });
    expect(footnoteScans()).toBeGreaterThan(before);
  });
});
