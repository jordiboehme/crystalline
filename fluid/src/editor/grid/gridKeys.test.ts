/**
 * The grid's keyboard: keys inside an open cell, grid navigation on a
 * focused `th`/`td`, and the way in from the main view. Each test asserts
 * the open cell (`openCellField`), the document bytes and where the focus is.
 */

import type { Extension } from "@codemirror/state";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import {
  bytes,
  cleanupCells,
  press,
  roomMain,
  soloMain,
  typeIn,
} from "../../test/cellKit";
import * as Y from "yjs";

import {
  PIPELESS_FIXTURES,
  tableStart,
  withTable,
} from "../../test/pipelessTables";
import { fencePreviews } from "../fencePreviews";
import { activeCell } from "./cellEditor";
import { hostOpenCell } from "./gridWidget";
import { gridExtension } from "./index";
import type { CloseReason } from "./openCell";
import {
  onCellClosed,
  openCellField,
  openCellRange,
  setOpenCell,
} from "./openCell";
import { gridTables } from "./structure";

/** Lines: 1 Intro, 2 blank, 3-5 the table, 6 blank, 7 Outro, 8 empty. */
const DOC = "Intro\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\nOutro\n";

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

function solo(text = DOC): EditorView {
  return soloMain(text, layers());
}

function grid(main: EditorView): HTMLTableElement {
  const table = main.dom.querySelector<HTMLTableElement>('table[role="grid"]');
  if (!table) throw new Error("no grid");
  return table;
}

function cellEl(main: EditorView, row: number, column: number): HTMLElement {
  const el = grid(main).querySelector<HTMLElement>(
    `[aria-rowindex="${String(row + 1)}"][aria-colindex="${String(column + 1)}"]`,
  );
  if (!el) throw new Error(`no cell ${String(row)}/${String(column)}`);
  return el;
}

/** The table's cell at `row` and `column`, from the current state. */
function cellAt(main: EditorView, row: number, column: number) {
  const cell = gridTables(main.state)[0]?.rows[row]?.[column];
  if (!cell) throw new Error(`no cell ${String(row)}/${String(column)}`);
  return cell;
}

/** Opens a cell through the intent, with the caret at `at` (default end). */
function open(
  main: EditorView,
  row: number,
  column: number,
  at?: number,
): EditorView {
  const pos = at ?? cellAt(main, row, column).content.to;
  main.dispatch({
    effects: setOpenCell.of({ anchor: pos, head: pos, focused: true }),
  });
  const editor = hostOpenCell(main, "now");
  if (!editor) throw new Error("not hosted");
  return editor.view;
}

function cellView(main: EditorView): EditorView {
  const cell = activeCell(main);
  if (!cell) throw new Error("no cell editor mounted");
  return cell.view;
}

/** Row and column of the open cell, or null. */
function where(main: EditorView): [number, number] | null {
  const cell = openCellRange(main.state);
  return cell ? [cell.row, cell.column] : null;
}

/** The open cell is `row`/`column`, hosted in its element and focused. */
function expectOpen(main: EditorView, row: number, column: number): void {
  expect(where(main)).toEqual([row, column]);
  const view = cellView(main);
  expect(cellEl(main, row, column).contains(view.dom)).toBe(true);
  expect(view.dom.contains(document.activeElement)).toBe(true);
  expect(main.state.field(openCellField)?.focused).toBe(true);
}

function head(main: EditorView): number {
  return cellView(main).state.selection.main.head;
}

/** A keydown on `el`, shaped like a browser's. */
function keyOn(
  el: HTMLElement,
  key: string,
  init: KeyboardEventInit = {},
): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key,
    bubbles: true,
    cancelable: true,
    ...init,
  });
  el.dispatchEvent(event);
  return event;
}

const mac = /Mac/.test(navigator.platform);
const modKey = mac ? { metaKey: true } : { ctrlKey: true };

const flush = () => new Promise<void>((resolve) => queueMicrotask(resolve));

