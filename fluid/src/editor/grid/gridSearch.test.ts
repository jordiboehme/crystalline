/**
 * Search and replace in the grid: a hit inside a cell opens that cell with
 * the hit selected (on a read-only page it only marks the cell), a hit
 * outside every table closes the open cell, and the search keys work from
 * inside a cell.
 */

import {
  SearchQuery,
  closeSearchPanel,
  findNext,
  getSearchQuery,
  openSearchPanel,
  replaceAll,
  replaceNext,
  searchPanelOpen,
  setSearchQuery,
} from "@codemirror/search";
import type { Extension } from "@codemirror/state";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import * as Y from "yjs";

import {
  bytes,
  cleanupCells,
  openText,
  press,
  roomMain,
  soloMain,
  typeIn,
} from "../../test/cellKit";
import { fencePreviews } from "../fencePreviews";
import { activeCell } from "./cellEditor";
import { GridWidget, gridField } from "./gridWidget";
import { gridExtension } from "./index";
import { openCellField, openCellRange } from "./openCell";
import { gridTables } from "./structure";

const DOC = "| a | b |\n| - | - |\n| 12 | 2 |\n\nafter 2\n";

let errors: MockInstance<typeof console.error>;

beforeEach(() => {
  errors = vi.spyOn(console, "error");
});

afterEach(() => {
  cleanupCells();
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

function layers(): Extension[] {
  return [gridExtension(), fencePreviews(false)];
}

function cellEl(main: EditorView, row: number, column: number): HTMLElement {
  const el = main.dom.querySelector<HTMLElement>(
    `table[role="grid"] [aria-rowindex="${String(row + 1)}"][aria-colindex="${String(column + 1)}"]`,
  );
  if (!el) throw new Error(`no cell ${String(row)}/${String(column)}`);
  return el;
}

function mouseDown(el: HTMLElement): void {
  el.dispatchEvent(
    new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }),
  );
}

function hosted(main: EditorView) {
  const cell = activeCell(main);
  if (!cell) throw new Error("no cell editor mounted");
  return cell;
}

/** The open cell's row and column. */
function openPlace(main: EditorView): [number, number] | null {
  const cell = openCellRange(main.state);
  return cell ? [cell.row, cell.column] : null;
}

/** The text the cell view has selected. */
function selected(main: EditorView): string {
  const { view } = hosted(main);
  const { from, to } = view.state.selection.main;
  return view.state.doc.sliceString(from, to);
}

function query(main: EditorView, search: string, replace = ""): void {
  main.dispatch({
    effects: setSearchQuery.of(new SearchQuery({ search, replace })),
  });
}

function searchField(main: EditorView): HTMLInputElement {
  const input = main.dom.querySelector<HTMLInputElement>("[main-field]");
  if (!input) throw new Error("no search panel");
  return input;
}

/**
 * Opens the search panel with the focus in its search field. A browser's
 * `select()` focuses the field when the panel mounts; jsdom's does not.
 */
function openPanel(main: EditorView): HTMLInputElement {
  openSearchPanel(main);
  const input = searchField(main);
  input.focus();
  return input;
}

/** Enter in the panel's search field, the way a user asks for the next hit. */
function enterInPanel(main: EditorView): void {
  searchField(main).dispatchEvent(
    new KeyboardEvent("keydown", {
      key: "Enter",
      keyCode: 13,
      bubbles: true,
      cancelable: true,
    }),
  );
}

function shape(state: EditorState): number[] {
  return gridTables(state).flatMap((table) =>
    table.rows.map((row) => row.length),
  );
}

const flush = () => new Promise<void>((resolve) => queueMicrotask(resolve));

