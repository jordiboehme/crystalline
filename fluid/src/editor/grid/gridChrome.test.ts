/**
 * The grid's pointer and menu chrome: the hover "+" buttons on a border, the
 * "Cell actions" menu (right click, Shift+F10, the context-menu key, the
 * small button) and what each does to the document. jsdom has no layout, so
 * every cell's rectangle is stubbed to a 100 x 30 box on a regular grid.
 */

import { undo } from "@codemirror/commands";
import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import {
  bytes,
  cellOf,
  cleanupCells,
  roomMain,
  soloMain,
  typeIn,
} from "../../test/cellKit";
import type { PipelessFixture } from "../../test/pipelessTables";
import { PIPELESS_FIXTURES, withTable } from "../../test/pipelessTables";
import { fencePreviews } from "../fencePreviews";
import {
  tableAddColumnAfter,
  tableAddColumnBefore,
  tableAddRowAbove,
  tableAddRowBelow,
  tableAlignColumn,
  tableDeleteColumn,
  tableDeleteRow,
  tableMoveColumn,
  tableMoveRow,
} from "../tableVerbs";
import { activeCell, runMainUndo } from "./cellEditor";
import { hostOpenCell } from "./gridWidget";
import { gridExtension } from "./index";
import { openCellRange, setOpenCell } from "./openCell";

/** Header a/b, data 1/2 and 3/4. */
const DOC = "Intro\n\n| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |\n\nOutro\n";

let errors: MockInstance<typeof console.error>;

beforeEach(() => {
  errors = vi.spyOn(console, "error");
});

afterEach(() => {
  cleanupCells();
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

function layers(extra: Extension[] = []): Extension[] {
  return [gridExtension(), fencePreviews(false), ...extra];
}

function cellEl(main: EditorView, row: number, column: number): HTMLElement {
  const el = main.dom.querySelector<HTMLElement>(
    `[aria-rowindex="${String(row + 1)}"][aria-colindex="${String(column + 1)}"]`,
  );
  if (!el) throw new Error(`no cell ${String(row)}/${String(column)}`);
  return el;
}

/** Gives every cell of the grid a 100 x 30 box, and the root the origin. */
function layout(main: EditorView): void {
  const root = main.dom.querySelector<HTMLElement>(".cm-grid");
  if (!root) throw new Error("no grid");
  const box = (left: number, top: number, width: number, height: number) =>
    ({
      left,
      top,
      width,
      height,
      right: left + width,
      bottom: top + height,
      x: left,
      y: top,
      toJSON: () => ({}),
    }) as DOMRect;
  root.getBoundingClientRect = () => box(0, 0, 200, 90);
  for (const el of root.querySelectorAll<HTMLElement>("[aria-rowindex]")) {
    const row = Number(el.getAttribute("aria-rowindex")) - 1;
    const column = Number(el.getAttribute("aria-colindex")) - 1;
    el.getBoundingClientRect = () => box(column * 100, row * 30, 100, 30);
  }
}

function move(el: Element, x: number, y: number): void {
  el.dispatchEvent(
    new MouseEvent("mousemove", {
      bubbles: true,
      clientX: x,
      clientY: y,
    }),
  );
}

function button(name: string): HTMLButtonElement | null {
  return (
    [...document.querySelectorAll<HTMLButtonElement>("button")].find(
      (b) => b.getAttribute("aria-label") === name && !b.hidden,
    ) ?? null
  );
}

function menu(): HTMLElement | null {
  return document.querySelector<HTMLElement>(
    '[role="menu"][aria-label="Cell actions"]',
  );
}

function items(): HTMLElement[] {
  const m = menu();
  return m ? [...m.querySelectorAll<HTMLElement>('[role^="menuitem"]')] : [];
}

function item(label: string): HTMLElement {
  const found = items().find((i) => i.textContent === label);
  if (!found) throw new Error(`no menu item ${label}`);
  return found;
}

function contextMenu(el: HTMLElement): MouseEvent {
  const event = new MouseEvent("contextmenu", {
    bubbles: true,
    cancelable: true,
    clientX: 10,
    clientY: 10,
  });
  el.dispatchEvent(event);
  return event;
}

function key(
  el: Element,
  k: string,
  opts: { shift?: boolean } = {},
): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key: k,
    shiftKey: opts.shift ?? false,
    bubbles: true,
    cancelable: true,
  });
  el.dispatchEvent(event);
  return event;
}

