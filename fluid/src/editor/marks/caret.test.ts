/**
 * The caret over hidden marks: one visible character per arrow press, one
 * position per drawn caret place (`canonical`), and every empty caret a local
 * transaction sets resting there. Each case runs in hidden mode solo and in a
 * room, and the show-mode and Raw cases pin today's behaviour on the line the
 * caret touches. Moving the caret never changes a byte.
 */

import { EditorSelection, StateEffect } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  remoteInsert,
  setCaret,
  typeText,
} from "../../test/markKit";
import { repoMarkdown } from "../../test/repoMarkdown";
import { fromCell } from "../grid/cellEditor";
import { openCellField } from "../grid/openCell";
import { mainSelectionParked } from "../parkedSelection";
import { prefetchGrid } from "../grid/loader";
import { canonical, snapRange, stepVisible } from "./caret";
import { runsOnLine } from "./hiddenMarks";

afterEach(cleanupMarkViews);

beforeAll(async () => {
  await prefetchGrid();
});

const BOLD = "a **bold** c";

/** What typing `**word** next` writes: exactly what was typed. */
const TYPED_BY_HAND = "**word** next";

/** From, ArrowRight, ArrowLeft; null where the key is handed on. */
const TABLE: [number, number | null, number | null][] = [
  [0, 1, null],
  [1, 2, 0],
  [2, 5, 1],
  [5, 6, 2],
  [7, 8, 6],
  [8, 11, 7],
  [11, 12, 8],
  [12, null, 11],
];

describe.each([
  { name: "solo", room: false },
  { name: "in a room", room: true },
])("hidden mode, $name", ({ room }) => {
  const view = (text: string) => markView(text, { room });

  it.each(TABLE)("from %i, ArrowRight and ArrowLeft", (from, right, left) => {
    const v = view(BOLD);
    setCaret(v, from);
    expect(caret(v)).toBe(from);
    expect(stepVisible(v.state, from, true)).toBe(right);
    expect(stepVisible(v.state, from, false)).toBe(left);
    keys(v, "ArrowRight");
    expect(caret(v)).toBe(right ?? from);
    setCaret(v, from);
    keys(v, "ArrowLeft");
    expect(caret(v)).toBe(left ?? from);
    expect(bytes(v.state)).toBe(BOLD);
  });

  it("puts each hidden position at its one drawn place", () => {
    const v = view(BOLD);
    expect([3, 4].map((p) => canonical(v.state, p))).toEqual([2, 2]);
    expect([9, 10].map((p) => canonical(v.state, p))).toEqual([8, 8]);
    const item = view("  - item");
    expect([2, 3, 4].map((p) => canonical(item.state, p))).toEqual([4, 4, 4]);
    // The indentation is text a caret may sit in.
    expect([0, 1].map((p) => canonical(item.state, p))).toEqual([0, 1]);
  });

  it("Home on a heading lands after its prefix, End before a closing mark", () => {
    const heading = view("## Title");
    setCaret(heading, 8);
    keys(heading, "Home");
    expect(caret(heading)).toBe(3);
    const bold = view("x **b**");
    setCaret(bold, 0);
    keys(bold, "End");
    expect(caret(bold)).toBe(5);
  });

  it("Shift+ArrowRight extends the head one visible character at a time", () => {
    const v = view(BOLD);
    setCaret(v, 7);
    keys(v, "Shift-ArrowRight");
    expect(v.state.selection.main).toMatchObject({ anchor: 7, head: 8 });
    keys(v, "Shift-ArrowRight");
    expect(v.state.selection.main).toMatchObject({ anchor: 7, head: 11 });
    keys(v, "Shift-ArrowLeft", "Shift-ArrowLeft");
    expect(v.state.selection.main).toMatchObject({ anchor: 7, head: 7 });
  });

  it("Mod+ArrowRight runs the group motion and lands canonical", () => {
    const v = view("a **bold** c **d**");
    setCaret(v, 0);
    const seen: number[] = [];
    for (let i = 0; i < 6; i += 1) {
      keys(v, "Mod-ArrowRight");
      seen.push(caret(v));
      expect(canonical(v.state, caret(v))).toBe(caret(v));
    }
    expect(seen.at(-1)).toBe(16);
  });

  it("puts a pointer caret inside a run at its drawn place", () => {
    const v = view(BOLD);
    setCaret(v, 10);
    expect(caret(v)).toBe(8);
    setCaret(v, 3);
    expect(caret(v)).toBe(2);
  });

  it("leaves a non-empty selection as it was", () => {
    const v = view(BOLD);
    v.dispatch({
      selection: EditorSelection.range(4, 8),
      userEvent: "select.pointer",
    });
    expect(v.state.selection.main).toMatchObject({ from: 4, to: 8 });
    v.dispatch({
      selection: EditorSelection.range(3, 9),
      userEvent: "select.pointer",
    });
    expect(v.state.selection.main).toMatchObject({ from: 3, to: 9 });
    expect(snapRange(v.state, v.state.selection.main)).toMatchObject({
      from: 4,
      to: 8,
    });
    expect(snapRange(v.state, EditorSelection.range(9, 3))).toMatchObject({
      anchor: 8,
      head: 4,
    });
  });

  it("lands canonical on ArrowUp and ArrowDown", () => {
    const v = view("**ab** x\n**cd** y\n## e");
    for (const start of [1, 4, 7, 10, 12, 15]) {
      setCaret(v, start);
      keys(v, "ArrowDown");
      expect(canonical(v.state, caret(v))).toBe(caret(v));
      setCaret(v, start);
      keys(v, "ArrowUp");
      expect(canonical(v.state, caret(v))).toBe(caret(v));
    }
  });

  it("never moves a caret set while the view composes", () => {
    const v = view(BOLD);
    Object.defineProperty(v, "composing", { get: () => true });
    v.dispatch({ selection: EditorSelection.cursor(10) });
    expect(caret(v)).toBe(10);
  });

  it("goes over the line break at a line's visible edge", () => {
    const v = view("x **b**\ny");
    setCaret(v, 5);
    keys(v, "ArrowRight");
    expect(caret(v)).toBe(8);
    keys(v, "ArrowLeft");
    expect(caret(v)).toBe(5);
    const h = view("a\n## H");
    setCaret(h, 1);
    keys(h, "ArrowRight");
    expect(caret(h)).toBe(5);
    keys(h, "ArrowLeft");
    expect(caret(h)).toBe(1);
    expect(bytes(h.state)).toBe("a\n## H");
  });
});