describe("in a cell: Tab and Shift+Tab", () => {
  test("Tab walks the cells row by row", () => {
    const main = solo();
    open(main, 0, 0);
    expect(press(cellView(main), "Tab")).toBe(true);
    expectOpen(main, 0, 1);
    press(cellView(main), "Tab");
    expectOpen(main, 1, 0);
    expect(bytes(main.state)).toBe(DOC);
  });

  test("Shift+Tab goes back row by row, and the first header cell stays", () => {
    const main = solo();
    open(main, 1, 0);
    expect(press(cellView(main), "Tab", { shift: true })).toBe(true);
    expectOpen(main, 0, 1);
    press(cellView(main), "Tab", { shift: true });
    expectOpen(main, 0, 0);
    expect(press(cellView(main), "Tab", { shift: true })).toBe(true);
    expectOpen(main, 0, 0);
    expect(bytes(main.state)).toBe(DOC);
  });

  test("the roving tab stop follows the open cell", () => {
    const main = solo();
    open(main, 0, 0);
    press(cellView(main), "Tab");
    expect(cellEl(main, 0, 1).tabIndex).toBe(0);
    expect(cellEl(main, 0, 0).tabIndex).toBe(-1);
  });

  test("Tab in the last cell adds a row and opens its first cell", () => {
    const main = solo();
    open(main, 1, 1);
    expect(press(cellView(main), "Tab")).toBe(true);
    expect(gridTables(main.state)[0]?.rows.length).toBe(3);
    expectOpen(main, 2, 0);
    expect(bytes(main.state)).toBe(
      "Intro\n\n| a | b |\n| - | - |\n| 1 | 2 |\n|  |  |\n\nOutro\n",
    );
  });

  test("Tab in the last cell, then Mod-z, restores the bytes and the cell", async () => {
    const main = solo();
    const view = open(main, 1, 1);
    const before = head(main);
    press(view, "Tab");
    expectOpen(main, 2, 0);
    expect(press(cellView(main), "z", { mod: true })).toBe(true);
    expect(bytes(main.state)).toBe(DOC);
    expectOpen(main, 1, 1);
    expect(head(main)).toBe(before);
    await flush();
    expectOpen(main, 1, 1);
  });

  test("the added row is its own undo step on both sides", () => {
    const main = solo();
    const view = open(main, 1, 1);
    typeIn({ view, destroy: () => undefined }, "x");
    press(view, "Tab");
    typeIn(activeCell(main) ?? { view, destroy: () => undefined }, "y");
    const withY =
      "Intro\n\n| a | b |\n| - | - |\n| 1 | 2x |\n| y |  |\n\nOutro\n";
    expect(bytes(main.state)).toBe(withY);
    press(cellView(main), "z", { mod: true });
    expect(bytes(main.state)).toBe(
      "Intro\n\n| a | b |\n| - | - |\n| 1 | 2x |\n|  |  |\n\nOutro\n",
    );
    press(cellView(main), "z", { mod: true });
    expect(bytes(main.state)).toBe(
      "Intro\n\n| a | b |\n| - | - |\n| 1 | 2x |\n\nOutro\n",
    );
    expectOpen(main, 1, 1);
  });

  test("in a room, Tab in the last cell then Mod-z restores the bytes and the cell", () => {
    const { main, rtext } = roomMain(DOC, layers());
    open(main, 1, 1);
    press(cellView(main), "Tab");
    expectOpen(main, 2, 0);
    expect(rtext.toJSON()).toContain("|  |  |");
    expect(press(cellView(main), "z", { mod: true })).toBe(true);
    expect(bytes(main.state)).toBe(DOC);
    expect(rtext.toJSON()).toBe(DOC);
    expectOpen(main, 1, 1);
  });
});

describe("in a room, the added row is its own undo step (R14, R15)", () => {
  test("Mod-z after typing and Tab in the last cell keeps the typing", () => {
    const { main, rtext } = roomMain(DOC, layers());
    const view = open(main, 1, 1);
    typeIn({ view, destroy: () => undefined }, "x");
    press(view, "Tab");
    press(cellView(main), "z", { mod: true });
    const typed = DOC.replace("| 2 |", "| 2x |");
    expect(bytes(main.state)).toBe(typed);
    expect(rtext.toJSON()).toBe(typed);
    expectOpen(main, 1, 1);
  });

  test("typing, Tab in the last cell, typing: three undo steps", () => {
    const { main } = roomMain(DOC, layers());
    const view = open(main, 1, 1);
    typeIn({ view, destroy: () => undefined }, "x");
    press(view, "Tab");
    const added = cellView(main);
    typeIn({ view: added, destroy: () => undefined }, "y");
    expect(bytes(main.state)).toBe(
      "Intro\n\n| a | b |\n| - | - |\n| 1 | 2x |\n| y |  |\n\nOutro\n",
    );
    press(cellView(main), "z", { mod: true });
    expect(bytes(main.state)).toBe(
      "Intro\n\n| a | b |\n| - | - |\n| 1 | 2x |\n|  |  |\n\nOutro\n",
    );
    expectOpen(main, 2, 0);
    press(cellView(main), "z", { mod: true });
    expect(bytes(main.state)).toBe(DOC.replace("| 2 |", "| 2x |"));
    expectOpen(main, 1, 1);
    press(cellView(main), "z", { mod: true });
    expect(bytes(main.state)).toBe(DOC);
  });

  test("after a row-adding Tab the peer sees no main cursor in the table", async () => {
    const { main, awareness, remoteAwareness, local } = roomMain(DOC, layers());
    open(main, 1, 1);
    expect(main.hasFocus).toBe(false);
    press(cellView(main), "Tab");
    expectOpen(main, 2, 0);
    expect(main.hasFocus).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 20));
    const table = gridTables(main.state)[0];
    if (!table) throw new Error("no table");
    const state = remoteAwareness.getStates().get(awareness.clientID) as
      { cursor?: { anchor: unknown; head: unknown } | null } | undefined;
    const cursor = state?.cursor ?? null;
    if (cursor !== null) {
      // Whatever is published lies outside the table, or is the open
      // cell's own caret (Task 6 publishes it), never main's.
      const caret = cellView(main).state.selection.main;
      const at = [cursor.anchor, cursor.head].map(
        (rel) =>
          Y.createAbsolutePositionFromRelativePosition(
            Y.createRelativePositionFromJSON(rel),
            local,
          )?.index ?? -1,
      );
      const outside = at.every((i) => i < table.from || i > table.to);
      expect(outside || (at[0] === caret.anchor && at[1] === caret.head)).toBe(
        true,
      );
      expectOpen(main, 2, 0);
    }
  });
});