function open(main: EditorView, row: number, column: number): EditorView {
  const pos = cellOf(main, row, column).content.to;
  main.dispatch({
    effects: setOpenCell.of({ anchor: pos, head: pos, focused: true }),
  });
  const editor = hostOpenCell(main, "now");
  if (!editor) throw new Error("not hosted");
  return editor.view;
}

function where(main: EditorView): [number, number] | null {
  const cell = openCellRange(main.state);
  return cell ? [cell.row, cell.column] : null;
}

/** What a verb writes on a plain buffer holding DOC, at a cell's content. */
function expected(
  row: number,
  column: number,
  run: (view: EditorView, at: number) => boolean,
): string {
  const plain = soloMain(DOC);
  const at = cellOf(plain, row, column).content.from;
  expect(run(plain, at)).toBe(true);
  return bytes(plain.state);
}

const CASES: [
  string,
  number,
  number,
  (view: EditorView, at: number) => boolean,
][] = [
  ["Insert row above", 2, 1, (v, at) => tableAddRowAbove(v, at)],
  ["Insert row below", 1, 0, (v, at) => tableAddRowBelow(v, at)],
  ["Move row up", 2, 0, (v, at) => tableMoveRow(v, -1, at)],
  ["Move row down", 1, 0, (v, at) => tableMoveRow(v, 1, at)],
  ["Delete row", 1, 1, (v, at) => tableDeleteRow(v, at)],
  ["Insert column left", 1, 1, (v, at) => tableAddColumnBefore(v, at)],
  ["Insert column right", 1, 0, (v, at) => tableAddColumnAfter(v, at)],
  ["Move column left", 0, 1, (v, at) => tableMoveColumn(v, -1, at)],
  ["Move column right", 1, 0, (v, at) => tableMoveColumn(v, 1, at)],
  ["Delete column", 1, 1, (v, at) => tableDeleteColumn(v, at)],
  ["Align left", 1, 0, (v, at) => tableAlignColumn(v, "left", at)],
  ["Align center", 1, 0, (v, at) => tableAlignColumn(v, "center", at)],
  ["Align right", 1, 0, (v, at) => tableAlignColumn(v, "right", at)],
];