it("leaves main's caret alone while it is parked behind an open cell", () => {
  const v = markView(BOLD);
  v.dispatch({
    effects: StateEffect.appendConfig.of(mainSelectionParked.of(true)),
  });
  v.dispatch({ selection: EditorSelection.cursor(10) });
  expect(caret(v)).toBe(10);
  // The shape a search from the cell seeds main with (`onMain`).
  v.dispatch({
    selection: EditorSelection.cursor(9),
    annotations: fromCell.of(true),
  });
  expect(caret(v)).toBe(9);
  setCaret(v, 3);
  expect(caret(v)).toBe(3);
});

it("crosses a chip whole, one press", () => {
  const v = markView("**a** [[T]] b");
  setCaret(v, 6);
  keys(v, "ArrowRight");
  expect(caret(v)).toBe(11);
  keys(v, "ArrowLeft");
  expect(caret(v)).toBe(6);
});

it("typing a closing pair by hand ends the bold there", () => {
  const v = markView("");
  typeText(v, "**word** next");
  // The caret after the typed closing `**` rests inside the bold (the left
  // side wins), but the closing mark just typed turns the bold off for the
  // next text (formatInput rule 3), so the space and `next` go after it.
  expect(bytes(v.state)).toBe(TYPED_BY_HAND);
});

it("leaves a mapped caret alone when a peer's change arrives", () => {
  const v = markView("b", { room: true });
  setCaret(v, 0);
  remoteInsert(v, 0, "## ");
  expect(bytes(v.state)).toBe("## b");
  // Canonical would be 3; no normalizing dispatch followed the peer's change.
  expect(caret(v)).toBe(0);
});

