/**
 * The floating action bar: its text group on a selection, its text and table
 * groups in an open cell, and its keyboard (Alt+F10 in, arrows across,
 * Escape back with the selection as it was).
 */

import { undo } from "@codemirror/commands";
import type { Extension } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { Tooltips } from "../components/primitives";
import { bytes, cellOf, cleanupCells, soloMain } from "../test/cellKit";
import FloatingBar from "./FloatingBar";
import { fencePreviews } from "./fencePreviews";
import type { FloatingContext } from "./floatingContext";
import { floatingContextListener } from "./floatingContext";
import { activeCell } from "./grid/cellEditor";
import { tableAddRowBelow } from "./tableVerbs";
import { hostOpenCell } from "./grid/gridWidget";
import { gridExtension } from "./grid/index";
import { openCellRange, setOpenCell } from "./grid/openCell";

const DOC =
  "Intro words\n\n| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |\n\nOutro\n";

let errors: MockInstance<typeof console.error>;

beforeEach(() => {
  errors = vi.spyOn(console, "error");
});

afterEach(() => {
  cleanupCells();
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

interface Harness {
  main: EditorView;
  context: () => FloatingContext;
  /** Renders the bar for the current context. */
  show: (wikilinks?: boolean) => void;
}

function harness(extra: Extension[] = []): Harness {
  let latest: FloatingContext = null;
  let rerender: ((ui: React.ReactElement) => void) | null = null;
  let wl = true;
  const ui = () => <FloatingBar main={main} context={latest} wikilinks={wl} />;
  const main = soloMain(DOC, [
    gridExtension(),
    fencePreviews(false),
    floatingContextListener((next) => {
      latest = next;
      if (rerender) {
        const draw = rerender;
        act(() => {
          draw(ui());
        });
      }
    }),
    ...extra,
  ]);
  return {
    main,
    context: () => latest,
    show: (wikilinks = true) => {
      wl = wikilinks;
      const result = render(ui(), { wrapper: Tooltips });
      rerender = result.rerender;
    },
  };
}

function bar(): HTMLElement {
  return screen.getByRole("toolbar", { name: "Quick format" });
}

function quick(name: string): HTMLElement {
  return within(bar()).getByRole("button", { name });
}

function openCell(main: EditorView, row: number, column: number): EditorView {
  const pos = cellOf(main, row, column).content.to;
  main.dispatch({
    effects: setOpenCell.of({ anchor: pos, head: pos, focused: true }),
  });
  const editor = hostOpenCell(main, "now");
  if (!editor) throw new Error("not hosted");
  return editor.view;
}

function altF10(target: Element): void {
  fireEvent.keyDown(target, { key: "F10", altKey: true, keyCode: 121 });
}

describe("the floating bar on a text selection", () => {
  test("Bold wraps the selection in ** and keeps it selected", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    fireEvent.click(quick("Bold"));
    expect(bytes(h.main.state).startsWith("**Intro** words")).toBe(true);
    const sel = h.main.state.selection.main;
    expect(h.main.state.sliceDoc(sel.from, sel.to)).toBe("Intro");
  });

  test("offers only the text group, under the sticky bar's names", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    for (const name of [
      "Bold",
      "Italic",
      "Strikethrough",
      "Inline code",
      "Link",
      "Wiki link",
    ]) {
      expect(quick(name)).toBeInTheDocument();
    }
    expect(
      within(bar()).queryByRole("button", { name: "Insert row below" }),
    ).toBeNull();
  });

  test("a document without wiki links gets no Wiki link button", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show(false);
    expect(
      within(bar()).queryByRole("button", { name: "Wiki link" }),
    ).toBeNull();
  });

  test("the bar cancels its own mousedown", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    const event = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
    });
    quick("Italic").dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  });

  test("the bar hides while the pointer is down in the editor", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    fireEvent.mouseDown(h.main.dom);
    expect(screen.queryByRole("toolbar", { name: "Quick format" })).toBeNull();
    fireEvent.mouseUp(document);
    expect(bar()).toBeInTheDocument();
  });

  test("Escape in the text hides the bar until a new selection", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    fireEvent.keyDown(h.main.contentDOM, { key: "Escape" });
    expect(screen.queryByRole("toolbar", { name: "Quick format" })).toBeNull();
    // A press and release in the editor picks a selection again (jsdom draws
    // every selection at the same empty rect, so the press is what tells).
    // On the editor's frame: CodeMirror's own press handling, which would
    // move the selection, is not what is tested here.
    fireEvent.mouseDown(h.main.dom);
    fireEvent.mouseUp(document);
    expect(bar()).toBeInTheDocument();
  });

  test("the bar hides when the focus leaves the editor", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    act(() => {
      outside.focus();
    });
    expect(screen.queryByRole("toolbar", { name: "Quick format" })).toBeNull();
    act(() => {
      h.main.focus();
    });
    expect(bar()).toBeInTheDocument();
    outside.remove();
  });

  test("Alt+F10 focuses the first button, arrows move, Escape goes back", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    const before = h.main.state.selection;
    // The bar never takes focus on its own.
    expect(bar().contains(document.activeElement)).toBe(false);
    altF10(h.main.contentDOM);
    await vi.waitFor(() => {
      expect(quick("Bold")).toHaveFocus();
    });
    // One tab stop: the roving tabindex.
    expect(quick("Bold")).toHaveAttribute("tabindex", "0");
    expect(quick("Italic")).toHaveAttribute("tabindex", "-1");
    fireEvent.keyDown(quick("Bold"), { key: "ArrowRight" });
    expect(quick("Italic")).toHaveFocus();
    fireEvent.keyDown(quick("Italic"), { key: "ArrowLeft" });
    expect(quick("Bold")).toHaveFocus();
    fireEvent.keyDown(quick("Bold"), { key: "End" });
    expect(quick("Wiki link")).toHaveFocus();
    fireEvent.keyDown(quick("Wiki link"), { key: "Home" });
    expect(quick("Bold")).toHaveFocus();
    fireEvent.keyDown(quick("Bold"), { key: "Escape" });
    expect(h.main.hasFocus).toBe(true);
    expect(h.main.state.selection.eq(before)).toBe(true);
  });
});