describe("the Cell actions menu", () => {
  for (const [label, row, column, run] of CASES) {
    test(`${label} writes what the verb writes and is one undo step`, () => {
      const main = soloMain(DOC, layers());
      const want = expected(row, column, run);
      expect(want).not.toBe(DOC);
      contextMenu(cellEl(main, row, column));
      item(label).click();
      expect(bytes(main.state)).toBe(want);
      expect(menu()).toBeNull();
      undo(main);
      expect(bytes(main.state)).toBe(DOC);
    });
  }

  test("Align center changes only the delimiter cell of that column", () => {
    const main = soloMain(DOC, layers());
    contextMenu(cellEl(main, 1, 0));
    item("Align center").click();
    expect(bytes(main.state)).toBe(DOC.replace("| - | - |", "| :---: | - |"));
  });

  test("the items are menuitems in the listed order, alignments radios", () => {
    const main = soloMain(DOC, layers());
    contextMenu(cellEl(main, 1, 0));
    expect(items().map((i) => i.textContent)).toEqual([
      "Insert row above",
      "Insert row below",
      "Move row up",
      "Move row down",
      "Delete row",
      "Insert column left",
      "Insert column right",
      "Move column left",
      "Move column right",
      "Delete column",
      "Align left",
      "Align center",
      "Align right",
    ]);
    for (const label of ["Align left", "Align center", "Align right"]) {
      expect(item(label).getAttribute("role")).toBe("menuitemradio");
    }
    expect(item("Insert row above").getAttribute("role")).toBe("menuitem");
  });

  test("the current alignment is aria-checked", () => {
    const main = soloMain(DOC.replace("| - | - |", "| :-: | - |"), layers());
    contextMenu(cellEl(main, 1, 0));
    expect(item("Align center").getAttribute("aria-checked")).toBe("true");
    expect(item("Align left").getAttribute("aria-checked")).toBe("false");
    contextMenu(cellEl(main, 1, 1));
    expect(item("Align center").getAttribute("aria-checked")).toBe("false");
  });

  test("items that cannot apply are aria-disabled", () => {
    const main = soloMain(DOC, layers());
    contextMenu(cellEl(main, 0, 0)); // header, first column
    for (const label of [
      "Insert row above",
      "Move row up",
      "Move row down",
      "Delete row",
      "Move column left",
    ]) {
      expect(item(label).getAttribute("aria-disabled")).toBe("true");
    }
    expect(item("Insert row below").getAttribute("aria-disabled")).toBeNull();
    item("Delete row").click(); // a disabled item does nothing
    expect(bytes(main.state)).toBe(DOC);
    expect(menu()).not.toBeNull();
  });

  test("move up on the first data row and move down on the last are disabled", () => {
    const main = soloMain(DOC, layers());
    contextMenu(cellEl(main, 1, 0));
    expect(item("Move row up").getAttribute("aria-disabled")).toBe("true");
    expect(item("Move row down").getAttribute("aria-disabled")).toBeNull();
    contextMenu(cellEl(main, 2, 0));
    expect(item("Move row down").getAttribute("aria-disabled")).toBe("true");
  });

  test("right click takes the context menu from the browser", () => {
    const main = soloMain(DOC, layers());
    const event = contextMenu(cellEl(main, 1, 0));
    expect(event.defaultPrevented).toBe(true);
    expect(menu()).not.toBeNull();
  });

  test("Shift+F10 opens it with focus on the first item, Escape goes back", () => {
    const main = soloMain(DOC, layers());
    const el = cellEl(main, 1, 1);
    el.focus();
    key(el, "F10", { shift: true });
    expect(menu()).not.toBeNull();
    expect(document.activeElement).toBe(items()[0]);
    key(document.activeElement as HTMLElement, "Escape");
    expect(menu()).toBeNull();
    expect(document.activeElement).toBe(el);
  });

  test("the context-menu key opens it from the cell editor, Escape returns to it", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 1);
    key(view.contentDOM, "ContextMenu");
    expect(menu()).not.toBeNull();
    expect(document.activeElement).toBe(items()[0]);
    key(document.activeElement as HTMLElement, "Escape");
    expect(menu()).toBeNull();
    expect(view.contentDOM.contains(document.activeElement)).toBe(true);
  });

  test("arrows move between items, Enter runs the focused one", () => {
    const main = soloMain(DOC, layers());
    const el = cellEl(main, 1, 0);
    el.focus();
    key(el, "ContextMenu");
    const first = document.activeElement as HTMLElement;
    key(first, "ArrowDown");
    expect(document.activeElement).toBe(items()[1]);
    key(document.activeElement as HTMLElement, "ArrowUp");
    expect(document.activeElement).toBe(items()[0]);
    key(document.activeElement as HTMLElement, "ArrowUp");
    expect(document.activeElement).toBe(items().at(-1));
    key(document.activeElement as HTMLElement, "ArrowDown");
    key(document.activeElement as HTMLElement, "ArrowDown");
    key(document.activeElement as HTMLElement, "Enter"); // Insert row below
    expect(bytes(main.state)).toBe(
      expected(1, 0, (v, at) => tableAddRowBelow(v, at)),
    );
  });

  test("the Cell actions button shows on the focused cell and opens the menu", () => {
    const main = soloMain(DOC, layers());
    expect(button("Cell actions")).toBeNull();
    const el = cellEl(main, 1, 1);
    el.dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
    const small = button("Cell actions");
    expect(small).not.toBeNull();
    small?.click();
    expect(menu()).not.toBeNull();
    item("Delete column").click();
    expect(bytes(main.state)).toBe(
      expected(1, 1, (v, at) => tableDeleteColumn(v, at)),
    );
  });

  test("hover alone draws no Cell actions button (finding 2)", () => {
    const main = soloMain(DOC, layers());
    layout(main);
    const el = cellEl(main, 2, 0);
    el.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    move(el, 50, 75);
    expect(button("Cell actions")).toBeNull();
  });

  test("the button stands on the focused cell's top-right corner, half outside", () => {
    const main = soloMain(DOC, layers());
    layout(main);
    // Cell 1/1 is the box 100..200 x 30..60; the button is 16 px square.
    cellEl(main, 1, 1).focus();
    const small = button("Cell actions");
    expect(small?.style.left).toBe("192px");
    expect(small?.style.top).toBe("22px");
  });

  test("the open cell gets the button too, and a click on another cell opens that cell", () => {
    const main = soloMain(DOC, layers());
    layout(main);
    open(main, 1, 0);
    expect(button("Cell actions")).not.toBeNull();
    const other = cellEl(main, 2, 1);
    other.dispatchEvent(
      new MouseEvent("mousedown", {
        bubbles: true,
        cancelable: true,
        button: 0,
        clientX: 150,
        clientY: 75,
      }),
    );
    expect(where(main)).toEqual([2, 1]);
    expect(menu()).toBeNull();
  });

  test("a hovered plus takes the shared corner; the button comes back after", () => {
    const main = soloMain(DOC, layers());
    layout(main);
    cellEl(main, 0, 0).focus();
    expect(button("Cell actions")).not.toBeNull();
    move(cellEl(main, 0, 0), 98, 15);
    expect(button("Insert column here")).not.toBeNull();
    expect(button("Cell actions")).toBeNull();
    move(cellEl(main, 0, 0), 50, 15);
    expect(button("Insert column here")).toBeNull();
    expect(button("Cell actions")).not.toBeNull();
  });

  test("a new row is opened in its first cell after Insert row below", () => {
    const main = soloMain(DOC, layers());
    contextMenu(cellEl(main, 1, 1));
    item("Insert row below").click();
    expect(where(main)).toEqual([2, 1]);
    expect(activeCell(main)).not.toBeNull();
  });

  test("Move row down opens the cell in the moved row", () => {
    const main = soloMain(DOC, layers());
    contextMenu(cellEl(main, 1, 1));
    item("Move row down").click();
    expect(where(main)).toEqual([2, 1]);
  });

  test("the main view does not take the focus", () => {
    const main = soloMain(DOC, layers());
    contextMenu(cellEl(main, 1, 1));
    item("Insert column right").click();
    expect(activeCell(main)?.view.dom.contains(document.activeElement)).toBe(
      true,
    );
    expect(main.hasFocus).toBe(false);
  });
});