describe("a hit inside a cell", () => {
  test("findNext for 2 opens the 12 cell with the hit selected, focus stays in the panel", () => {
    const main = soloMain(DOC, layers());
    query(main, "2");
    const input = openPanel(main);
    expect(document.activeElement).toBe(input);
    findNext(main);
    expect(openPlace(main)).toEqual([1, 0]);
    expect(selected(main)).toBe("2");
    expect(document.activeElement).toBe(input);
    expect(main.state.field(openCellField)?.focused).toBe(false);
  });

  test("with focus in the search panel, the next hit moves the open cell", () => {
    const main = soloMain(DOC, layers());
    query(main, "2");
    openPanel(main);
    enterInPanel(main);
    expect(openPlace(main)).toEqual([1, 0]);
    enterInPanel(main);
    expect(openPlace(main)).toEqual([1, 1]);
    expect(selected(main)).toBe("2");
    expect(cellEl(main, 1, 0).textContent).toBe("12");
    expect(document.activeElement).toBe(searchField(main));
  });

  test("findNext from a focused main view focuses the cell", () => {
    const main = soloMain(DOC, layers());
    query(main, "2");
    main.focus();
    findNext(main);
    const cell = hosted(main);
    expect(cell.view.dom.contains(document.activeElement)).toBe(true);
    expect(selected(main)).toBe("2");
  });

  test("a hit in a table outside the viewport opens the cell as it is drawn", () => {
    const text = "line\n\n".repeat(3000) + "| a | b |\n| - | - |\n| 12 | 2 |\n";
    const main = soloMain(text, layers());
    expect(main.dom.querySelector(".cm-grid")).toBeNull();
    query(main, "2");
    openPanel(main);
    findNext(main);
    expect(openPlace(main)).toEqual([1, 0]);
    const root = main.dom.querySelector(".cm-grid");
    expect(root?.contains(hosted(main).view.dom)).toBe(true);
    expect(selected(main)).toBe("2");
  });

  test("a hit's cell torn down with its table comes back with the hit, unfocused", async () => {
    const main = soloMain(DOC, layers());
    query(main, "2");
    const input = openPanel(main);
    findNext(main);
    let widget: GridWidget | null = null;
    main.state
      .field(gridField)
      .between(0, main.state.doc.length, (_f, _t, deco) => {
        const w = (deco.spec as { widget?: unknown }).widget;
        if (!widget && w instanceof GridWidget) widget = w;
      });
    if (!widget) throw new Error("no grid widget");
    const found: GridWidget = widget;
    const dom = main.dom.querySelector<HTMLElement>(".cm-grid");
    if (!dom) throw new Error("no grid root");
    // Out of the viewport and back: the widget's DOM is destroyed, and the
    // table is drawn anew from the open-cell intent.
    found.destroy(dom);
    expect(activeCell(main)).toBeNull();
    const again = found.toDOM(main);
    document.body.appendChild(again);
    await flush();
    expect(again.contains(hosted(main).view.dom)).toBe(true);
    expect(selected(main)).toBe("2");
    expect(document.activeElement).toBe(input);
    again.remove();
  });

  test("replaceNext puts two in place of 2 inside the cell and nothing else", () => {
    const text = "| a | b |\n| - | - |\n| 1 | 2 |\n";
    const main = soloMain(text, layers());
    const before = shape(main.state);
    query(main, "2", "two");
    openSearchPanel(main);
    findNext(main);
    expect(openPlace(main)).toEqual([1, 1]);
    replaceNext(main);
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1 | two |\n");
    expect(shape(main.state)).toEqual(before);
    expect(openText(main)).toBe("two");
    const { view } = hosted(main);
    expect(view.state.doc.eq(main.state.doc)).toBe(true);
  });

  test("replaceAll across two tables changes only the matched bytes", () => {
    const text =
      "| a | b |\n| - | - |\n| 1 | 2 |\n\nmid 2\n\n| c |\n| - |\n| 22 |\n";
    const main = soloMain(text, layers());
    mouseDown(cellEl(main, 1, 0));
    query(main, "2", "two");
    replaceAll(main);
    expect(bytes(main.state)).toBe(
      "| a | b |\n| - | - |\n| 1 | two |\n\nmid two\n\n| c |\n| - |\n| twotwo |\n",
    );
    expect(openText(main)).toBe("1");
    expect(hosted(main).view.state.doc.eq(main.state.doc)).toBe(true);
  });
});