describe("the floating bar in an open cell", () => {
  test("Bold wraps the cell selection inside the cell only", async () => {
    const h = harness();
    const cell = openCell(h.main, 2, 1);
    const { from, to } = cellOf(h.main, 2, 1).content;
    cell.dispatch({ selection: { anchor: from, head: to } });
    await vi.waitFor(() => {
      expect(h.context()).toMatchObject({ kind: "cell", selection: true });
    });
    h.show();
    fireEvent.click(quick("Bold"));
    expect(bytes(h.main.state)).toBe(DOC.replace("| 3 | 4 |", "| 3 | **4** |"));
    expect(activeCell(h.main)?.view.hasFocus).toBe(true);
  });

  test("Insert row below adds the row below the open cell's row", async () => {
    const h = harness();
    openCell(h.main, 1, 0);
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("cell");
    });
    h.show();
    fireEvent.click(quick("Insert row below"));
    const plain = soloMain(DOC);
    expect(tableAddRowBelow(plain, cellOf(plain, 1, 0).content.from)).toBe(
      true,
    );
    expect(bytes(h.main.state)).toBe(bytes(plain.state));
    // The new row's cell is open, and one undo takes the row away again.
    expect(openCellRange(h.main.state)).toMatchObject({ row: 2, column: 0 });
    undo(h.main);
    expect(bytes(h.main.state)).toBe(DOC);
  });

  test("the cell group adds the table verbs and More table actions", async () => {
    const h = harness();
    openCell(h.main, 1, 0);
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("cell");
    });
    h.show();
    for (const name of [
      "Bold",
      "Insert row below",
      "Insert column right",
      "Delete row",
      "Delete column",
      "Align",
      "More table actions",
    ]) {
      expect(quick(name)).toBeInTheDocument();
    }
    fireEvent.click(quick("More table actions"));
    expect(
      document.querySelector('[role="menu"][aria-label="Cell actions"]'),
    ).not.toBeNull();
  });

  test("Delete row is off on the header row", async () => {
    const h = harness();
    openCell(h.main, 0, 0);
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("cell");
    });
    h.show();
    expect(quick("Delete row")).toBeDisabled();
  });

  test("Alt+F10 from the cell and Escape back keep the cell's selection", async () => {
    const h = harness();
    const cell = openCell(h.main, 1, 1);
    const { from, to } = cellOf(h.main, 1, 1).content;
    cell.dispatch({ selection: { anchor: from, head: to } });
    await vi.waitFor(() => {
      expect(h.context()).toMatchObject({ kind: "cell", selection: true });
    });
    h.show();
    altF10(cell.contentDOM);
    await vi.waitFor(() => {
      expect(quick("Bold")).toHaveFocus();
    });
    fireEvent.keyDown(quick("Bold"), { key: "ArrowRight" });
    expect(quick("Italic")).toHaveFocus();
    fireEvent.keyDown(quick("Italic"), { key: "Escape" });
    const view = activeCell(h.main)?.view;
    expect(view?.hasFocus).toBe(true);
    expect(view?.state.selection.main).toMatchObject({
      anchor: from,
      head: to,
    });
  });
});