/**
 * R23: every menu item on a table without outer pipes, with the bytes it
 * writes. Each row keeps its own edge spelling; an edge pipe appears only
 * where a cell needs it (an empty edge cell, a one-column table, a cell
 * that would open a line with a blank).
 */
const SPELLED: Record<string, Record<string, string[]>> = {
  "pipeless.md": {
    "Insert row above": ["a | b", "--|--", "1 | 2", "|  |  |", "3 | 4"],
    "Insert row below": ["a | b", "--|--", "1 | 2", "|  |  |", "3 | 4"],
    "Move row up": ["a | b", "--|--", "3 | 4", "1 | 2"],
    "Move row down": ["a | b", "--|--", "3 | 4", "1 | 2"],
    "Delete row": ["a | b", "--|--", "3 | 4"],
    "Insert column left": [
      "a | Column | b",
      "--| --- |--",
      "1 |  | 2",
      "3 |  | 4",
    ],
    "Insert column right": [
      "a | Column | b",
      "--| --- |--",
      "1 |  | 2",
      "3 |  | 4",
    ],
    "Move column left": ["| b|a ", "--|--", "| 2|1 ", "| 4|3 "],
    "Move column right": ["| b|a ", "--|--", "| 2|1 ", "| 4|3 "],
    "Delete column": ["| a |", "| --|", "| 1 |", "| 3 |"],
    "Align left": ["a | b", ":--- |--", "1 | 2", "3 | 4"],
    "Align center": ["a | b", ":---: |--", "1 | 2", "3 | 4"],
    "Align right": ["a | b", "---: |--", "1 | 2", "3 | 4"],
  },
  "mixed.md": {
    "Insert row above": [
      "| a | b |",
      "|:--|--:|",
      "1 | 2",
      "|  |  |",
      "| 3 | 4 |",
    ],
    "Insert row below": [
      "| a | b |",
      "|:--|--:|",
      "1 | 2",
      "|  |  |",
      "| 3 | 4 |",
    ],
    "Move row up": ["| a | b |", "|:--|--:|", "| 3 | 4 |", "1 | 2"],
    "Move row down": ["| a | b |", "|:--|--:|", "| 3 | 4 |", "1 | 2"],
    "Delete row": ["| a | b |", "|:--|--:|", "| 3 | 4 |"],
    "Insert column left": [
      "| a | Column | b |",
      "|:--| --- |--:|",
      "1 |  | 2",
      "| 3 |  | 4 |",
    ],
    "Insert column right": [
      "| a | Column | b |",
      "|:--| --- |--:|",
      "1 |  | 2",
      "| 3 |  | 4 |",
    ],
    "Move column left": ["| b | a |", "|--:|:--|", "| 2|1 ", "| 4 | 3 |"],
    "Move column right": ["| b | a |", "|--:|:--|", "| 2|1 ", "| 4 | 3 |"],
    "Delete column": ["| a |", "|:--|", "| 1 |", "| 3 |"],
    "Align left": ["| a | b |", "| :--- |--:|", "1 | 2", "| 3 | 4 |"],
    "Align center": ["| a | b |", "| :---: |--:|", "1 | 2", "| 3 | 4 |"],
    "Align right": ["| a | b |", "| ---: |--:|", "1 | 2", "| 3 | 4 |"],
  },
};