describe("in a cell: Enter", () => {
  test("Enter moves to the cell below in the same column", () => {
    const main = solo();
    open(main, 0, 1);
    expect(press(cellView(main), "Enter")).toBe(true);
    expectOpen(main, 1, 1);
    expect(bytes(main.state)).toBe(DOC);
  });

  test("Enter in the last row adds a row and opens the same column", () => {
    const main = solo();
    open(main, 1, 1);
    press(cellView(main), "Enter");
    expectOpen(main, 2, 1);
    expect(bytes(main.state)).toBe(
      "Intro\n\n| a | b |\n| - | - |\n| 1 | 2 |\n|  |  |\n\nOutro\n",
    );
    press(cellView(main), "z", { mod: true });
    expect(bytes(main.state)).toBe(DOC);
    expectOpen(main, 1, 1);
  });

  test("Shift+Enter still inserts a line break tag", () => {
    const main = solo();
    open(main, 1, 0);
    press(cellView(main), "Enter", { shift: true });
    expect(bytes(main.state)).toContain("| 1<br> | 2 |");
    expectOpen(main, 1, 0);
  });
});

describe("in a cell: arrows", () => {
  test("ArrowLeft at the content start goes to the previous cell's end", () => {
    const main = solo();
    open(main, 1, 0, cellAt(main, 1, 0).content.from);
    expect(press(cellView(main), "ArrowLeft")).toBe(true);
    expectOpen(main, 0, 1);
    expect(head(main)).toBe(cellAt(main, 0, 1).content.to);
  });

  test("ArrowLeft inside the content stays in the cell", () => {
    const main = solo("| ab | c |\n| - | - |\n| 1 | 2 |\n");
    open(main, 0, 0);
    press(cellView(main), "ArrowLeft");
    expect(where(main)).toEqual([0, 0]);
  });

  test("ArrowRight at the content end goes to the next cell's start", () => {
    const main = solo();
    open(main, 0, 1);
    expect(press(cellView(main), "ArrowRight")).toBe(true);
    expectOpen(main, 1, 0);
    expect(head(main)).toBe(cellAt(main, 1, 0).content.from);
  });

  test("ArrowRight before the end stays in the cell", () => {
    const main = solo();
    open(main, 0, 1, cellAt(main, 0, 1).content.from);
    press(cellView(main), "ArrowRight");
    expect(where(main)).toEqual([0, 1]);
  });

  test("after a hidden leading escape the caret is at the visible start", () => {
    const main = solo("| a | \\|x |\n| - | - |\n| 1 | 2 |\n");
    const cell = cellAt(main, 0, 1);
    expect(cell.text).toBe("\\|x");
    open(main, 0, 1, cell.content.from + 1);
    expect(press(cellView(main), "ArrowLeft")).toBe(true);
    expectOpen(main, 0, 0);
  });

  test("a selection is not at an edge", () => {
    const main = solo();
    const view = open(main, 0, 1);
    const { from, to } = cellAt(main, 0, 1).content;
    view.dispatch({ selection: { anchor: from, head: to } });
    press(view, "ArrowRight");
    expect(where(main)).toEqual([0, 1]);
  });

  test("ArrowUp and ArrowDown move in the same column", () => {
    const main = solo();
    open(main, 1, 1);
    expect(press(cellView(main), "ArrowUp")).toBe(true);
    expectOpen(main, 0, 1);
    expect(press(cellView(main), "ArrowDown")).toBe(true);
    expectOpen(main, 1, 1);
  });

  test("ArrowUp from the header leaves to the end of the line above", async () => {
    const main = solo();
    const closed: CloseReason[] = [];
    onCellClosed(main, (reason) => closed.push(reason));
    open(main, 0, 1);
    expect(press(cellView(main), "ArrowUp")).toBe(true);
    expect(main.state.field(openCellField)).toBeNull();
    expect(activeCell(main)).toBeNull();
    expect(main.state.selection.main.head).toBe(main.state.doc.line(2).to);
    expect(main.hasFocus).toBe(true);
    await flush();
    expect(closed).toEqual(["user"]);
    expect(bytes(main.state)).toBe(DOC);
  });

  test("ArrowDown from the last row leaves to the start of the line below", () => {
    const main = solo();
    open(main, 1, 0);
    expect(press(cellView(main), "ArrowDown")).toBe(true);
    expect(main.state.field(openCellField)).toBeNull();
    expect(main.state.selection.main.head).toBe(main.state.doc.line(6).from);
    expect(main.hasFocus).toBe(true);
  });

  test("ArrowDown stays when the table is the last thing in the document", () => {
    const main = solo("Intro\n\n| a | b |\n| - | - |\n| 1 | 2 |");
    open(main, 1, 0);
    expect(press(cellView(main), "ArrowDown")).toBe(true);
    expectOpen(main, 1, 0);
  });
});