describe("a hit outside every table", () => {
  test("closes the open cell, with focus left in the panel", () => {
    const main = soloMain(DOC, layers());
    query(main, "after");
    openSearchPanel(main);
    mouseDown(cellEl(main, 1, 1));
    const cell = hosted(main);
    const input = searchField(main);
    input.focus();
    findNext(main);
    expect(main.state.field(openCellField)).toBeNull();
    expect(activeCell(main)).toBeNull();
    expect(cell.view.dom.isConnected).toBe(false);
    expect(document.activeElement).toBe(input);
    const sel = main.state.selection.main;
    expect(main.state.doc.sliceString(sel.from, sel.to)).toBe("after");
  });

  test("an undo from the cell that puts main's selection back outside keeps it open", () => {
    const { main } = roomMain(DOC, layers());
    main.dispatch({ selection: { anchor: main.state.doc.length } });
    mouseDown(cellEl(main, 1, 1));
    const cell = hosted(main);
    typeIn(cell, "x");
    expect(press(cell.view, "z", { mod: true })).toBe(true);
    expect(activeCell(main)).toBe(cell);
    expect(openText(main)).toBe("2");
  });
});

describe("search keys inside a cell", () => {
  test("Mod-f opens the main view's search panel with the cell's selection", () => {
    const main = soloMain(DOC, layers());
    mouseDown(cellEl(main, 1, 0));
    const cell = hosted(main);
    const content = openCellRange(main.state)?.content;
    if (!content) throw new Error("no open cell");
    cell.view.dispatch({
      selection: { anchor: content.from, head: content.to },
    });
    expect(press(cell.view, "f", { mod: true })).toBe(true);
    expect(searchPanelOpen(main.state)).toBe(true);
    expect(getSearchQuery(main.state).search).toBe("12");
    // A browser moves the focus to the panel's field here; jsdom's
    // `select()` does not, so that part is a browser check.
  });

  test("F3 finds from the cell's caret and keeps the focus in the grid", () => {
    const main = soloMain(DOC, layers());
    query(main, "2");
    mouseDown(cellEl(main, 1, 0));
    const cell = hosted(main);
    const content = openCellRange(main.state)?.content;
    if (!content) throw new Error("no open cell");
    // The caret at the end of "12": the next 2 is the one in the next cell.
    cell.view.dispatch({ selection: { anchor: content.to } });
    expect(press(cell.view, "F3")).toBe(true);
    expect(openPlace(main)).toEqual([1, 1]);
    expect(selected(main)).toBe("2");
    expect(hosted(main).view.dom.contains(document.activeElement)).toBe(true);
  });

  test("Mod-g also moves to the next hit", () => {
    const main = soloMain(DOC, layers());
    query(main, "2");
    mouseDown(cellEl(main, 1, 0));
    const cell = hosted(main);
    const content = openCellRange(main.state)?.content;
    if (!content) throw new Error("no open cell");
    cell.view.dispatch({ selection: { anchor: content.from } });
    expect(press(cell.view, "g", { mod: true })).toBe(true);
    expect(openPlace(main)).toEqual([1, 0]);
    expect(selected(main)).toBe("2");
  });

  test("closing the panel hands the focus back to the open cell", async () => {
    const main = soloMain(DOC, layers());
    query(main, "2");
    openPanel(main);
    findNext(main);
    const cell = hosted(main);
    expect(cell.view.dom.contains(document.activeElement)).toBe(false);
    closeSearchPanel(main);
    await vi.waitFor(() => {
      expect(cell.view.dom.contains(document.activeElement)).toBe(true);
    });
    expect(activeCell(main)).toBe(cell);
  });
});