describe.each(PIPELESS_FIXTURES)(
  "the Cell actions menu without outer pipes: $name",
  (f: PipelessFixture) => {
    for (const [label, row, column] of CASES) {
      test(`${label} writes the table's own spelling, one undo step`, () => {
        const main = soloMain(f.text, layers());
        contextMenu(cellEl(main, row, column));
        item(label).click();
        const lines = SPELLED[f.name]?.[label];
        if (!lines) throw new Error(`no spelling for ${label}`);
        expect(bytes(main.state)).toBe(withTable(f, lines));
        expect(menu()).toBeNull();
        undo(main);
        expect(bytes(main.state)).toBe(f.text);
      });
    }

    test("the plus buttons insert a column and a row in its spelling", () => {
      const main = soloMain(f.text, layers());
      layout(main);
      move(cellEl(main, 0, 1), 102, 15);
      button("Insert column here")?.click();
      expect(bytes(main.state)).toBe(
        withTable(f, SPELLED[f.name]?.["Insert column left"] ?? []),
      );
      expect(where(main)).toEqual([0, 1]);
      undo(main);
      expect(bytes(main.state)).toBe(f.text);
      layout(main);
      move(cellEl(main, 2, 0), 5, 61);
      button("Insert row here")?.click();
      expect(bytes(main.state)).toBe(
        withTable(f, SPELLED[f.name]?.["Insert row above"] ?? []),
      );
      expect(where(main)).toEqual([2, 0]);
    });
  },
);

describe("deleting under an open cell", () => {
  test("Delete row on the open cell's row leaves focus off the main view", async () => {
    const main = soloMain(DOC, layers());
    open(main, 2, 1);
    contextMenu(cellEl(main, 2, 1));
    item("Delete row").click();
    await Promise.resolve();
    await Promise.resolve();
    expect(bytes(main.state)).toBe(DOC.replace("| 3 | 4 |\n", ""));
    expect(main.hasFocus).toBe(false);
    expect(main.dom.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).not.toBe(document.body);
  });

  test("Delete column leaves the focus on a cell of the table", async () => {
    const main = soloMain(DOC, layers());
    const el = cellEl(main, 1, 1);
    el.focus();
    contextMenu(el);
    item("Delete column").click();
    await Promise.resolve();
    expect(main.hasFocus).toBe(false);
    expect(main.dom.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).not.toBe(document.body);
  });
});