describe("show mode, on the line the caret touches", () => {
  it("keeps a pointer caret where it was put", () => {
    const v = markView(BOLD, { mode: "show" });
    setCaret(v, 10);
    expect(caret(v)).toBe(10);
    keys(v, "ArrowLeft");
    expect(caret(v)).toBe(9);
  });

  it("is today's arrow over the shown marks", () => {
    const v = markView(BOLD, { mode: "show" });
    setCaret(v, 2);
    keys(v, "ArrowRight");
    expect(caret(v)).toBe(3);
  });
});

it("is today's arrow in Raw", () => {
  const v = markView(BOLD, { raw: true });
  setCaret(v, 10);
  expect(caret(v)).toBe(10);
  keys(v, "ArrowLeft");
  expect(caret(v)).toBe(9);
});

describe.each(["hidden", "show"] as const)("a Reflection, %s mode", (mode) => {
  it("ArrowRight at a bold heading's visible end does not rest on the id line", () => {
    const text = "## **H**\n<!-- r:7f3a9c2e -->\n\nx";
    const v = markView(text, { adapter: "reflection", mode });
    setCaret(v, 8);
    expect(caret(v)).toBe(mode === "hidden" ? 6 : 8);
    keys(v, "ArrowRight");
    expect(v.state.doc.lineAt(caret(v)).number).toBe(3);
    expect(bytes(v.state)).toBe(text);
  });

  it("ArrowRight at a heading's end lands where the id guard puts it", () => {
    const text = "## H\n<!-- r:7f3a9c2e -->\n\nx";
    const v = markView(text, { adapter: "reflection", mode });
    setCaret(v, 4);
    keys(v, "ArrowRight");
    const line = v.state.doc.lineAt(caret(v));
    expect(line.number).toBe(3);
    expect(caret(v)).toBe(25);
    keys(v, "ArrowLeft");
    expect(caret(v)).toBe(4);
    expect(bytes(v.state)).toBe(text);
  });
});

describe("into and out of a drawn table", () => {
  const TABLE_DOC = "x **b**\n| **b** | c |\n| - | - |\n| 1 | 2 |\n\n## H";

  function opened(v: EditorView) {
    return v.state.field(openCellField, false) ?? null;
  }

  it("ArrowRight at the visible end of a bold line above opens the first cell", () => {
    const v = markView(TABLE_DOC);
    setCaret(v, 7);
    expect(caret(v)).toBe(5);
    expect(keys(v, "ArrowRight")).toBe(true);
    expect(opened(v)).not.toBeNull();
    expect(bytes(v.state)).toBe(TABLE_DOC);
  });

  it("ArrowLeft at the visible start of a heading below opens the last row", () => {
    const v = markView(TABLE_DOC.replace("\n\n## H", "\n## H"));
    const heading = v.state.doc.line(v.state.doc.lines);
    setCaret(v, heading.from);
    expect(caret(v)).toBe(heading.from + 3);
    expect(keys(v, "ArrowLeft")).toBe(true);
    expect(opened(v)).not.toBeNull();
  });
});

/**
 * The repository's own Markdown: arrows, Home, End and the vertical keys from
 * lines that hold hidden marks (a spread of them, so the suite stays quick),
 * and the bytes are the same at the end.
 */
describe("caret moves change no byte", () => {
  /** How many lines with marks each file is walked from. */
  const LINES = 8;

  it.each(repoMarkdown())("%s", (_name, text) => {
    const v = markView(text);
    const doc = v.state.doc;
    const marked: number[] = [];
    for (let n = 1; n <= doc.lines; n += 1) {
      if (runsOnLine(v.state, n).length > 0) {
        marked.push(n);
      }
    }
    const step = Math.max(1, Math.floor(marked.length / LINES));
    for (let i = 0; i < marked.length; i += step) {
      const line = doc.line(marked[i] ?? 1);
      setCaret(v, line.from + Math.floor(line.length / 2));
      keys(v, "ArrowRight", "End", "ArrowRight");
      setCaret(v, line.to);
      keys(v, "ArrowLeft", "Shift-ArrowLeft", "Home", "ArrowLeft", "ArrowDown");
    }
    expect(v.state.doc.eq(doc)).toBe(true);
    expect(bytes(v.state)).toBe(text);
  });
});