describe("in a cell: Escape", () => {
  test("Escape closes the cell and focuses its element", async () => {
    const main = solo();
    open(main, 0, 1);
    expect(press(cellView(main), "Escape")).toBe(true);
    expect(main.state.field(openCellField)).toBeNull();
    expect(activeCell(main)).toBeNull();
    const el = cellEl(main, 0, 1);
    expect(document.activeElement).toBe(el);
    expect(el.tabIndex).toBe(0);
    expect(cellEl(main, 0, 0).tabIndex).toBe(-1);
    expect(el.textContent).toBe("b");
    await flush();
    expect(document.activeElement).toBe(el);
    expect(bytes(main.state)).toBe(DOC);
  });

  test("a key typed in an open cell does not reach grid navigation", () => {
    const main = solo();
    const view = open(main, 0, 0);
    const event = keyOn(view.contentDOM, "Z", { shiftKey: true, keyCode: 90 });
    // jsdom inserts nothing for a keydown; what matters is that the grid did
    // not replace the cell's content with the key.
    expect(event.defaultPrevented).toBe(false);
    expect(bytes(main.state)).toBe(DOC);
    expectOpen(main, 0, 0);
  });
});

/** Escapes into grid navigation on `row`/`column`. */
function navOn(main: EditorView, row: number, column: number): HTMLElement {
  open(main, row, column);
  press(cellView(main), "Escape");
  const el = cellEl(main, row, column);
  expect(document.activeElement).toBe(el);
  return el;
}

function focused(main: EditorView): [number, number] | null {
  const el = document.activeElement;
  if (!(el instanceof HTMLElement) || !grid(main).contains(el)) return null;
  return [
    Number(el.getAttribute("aria-rowindex")) - 1,
    Number(el.getAttribute("aria-colindex")) - 1,
  ];
}

const WIDE = "| a | b | c |\n| - | - | - |\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |\n";