describe("a read-only page (R17)", () => {
  function readOnly(): EditorView {
    return soloMain(DOC, layers([EditorView.editable.of(false)]));
  }

  test("no menu opens and the document stays", () => {
    const main = readOnly();
    const event = contextMenu(cellEl(main, 1, 0));
    expect(menu()).toBeNull();
    expect(event.defaultPrevented).toBe(false);
    const el = cellEl(main, 1, 0);
    el.focus();
    key(el, "F10", { shift: true });
    expect(menu()).toBeNull();
    expect(bytes(main.state)).toBe(DOC);
  });

  test("no Cell actions button and no plus buttons are drawn", () => {
    const main = readOnly();
    layout(main);
    cellEl(main, 1, 0).dispatchEvent(
      new FocusEvent("focusin", { bubbles: true }),
    );
    cellEl(main, 1, 0).dispatchEvent(
      new MouseEvent("mouseover", { bubbles: true }),
    );
    expect(button("Cell actions")).toBeNull();
    move(cellEl(main, 0, 1), 102, 15);
    move(cellEl(main, 2, 0), 5, 61);
    expect(button("Insert column here")).toBeNull();
    expect(button("Insert row here")).toBeNull();
  });
});

describe("the hover plus buttons", () => {
  test("Insert column here between columns 1 and 2 inserts before column 2", () => {
    const main = soloMain(DOC, layers());
    layout(main);
    expect(button("Insert column here")).toBeNull();
    move(cellEl(main, 0, 1), 102, 15);
    const plus = button("Insert column here");
    expect(plus).not.toBeNull();
    plus?.click();
    expect(bytes(main.state)).toBe(
      expected(0, 1, (v, at) => tableAddColumnBefore(v, at)),
    );
    expect(where(main)).toEqual([0, 1]);
    undo(main);
    expect(bytes(main.state)).toBe(DOC);
  });

  test("the border from the left-hand cell's right edge is the same border", () => {
    const main = soloMain(DOC, layers());
    layout(main);
    move(cellEl(main, 0, 0), 98, 15);
    button("Insert column here")?.click();
    expect(bytes(main.state)).toBe(
      expected(0, 1, (v, at) => tableAddColumnBefore(v, at)),
    );
  });

  test("away from a border there is no button", () => {
    const main = soloMain(DOC, layers());
    layout(main);
    move(cellEl(main, 0, 1), 150, 15);
    expect(button("Insert column here")).toBeNull();
    move(cellEl(main, 0, 1), 102, 15);
    expect(button("Insert column here")).not.toBeNull();
    main.dom
      .querySelector(".cm-grid")
      ?.dispatchEvent(new MouseEvent("mouseleave"));
    expect(button("Insert column here")).toBeNull();
  });

  test("Insert row here between two data rows inserts above the lower row", () => {
    const main = soloMain(DOC, layers());
    layout(main);
    move(cellEl(main, 2, 0), 5, 61);
    const plus = button("Insert row here");
    expect(plus).not.toBeNull();
    plus?.click();
    expect(bytes(main.state)).toBe(
      expected(2, 0, (v, at) => tableAddRowAbove(v, at)),
    );
    expect(where(main)).toEqual([2, 0]);
    undo(main);
    expect(bytes(main.state)).toBe(DOC);
  });
});