describe("main's caret never left in a table", () => {
  test("main focused on an old hit with no cell open opens that cell, not a frozen caret", async () => {
    const { main, awareness, remoteAwareness, local } = roomMain(DOC, layers());
    query(main, "2");
    const input = openPanel(main);
    findNext(main);
    const cell = hosted(main);
    // The user goes into the cell, steps out to grid navigation...
    cell.view.focus();
    press(cell.view, "Escape");
    expect(activeCell(main)).toBeNull();
    // ...then back to the panel, and closes it: main takes the focus.
    input.focus();
    closeSearchPanel(main);
    await vi.waitFor(() => {
      expect(activeCell(main)?.view.hasFocus).toBe(true);
    });
    await flush();
    const table = gridTables(main.state)[0];
    if (!table) throw new Error("no table");
    const state = remoteAwareness.getStates().get(awareness.clientID) as
      { cursor?: { anchor: unknown; head: unknown } | null } | undefined;
    const cursor = state?.cursor ?? null;
    if (cursor !== null) {
      const at = [cursor.anchor, cursor.head].map(
        (rel) =>
          Y.createAbsolutePositionFromRelativePosition(
            Y.createRelativePositionFromJSON(rel),
            local,
          )?.index ?? -1,
      );
      const caret = hosted(main).view.state.selection.main;
      const outside = at.every((i) => i < table.from || i > table.to);
      expect(outside || (at[0] === caret.anchor && at[1] === caret.head)).toBe(
        true,
      );
    }
    expect(selected(main)).toBe("2");
  });

  test("F3 from a focused cell to a hit outside every table gives main the focus", async () => {
    const main = soloMain(DOC, layers());
    query(main, "after");
    mouseDown(cellEl(main, 1, 1));
    const cell = hosted(main);
    expect(press(cell.view, "F3")).toBe(true);
    expect(activeCell(main)).toBeNull();
    await vi.waitFor(() => {
      expect(main.hasFocus).toBe(true);
    });
    const sel = main.state.selection.main;
    expect(main.state.doc.sliceString(sel.from, sel.to)).toBe("after");
  });
});

describe("read-only (R17)", () => {
  const READ_ONLY: Extension = [
    EditorView.editable.of(false),
    EditorState.readOnly.of(true),
  ];

  test("a hit marks its cell and opens no editor", () => {
    const main = soloMain(DOC, [...layers(), READ_ONLY]);
    const text = bytes(main.state);
    query(main, "2");
    openSearchPanel(main);
    findNext(main);
    expect(activeCell(main)).toBeNull();
    expect(main.state.field(openCellField)).toBeNull();
    expect(cellEl(main, 1, 0).classList.contains("cm-grid-hit")).toBe(true);
    findNext(main);
    expect(cellEl(main, 1, 0).classList.contains("cm-grid-hit")).toBe(false);
    expect(cellEl(main, 1, 1).classList.contains("cm-grid-hit")).toBe(true);
    findNext(main);
    expect(main.dom.querySelector(".cm-grid-hit")).toBeNull();
    expect(bytes(main.state)).toBe(text);
  });

  test("main taking the focus on a hit moves its caret out of the table", async () => {
    const main = soloMain(DOC, [...layers(), READ_ONLY]);
    query(main, "2");
    openPanel(main);
    findNext(main);
    main.focus();
    const table = gridTables(main.state)[0];
    if (!table) throw new Error("no table");
    await vi.waitFor(() => {
      expect(main.state.selection.main.head).toBe(DOC.indexOf("\nafter"));
    });
    expect(main.state.selection.main.head > table.to).toBe(true);
    expect(activeCell(main)).toBeNull();
  });

  test("the mark survives the table being drawn again", async () => {
    const main = soloMain(DOC, [...layers(), READ_ONLY]);
    query(main, "2");
    openSearchPanel(main);
    findNext(main);
    // A new row (a peer's, say) changes the shape: the grid is drawn anew.
    const end = DOC.indexOf("\n\nafter") + 1;
    main.dispatch({ changes: { from: end, insert: "| 3 | 4 |\n" } });
    await flush();
    expect(gridTables(main.state)[0]?.rows).toHaveLength(3);
    expect(cellEl(main, 1, 0).classList.contains("cm-grid-hit")).toBe(true);
  });
});