// R23: the cell keys work the same in a table without outer pipes.
describe.each(PIPELESS_FIXTURES)("keys without outer pipes: $name", (f) => {
  test("Tab walks every cell, Shift+Tab walks back, the bytes stay", () => {
    const main = solo(f.text);
    open(main, 0, 0);
    const order = [
      [0, 1],
      [1, 0],
      [1, 1],
      [2, 0],
      [2, 1],
    ];
    for (const [row, column] of order) {
      expect(press(cellView(main), "Tab")).toBe(true);
      expectOpen(main, row ?? 0, column ?? 0);
    }
    for (const [row, column] of order.reverse().slice(1)) {
      expect(press(cellView(main), "Tab", { shift: true })).toBe(true);
      expectOpen(main, row ?? 0, column ?? 0);
    }
    press(cellView(main), "Tab", { shift: true });
    expectOpen(main, 0, 0);
    expect(bytes(main.state)).toBe(f.text);
  });

  test("Tab in the last cell adds a row, Mod-z takes it back", () => {
    const main = solo(f.text);
    open(main, 2, 1);
    expect(press(cellView(main), "Tab")).toBe(true);
    expectOpen(main, 3, 0);
    // A new row's cells are empty, and an empty edge cell needs its edge
    // pipe to exist: the one place R23 adds pipes a table did not have.
    expect(bytes(main.state)).toBe(withTable(f, [...f.lines, "|  |  |"]));
    expect(gridTables(main.state)[0]?.rows.map((r) => r.length)).toEqual([
      2, 2, 2, 2,
    ]);
    const editor = activeCell(main);
    if (!editor) throw new Error("no cell editor mounted");
    typeIn(editor, "5");
    expect(bytes(main.state)).toBe(withTable(f, [...f.lines, "| 5 |  |"]));
    press(cellView(main), "z", { mod: true });
    press(cellView(main), "z", { mod: true });
    expect(bytes(main.state)).toBe(f.text);
  });

  test("Enter moves down, and in the last row adds a row in the same column", () => {
    const main = solo(f.text);
    open(main, 0, 1);
    expect(press(cellView(main), "Enter")).toBe(true);
    expectOpen(main, 1, 1);
    press(cellView(main), "Enter");
    expectOpen(main, 2, 1);
    expect(bytes(main.state)).toBe(f.text);
    press(cellView(main), "Enter");
    expectOpen(main, 3, 1);
    expect(bytes(main.state)).toBe(withTable(f, [...f.lines, "|  |  |"]));
  });

  test("arrows cross the cell edges and the rows", () => {
    const main = solo(f.text);
    open(main, 1, 0);
    expect(press(cellView(main), "ArrowRight")).toBe(true);
    expectOpen(main, 1, 1);
    expect(head(main)).toBe(cellAt(main, 1, 1).content.from);
    expect(press(cellView(main), "ArrowLeft")).toBe(true);
    expectOpen(main, 1, 0);
    expect(head(main)).toBe(cellAt(main, 1, 0).content.to);
    expect(press(cellView(main), "ArrowDown")).toBe(true);
    expectOpen(main, 2, 0);
    expect(press(cellView(main), "ArrowUp")).toBe(true);
    expectOpen(main, 1, 0);
    press(cellView(main), "ArrowUp");
    expectOpen(main, 0, 0);
    expect(bytes(main.state)).toBe(f.text);
  });

  test("Escape closes the cell onto its element, and Escape again leaves the table", async () => {
    const main = solo(f.text);
    open(main, 2, 1);
    expect(press(cellView(main), "Escape")).toBe(true);
    expect(main.state.field(openCellField)).toBeNull();
    const el = cellEl(main, 2, 1);
    expect(document.activeElement).toBe(el);
    expect(el.textContent).toBe("4");
    await flush();
    keyOn(el, "Escape");
    const end = tableStart(f) + f.lines.join("\n").length;
    expect(main.state.selection.main.head).toBe(end + 1);
    expect(main.hasFocus).toBe(true);
    expect(bytes(main.state)).toBe(f.text);
  });
});