describe("the menu's life (findings 4 to 6, T7)", () => {
  test("a mousedown outside closes it and leaves the document", () => {
    const main = soloMain(DOC, layers());
    contextMenu(cellEl(main, 1, 0));
    expect(menu()).not.toBeNull();
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(menu()).toBeNull();
    expect(bytes(main.state)).toBe(DOC);
  });

  test("a mousedown inside it keeps it open", () => {
    const main = soloMain(DOC, layers());
    contextMenu(cellEl(main, 1, 0));
    items()[0]?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(menu()).not.toBeNull();
  });

  test("Escape from a menu opened by right click puts the focus on its cell", () => {
    const main = soloMain(DOC, layers());
    const el = cellEl(main, 2, 1);
    contextMenu(el);
    key(document.activeElement as HTMLElement, "Escape");
    expect(menu()).toBeNull();
    expect(document.activeElement).toBe(el);
  });

  test("dispose removes the menu and its document listeners", () => {
    const main = soloMain(DOC, layers());
    contextMenu(cellEl(main, 1, 0));
    const removed = vi.spyOn(document, "removeEventListener");
    try {
      // The table goes, and its widget with it.
      main.dispatch({ changes: { from: 0, to: main.state.doc.length } });
      expect(menu()).toBeNull();
      const kinds = removed.mock.calls.map((call) => call[0]);
      expect(kinds).toContain("mousedown");
      expect(kinds).toContain("scroll");
    } finally {
      removed.mockRestore();
    }
  });

  test("a peer's edit to a cell of the table closes it, focus back on the cell (finding 4)", () => {
    const { main, rtext } = roomMain(DOC, layers());
    const el = cellEl(main, 2, 0);
    el.focus();
    key(el, "F10", { shift: true });
    expect(menu()).not.toBeNull();
    // Same shape: the peer types into "3", the row the menu points at.
    rtext.insert(DOC.indexOf("| 3") + 3, "9");
    expect(bytes(main.state)).toContain("| 39 |");
    expect(menu()).toBeNull();
    expect(document.activeElement).toBe(cellEl(main, 2, 0));
  });

  test("a peer's edit outside the table leaves it open", () => {
    const { main, rtext } = roomMain(DOC, layers());
    contextMenu(cellEl(main, 2, 0));
    rtext.insert(0, "More ");
    expect(menu()).not.toBeNull();
    item("Delete row").click();
    expect(bytes(main.state)).toBe(
      "More Intro\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\nOutro\n",
    );
  });

  test("a rebuild under a focused menu keeps the focus in the table (finding 5)", async () => {
    const { main, rtext } = roomMain(DOC, layers());
    const el = cellEl(main, 2, 1);
    el.focus();
    key(el, "F10", { shift: true });
    expect(document.activeElement).toBe(items()[0]);
    // A new row above: a shape change, so the widget is built again.
    rtext.insert(DOC.indexOf("| 3"), "| x | y |\n");
    await Promise.resolve();
    expect(menu()).toBeNull();
    expect(document.activeElement).not.toBe(document.body);
    expect(document.activeElement).toBe(cellEl(main, 3, 1));
  });

  test("a rebuild under a focused menu from an open cell refocuses the cell", async () => {
    const { main, rtext } = roomMain(DOC, layers());
    const view = open(main, 1, 1);
    key(view.contentDOM, "ContextMenu");
    expect(document.activeElement).toBe(items()[0]);
    rtext.insert(DOC.indexOf("| 3"), "| x | y |\n");
    await Promise.resolve();
    const cell = activeCell(main);
    expect(cell).not.toBeNull();
    expect(cell?.view.contentDOM.contains(document.activeElement)).toBe(true);
    expect(where(main)).toEqual([1, 1]);
  });

  test("it is fixed, below its anchor with room (finding 6)", () => {
    const main = soloMain(DOC, layers());
    layout(main);
    contextMenu(cellEl(main, 1, 0));
    const m = menu();
    expect(m?.style.position).toBe("fixed");
    expect(m?.style.top).toBe("10px");
    expect(m?.style.left).toBe("10px");
  });

  test("it flips above its anchor when there is no room below", () => {
    const main = soloMain(DOC, layers());
    layout(main);
    const height = vi
      .spyOn(HTMLElement.prototype, "offsetHeight", "get")
      .mockReturnValue(200);
    const room = vi.spyOn(window, "innerHeight", "get").mockReturnValue(400);
    try {
      const el = cellEl(main, 2, 0);
      el.dispatchEvent(
        new MouseEvent("contextmenu", {
          bubbles: true,
          cancelable: true,
          clientX: 10,
          clientY: 350,
        }),
      );
      expect(menu()?.style.top).toBe("150px");
    } finally {
      height.mockRestore();
      room.mockRestore();
    }
  });
});