describe("grid navigation", () => {
  test("arrows move between cells and stop at the edges", () => {
    const main = solo(WIDE);
    navOn(main, 0, 0);
    keyOn(document.activeElement as HTMLElement, "ArrowRight");
    expect(focused(main)).toEqual([0, 1]);
    keyOn(document.activeElement as HTMLElement, "ArrowDown");
    expect(focused(main)).toEqual([1, 1]);
    keyOn(document.activeElement as HTMLElement, "ArrowLeft");
    expect(focused(main)).toEqual([1, 0]);
    keyOn(document.activeElement as HTMLElement, "ArrowLeft");
    expect(focused(main)).toEqual([1, 0]);
    keyOn(document.activeElement as HTMLElement, "ArrowUp");
    expect(focused(main)).toEqual([0, 0]);
    const event = keyOn(document.activeElement as HTMLElement, "ArrowUp");
    expect(event.defaultPrevented).toBe(true);
    expect(focused(main)).toEqual([0, 0]);
    expect(cellEl(main, 0, 0).tabIndex).toBe(0);
    expect(cellEl(main, 1, 0).tabIndex).toBe(-1);
    expect(main.state.field(openCellField)).toBeNull();
  });

  test("Home and End go to the row's ends, Mod-Home and Mod-End to the table's", () => {
    const main = solo(WIDE);
    navOn(main, 1, 1);
    keyOn(document.activeElement as HTMLElement, "End");
    expect(focused(main)).toEqual([1, 2]);
    keyOn(document.activeElement as HTMLElement, "Home");
    expect(focused(main)).toEqual([1, 0]);
    keyOn(document.activeElement as HTMLElement, "End", modKey);
    expect(focused(main)).toEqual([2, 2]);
    keyOn(document.activeElement as HTMLElement, "Home", modKey);
    expect(focused(main)).toEqual([0, 0]);
  });

  test("Enter opens the cell with the caret at the end", () => {
    const main = solo(WIDE);
    const el = navOn(main, 1, 1);
    const event = keyOn(el, "Enter");
    expect(event.defaultPrevented).toBe(true);
    expectOpen(main, 1, 1);
    expect(head(main)).toBe(cellAt(main, 1, 1).content.to);
    expect(bytes(main.state)).toBe(WIDE);
  });

  test("F2 opens the cell with the caret at the end", () => {
    const main = solo(WIDE);
    const el = navOn(main, 2, 0);
    keyOn(el, "F2");
    expectOpen(main, 2, 0);
    expect(head(main)).toBe(cellAt(main, 2, 0).content.to);
  });

  test("a printable character opens the cell and replaces its content", () => {
    const main = solo(WIDE);
    const el = navOn(main, 1, 1);
    const event = keyOn(el, "Z", { shiftKey: true, keyCode: 90 });
    expect(event.defaultPrevented).toBe(true);
    expectOpen(main, 1, 1);
    expect(bytes(main.state)).toBe(WIDE.replace("| 2 |", "| Z |"));
    expect(head(main)).toBe(cellAt(main, 1, 1).content.to);
  });

  test("a typed pipe replaces the content escaped", () => {
    const main = solo(WIDE);
    const el = navOn(main, 1, 1);
    keyOn(el, "|", { shiftKey: true });
    expect(bytes(main.state)).toBe(WIDE.replace("| 2 |", "| \\| |"));
    expect(gridTables(main.state)[0]?.rows[1]?.length).toBe(3);
  });

  test("a chord is not a printable character", () => {
    const main = solo(WIDE);
    const el = navOn(main, 1, 1);
    const event = keyOn(el, "c", { ...modKey, keyCode: 67 });
    expect(event.defaultPrevented).toBe(false);
    expect(main.state.field(openCellField)).toBeNull();
    expect(bytes(main.state)).toBe(WIDE);
  });

  test("Delete clears the content in one change and keeps the focus", () => {
    const main = solo(WIDE);
    const el = navOn(main, 1, 1);
    keyOn(el, "Delete");
    expect(bytes(main.state)).toBe(WIDE.replace("| 2 |", "|  |"));
    expect(main.state.field(openCellField)).toBeNull();
    expect(focused(main)).toEqual([1, 1]);
    expect(cellEl(main, 1, 1).textContent).toBe("");
    // The main view never sees a key in the grid: undo is the grid's own.
    const undo = keyOn(cellEl(main, 1, 1), "z", { ...modKey, keyCode: 90 });
    expect(undo.defaultPrevented).toBe(true);
    expect(bytes(main.state)).toBe(WIDE);
    expect(focused(main)).toEqual([1, 1]);
    expect(main.state.field(openCellField)).toBeNull();
    keyOn(cellEl(main, 1, 1), "Z", { ...modKey, shiftKey: true, keyCode: 90 });
    expect(bytes(main.state)).toBe(WIDE.replace("| 2 |", "|  |"));
    expect(focused(main)).toEqual([1, 1]);
  });

  test("Backspace clears the content too, and an empty cell is left alone", () => {
    const main = solo(WIDE);
    const el = navOn(main, 1, 0);
    keyOn(el, "Backspace");
    const cleared = WIDE.replace("| 1 |", "|  |");
    expect(bytes(main.state)).toBe(cleared);
    const again = keyOn(document.activeElement as HTMLElement, "Backspace");
    expect(again.defaultPrevented).toBe(true);
    expect(bytes(main.state)).toBe(cleared);
  });

  // R23: clearing a bare edge cell must not take the cell with it (` | 2` is
  // a leading pipe and one cell, `1 | ` a trailing pipe and one cell), so
  // the cleared cell keeps the one pipe it needs, as an empty pasted value.
  describe.each(PIPELESS_FIXTURES)(
    "clearing without outer pipes: $name",
    (f) => {
      test.each([
        ["Delete", 0, "| | 2"],
        ["Backspace", 0, "| | 2"],
        ["Delete", 1, "1 | |"],
        ["Backspace", 1, "1 | |"],
      ])("%s on cell 1/%i keeps the row's two cells", (key, column, line) => {
        const main = solo(f.text);
        const el = navOn(main, 1, column);
        keyOn(el, key);
        const lines = [...f.lines];
        lines[2] = line;
        expect(bytes(main.state)).toBe(withTable(f, lines));
        expect(
          gridTables(main.state)[0]?.rows.map((row) => row.map((c) => c.text)),
        ).toEqual([
          ["a", "b"],
          column === 0 ? ["", "2"] : ["1", ""],
          ["3", "4"],
        ]);
        expect(focused(main)).toEqual([1, column]);
        expect(main.state.field(openCellField)).toBeNull();
        keyOn(cellEl(main, 1, column), "z", { ...modKey, keyCode: 90 });
        expect(bytes(main.state)).toBe(f.text);
      });
    },
  );

  test("Escape leaves the table with the caret on the line after it", () => {
    const main = solo();
    const el = navOn(main, 0, 1);
    const event = keyOn(el, "Escape");
    expect(event.defaultPrevented).toBe(true);
    expect(main.hasFocus).toBe(true);
    expect(main.state.selection.main.head).toBe(main.state.doc.line(6).from);
    expect(main.state.field(openCellField)).toBeNull();
  });

  test("Tab is left to the browser", () => {
    const main = solo();
    const el = navOn(main, 0, 1);
    expect(keyOn(el, "Tab").defaultPrevented).toBe(false);
    expect(keyOn(el, "Tab", { shiftKey: true }).defaultPrevented).toBe(false);
  });
});

describe("from the main view", () => {
  function caret(main: EditorView, pos: number): void {
    main.dispatch({ selection: { anchor: pos } });
    main.focus();
  }

  test("ArrowDown on the line above opens the first header cell", () => {
    const main = solo();
    caret(main, main.state.doc.line(2).from);
    expect(press(main, "ArrowDown")).toBe(true);
    expectOpen(main, 0, 0);
    expect(head(main)).toBe(cellAt(main, 0, 0).content.from);
    expect(bytes(main.state)).toBe(DOC);
  });

  test("ArrowRight at the end of the line above opens the first header cell", () => {
    const main = solo("Intro\n| a | b |\n| - | - |\n| 1 | 2 |\n");
    caret(main, main.state.doc.line(1).to);
    expect(press(main, "ArrowRight")).toBe(true);
    expectOpen(main, 0, 0);
  });

  test("ArrowRight before that line's end moves the caret as always", () => {
    const main = solo("Intro\n| a | b |\n| - | - |\n| 1 | 2 |\n");
    caret(main, 2);
    expect(press(main, "ArrowRight")).toBe(true);
    expect(main.state.field(openCellField)).toBeNull();
    expect(main.state.selection.main.head).toBe(3);
  });

  test("ArrowUp on the line below opens the last row's first cell", () => {
    const main = solo();
    caret(main, main.state.doc.line(6).from);
    expect(press(main, "ArrowUp")).toBe(true);
    expectOpen(main, 1, 0);
    expect(head(main)).toBe(cellAt(main, 1, 0).content.to);
  });

  test("ArrowLeft at the start of the line below opens the last row's first cell", () => {
    const main = solo();
    caret(main, main.state.doc.line(6).from);
    expect(press(main, "ArrowLeft")).toBe(true);
    expectOpen(main, 1, 0);
  });

  test("ArrowDown elsewhere is the default move", () => {
    const main = solo();
    caret(main, 0);
    press(main, "ArrowDown");
    expect(main.state.field(openCellField)).toBeNull();
  });

  test("a selection never enters", () => {
    const main = solo("Intro\n| a | b |\n| - | - |\n| 1 | 2 |\n");
    main.dispatch({ selection: { anchor: 0, head: 5 } });
    press(main, "ArrowRight");
    expect(main.state.field(openCellField)).toBeNull();
  });
});

const READ_ONLY: Extension = [
  EditorView.editable.of(false),
  EditorState.readOnly.of(true),
];