describe("a new header lands selected (finding 10)", () => {
  test("typing after the column plus replaces the placeholder", () => {
    const main = soloMain(DOC, layers());
    layout(main);
    move(cellEl(main, 0, 1), 102, 15);
    button("Insert column here")?.click();
    const cell = activeCell(main);
    if (!cell) throw new Error("no cell");
    typeIn(cell, "Name");
    expect(bytes(main.state)).toContain("| a | Name | b |");
  });

  test("Insert column right from the header selects the new header", () => {
    const main = soloMain(DOC, layers());
    contextMenu(cellEl(main, 0, 0));
    item("Insert column right").click();
    const cell = activeCell(main);
    if (!cell) throw new Error("no cell");
    typeIn(cell, "Role");
    expect(bytes(main.state)).toContain("| a | Role | b |");
  });

  test("Insert column right from a data row lands in the empty data cell", () => {
    const main = soloMain(DOC, layers());
    contextMenu(cellEl(main, 1, 0));
    item("Insert column right").click();
    const cell = activeCell(main);
    if (!cell) throw new Error("no cell");
    expect(cell.view.state.selection.main.empty).toBe(true);
  });
});

describe("a landing after a slow parse (finding 13)", () => {
  /**
   * The race behind the old flake of "Insert row here between two data
   * rows": a document change gets 20 ms of `Date.now()` to reparse, and a
   * landing that read the grown table before the rest was parsed found
   * nothing to open. A clock that moves 21 ms per read makes every such
   * budget run out after one step, so the test no longer depends on load.
   */
  function slowClock(): MockInstance<typeof Date.now> {
    let now = Date.now();
    return vi.spyOn(Date, "now").mockImplementation(() => {
      now += 21;
      return now;
    });
  }

  test("Insert row here lands in the new row", () => {
    const main = soloMain(DOC, layers());
    layout(main);
    move(cellEl(main, 2, 0), 5, 61);
    const plus = button("Insert row here");
    expect(plus).not.toBeNull();
    const clock = slowClock();
    try {
      plus?.click();
    } finally {
      clock.mockRestore();
    }
    expect(bytes(main.state)).toBe(
      expected(2, 0, (v, at) => tableAddRowAbove(v, at)),
    );
    expect(where(main)).toEqual([2, 0]);
  });

  test("Insert row below from the menu lands in the new row", () => {
    const main = soloMain(DOC, layers());
    contextMenu(cellEl(main, 1, 1));
    const clock = slowClock();
    try {
      item("Insert row below").click();
    } finally {
      clock.mockRestore();
    }
    expect(where(main)).toEqual([2, 1]);
  });
});

describe("a plus click is its own undo step beside typing", () => {
  function run(main: EditorView): string[] {
    const states: string[] = [bytes(main.state)];
    open(main, 1, 0);
    const first = activeCell(main);
    if (!first) throw new Error("no cell");
    typeIn(first, "X");
    states.push(bytes(main.state));
    layout(main);
    move(cellEl(main, 0, 1), 102, 15);
    button("Insert column here")?.click();
    states.push(bytes(main.state));
    const added = activeCell(main);
    if (!added) throw new Error("no cell in the new column");
    typeIn(added, "Y");
    states.push(bytes(main.state));
    return states;
  }

  function walk(main: EditorView, states: string[]): void {
    expect(new Set(states).size).toBe(4);
    for (let i = 2; i >= 0; i -= 1) {
      runMainUndo(main, "undo");
      expect(bytes(main.state)).toBe(states[i]);
    }
    for (let i = 1; i <= 3; i += 1) {
      runMainUndo(main, "redo");
      expect(bytes(main.state)).toBe(states[i]);
    }
  }

  test("solo", () => {
    const main = soloMain(DOC, layers());
    walk(main, run(main));
  });

  test("in a room", () => {
    const { main, rtext } = roomMain(DOC, layers());
    const states = run(main);
    expect(rtext.toJSON()).toBe(states[3]);
    walk(main, states);
    expect(rtext.toJSON()).toBe(states[3]);
  });
});