describe("read-only (R17)", () => {
  /** A writable buffer that a compartment can turn read-only. */
  function switchable(text = DOC) {
    const lock = new Compartment();
    const main = soloMain(text, [...layers(), lock.of([])]);
    const freeze = () => {
      main.dispatch({ effects: lock.reconfigure(READ_ONLY) });
    };
    return { main, freeze };
  }

  /** Grid navigation on a read-only buffer, on cell `row`/`column`. */
  function frozenNav(row: number, column: number, text = DOC) {
    const { main, freeze } = switchable(text);
    const el = navOn(main, row, column);
    freeze();
    el.focus();
    return { main, el };
  }

  test("a mousedown opens no cell and leaves the text selectable", () => {
    const main = soloMain(DOC, [...layers(), READ_ONLY]);
    const event = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
      button: 0,
    });
    cellEl(main, 1, 0).dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(main.state.field(openCellField)).toBeNull();
    expect(activeCell(main)).toBeNull();
  });

  test("an outside setOpenCell opens nothing and hostOpenCell hosts nothing", () => {
    const main = soloMain(DOC, [...layers(), READ_ONLY]);
    const at = cellAt(main, 1, 0).content.to;
    main.dispatch({
      effects: setOpenCell.of({ anchor: at, head: at, focused: true }),
    });
    expect(main.state.field(openCellField)).toBeNull();
    expect(hostOpenCell(main, "now")).toBeNull();
    expect(activeCell(main)).toBeNull();
  });

  test("turning read-only closes an open cell to the grid", async () => {
    const { main, freeze } = switchable();
    open(main, 1, 1);
    freeze();
    expect(main.state.field(openCellField)).toBeNull();
    expect(activeCell(main)).toBeNull();
    await flush();
    expect(grid(main).contains(document.activeElement)).toBe(true);
    expect(bytes(main.state)).toBe(DOC);
  });

  test("Enter and F2 open no cell", () => {
    const { main, el } = frozenNav(1, 1);
    expect(keyOn(el, "Enter").defaultPrevented).toBe(false);
    expect(keyOn(el, "F2").defaultPrevented).toBe(false);
    expect(main.state.field(openCellField)).toBeNull();
    expect(activeCell(main)).toBeNull();
  });

  test("a printable character changes nothing", () => {
    const { main, el } = frozenNav(1, 1);
    expect(
      keyOn(el, "Z", { shiftKey: true, keyCode: 90 }).defaultPrevented,
    ).toBe(false);
    expect(main.state.field(openCellField)).toBeNull();
    expect(bytes(main.state)).toBe(DOC);
  });

  test("Delete, Backspace and Mod-z change nothing", () => {
    const { main, el } = frozenNav(1, 1);
    keyOn(el, "Delete");
    keyOn(el, "Backspace");
    keyOn(el, "z", { ...modKey, keyCode: 90 });
    expect(bytes(main.state)).toBe(DOC);
  });

  test("arrows still move between cells", () => {
    const { main, el } = frozenNav(1, 0);
    keyOn(el, "ArrowRight");
    expect(focused(main)).toEqual([1, 1]);
  });

  test("Tab in the last cell adds no row: the cell is closed first", () => {
    const { main, freeze } = switchable();
    const view = open(main, 1, 1);
    freeze();
    // The frozen buffer closed the cell; its old view runs no grid key.
    expect(press(view, "Tab")).toBe(false);
    expect(bytes(main.state)).toBe(DOC);
    expect(gridTables(main.state)[0]?.rows.length).toBe(2);
  });

  test("the main view's entry keys open nothing", () => {
    const main = soloMain(DOC, [...layers(), READ_ONLY]);
    // The key is not swallowed: the default move runs.
    main.dispatch({ selection: { anchor: main.state.doc.line(2).to } });
    press(main, "ArrowRight");
    expect(main.state.selection.main.head).not.toBe(main.state.doc.line(2).to);
    for (const [line, key] of [
      [2, "ArrowDown"],
      [6, "ArrowUp"],
      [6, "ArrowLeft"],
    ] as const) {
      main.dispatch({ selection: { anchor: main.state.doc.line(line).from } });
      press(main, key);
      expect(main.state.field(openCellField)).toBeNull();
    }
    main.dispatch({ selection: { anchor: main.state.doc.line(2).to } });
    press(main, "ArrowRight");
    expect(main.state.field(openCellField)).toBeNull();
    expect(activeCell(main)).toBeNull();
  });
});

describe("AltGr (R18a)", () => {
  test("Ctrl and Alt together with one character replace the cell", () => {
    const main = solo(WIDE);
    const el = navOn(main, 1, 1);
    const event = keyOn(el, "|", { ctrlKey: true, altKey: true });
    expect(event.defaultPrevented).toBe(true);
    expect(bytes(main.state)).toBe(WIDE.replace("| 2 |", "| \\| |"));
    expectOpen(main, 1, 1);
  });

  test("@ through AltGr replaces the cell too", () => {
    const main = solo(WIDE);
    const el = navOn(main, 1, 1);
    keyOn(el, "@", { ctrlKey: true, altKey: true });
    expect(bytes(main.state)).toBe(WIDE.replace("| 2 |", "| @ |"));
  });
});

describe("grid-nav edits are their own undo step (R18b)", () => {
  function typeEscapeDelete(main: EditorView) {
    const view = open(main, 1, 1);
    typeIn({ view, destroy: () => undefined }, "x");
    press(view, "Escape");
    const el = cellEl(main, 1, 1);
    expect(document.activeElement).toBe(el);
    keyOn(el, "Delete");
    expect(bytes(main.state)).toBe(DOC.replace("| 2 |", "|  |"));
    keyOn(el, "z", { ...modKey, keyCode: 90 });
    expect(bytes(main.state)).toBe(DOC.replace("| 2 |", "| 2x |"));
  }

  test("solo: Mod-z after type, Escape, Delete restores only the cleared text", () => {
    typeEscapeDelete(solo());
  });

  test("room: Mod-z after type, Escape, Delete restores only the cleared text", () => {
    const { main } = roomMain(DOC, layers());
    typeEscapeDelete(main);
  });

  test("room: a printable replacement is its own undo step", () => {
    const { main } = roomMain(DOC, layers());
    const view = open(main, 1, 1);
    typeIn({ view, destroy: () => undefined }, "x");
    press(view, "Escape");
    keyOn(cellEl(main, 1, 1), "Q", { shiftKey: true, keyCode: 81 });
    expect(bytes(main.state)).toBe(DOC.replace("| 2 |", "| Q |"));
    press(cellView(main), "z", { mod: true });
    expect(bytes(main.state)).toBe(DOC.replace("| 2 |", "| 2x |"));
  });
});
