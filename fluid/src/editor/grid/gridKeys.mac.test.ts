/**
 * The grid's keys on a Mac (R111). CodeMirror and `platform.ts` read
 * `navigator.platform` once, when they load, so it is set before any import.
 *
 * A MacBook has no Home and End, so Cmd+Up and Cmd+Down go to the row's ends as
 * they do. Cmd is the grid's modifier for undo and the table's ends, and
 * Ctrl+Return opens Cell actions, as Shift+F10 does.
 */

import { afterEach, describe, expect, test, vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "MacIntel",
    configurable: true,
  });
});

import type { EditorView } from "@codemirror/view";

import { bytes, cleanupCells, press, soloMain } from "../../test/cellKit";
import { isMac } from "../../platform";
import { fencePreviews } from "../fencePreviews";
import { activeCell } from "./cellEditor";
import { hostOpenCell } from "./gridWidget";
import { gridExtension } from "./index";
import { openCellField, setOpenCell } from "./openCell";
import { gridTables } from "./structure";
import { parsedState } from "../../test/parse";

const WIDE = "| a | b | c |\n| - | - | - |\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |\n";

afterEach(() => {
  cleanupCells();
});

function solo(text = WIDE): EditorView {
  return soloMain(text, [gridExtension(), fencePreviews(false)]);
}

function cellEl(main: EditorView, row: number, column: number): HTMLElement {
  const el = main.dom.querySelector<HTMLElement>(
    `[aria-rowindex="${String(row + 1)}"][aria-colindex="${String(column + 1)}"]`,
  );
  if (!el) throw new Error(`no cell ${String(row)}/${String(column)}`);
  return el;
}

function open(main: EditorView, row: number, column: number): EditorView {
  const cell = gridTables(parsedState(main.state))[0]?.rows[row]?.[column];
  if (!cell) throw new Error(`no cell ${String(row)}/${String(column)}`);
  const pos = cell.content.to;
  main.dispatch({
    effects: setOpenCell.of({ anchor: pos, head: pos, focused: true }),
  });
  const editor = hostOpenCell(main, "now");
  if (!editor) throw new Error("not hosted");
  return editor.view;
}

/** The focus on the cell element itself, with no cell open. */
function navOn(main: EditorView, row: number, column: number): HTMLElement {
  open(main, row, column);
  const cell = activeCell(main);
  if (!cell) throw new Error("no cell editor mounted");
  press(cell.view, "Escape");
  const el = cellEl(main, row, column);
  expect(document.activeElement).toBe(el);
  return el;
}

function focused(main: EditorView): [number, number] | null {
  const el = document.activeElement;
  if (!(el instanceof HTMLElement) || !main.dom.contains(el)) return null;
  return [
    Number(el.getAttribute("aria-rowindex")) - 1,
    Number(el.getAttribute("aria-colindex")) - 1,
  ];
}

/** A keydown on whatever holds the focus, shaped like a browser's. */
function keyOn(key: string, init: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key,
    bubbles: true,
    cancelable: true,
    ...init,
  });
  (document.activeElement as HTMLElement).dispatchEvent(event);
  return event;
}

const cellMenu = () =>
  document.querySelector<HTMLElement>(
    '[role="menu"][aria-label="Cell actions"]',
  );

test("runs on a Mac", () => {
  expect(isMac()).toBe(true);
});

describe("the row's ends and the table's", () => {
  test("Cmd+Down goes to the row's end and Cmd+Up to its start", () => {
    const main = solo();
    navOn(main, 1, 1);
    expect(keyOn("ArrowDown", { metaKey: true }).defaultPrevented).toBe(true);
    expect(focused(main)).toEqual([1, 2]);
    expect(keyOn("ArrowUp", { metaKey: true }).defaultPrevented).toBe(true);
    expect(focused(main)).toEqual([1, 0]);
    expect(bytes(main.state)).toBe(WIDE);
  });

  test("Home and End still go to the row's ends", () => {
    const main = solo();
    navOn(main, 1, 1);
    keyOn("End");
    expect(focused(main)).toEqual([1, 2]);
    keyOn("Home");
    expect(focused(main)).toEqual([1, 0]);
  });

  test("Cmd+End and Cmd+Home go to the table's ends, Ctrl+End not", () => {
    const main = solo();
    navOn(main, 1, 1);
    keyOn("End", { metaKey: true });
    expect(focused(main)).toEqual([2, 2]);
    keyOn("Home", { metaKey: true });
    expect(focused(main)).toEqual([0, 0]);
    keyOn("End", { ctrlKey: true });
    expect(focused(main)).toEqual([0, 2]);
  });

  test("plain arrows and Shift or Option with them still move one cell", () => {
    const main = solo();
    navOn(main, 1, 1);
    keyOn("ArrowDown");
    expect(focused(main)).toEqual([2, 1]);
    keyOn("ArrowUp", { metaKey: true, shiftKey: true });
    expect(focused(main)).toEqual([1, 1]);
  });
});

describe("undo on the grid", () => {
  test("Cmd+Z undoes a cleared cell and Cmd+Shift+Z redoes it; Ctrl+Z does nothing", () => {
    const main = solo();
    navOn(main, 1, 1);
    keyOn("Delete");
    const cleared = WIDE.replace("| 2 |", "|  |");
    expect(bytes(main.state)).toBe(cleared);
    expect(keyOn("z", { ctrlKey: true, keyCode: 90 }).defaultPrevented).toBe(
      false,
    );
    expect(bytes(main.state)).toBe(cleared);
    expect(keyOn("z", { metaKey: true, keyCode: 90 }).defaultPrevented).toBe(
      true,
    );
    expect(bytes(main.state)).toBe(WIDE);
    keyOn("Z", { metaKey: true, shiftKey: true, keyCode: 90 });
    expect(bytes(main.state)).toBe(cleared);
    expect(main.state.field(openCellField)).toBeNull();
  });
});

describe("Cell actions on Ctrl+Return", () => {
  test("opens on a focused cell with the focus on its first item", () => {
    const main = solo();
    const el = navOn(main, 1, 1);
    expect(keyOn("Enter", { ctrlKey: true }).defaultPrevented).toBe(true);
    expect(cellMenu()).not.toBeNull();
    expect(cellMenu()?.contains(document.activeElement)).toBe(true);
    expect(main.state.field(openCellField)).toBeNull();
    keyOn("Escape");
    expect(cellMenu()).toBeNull();
    expect(document.activeElement).toBe(el);
    expect(bytes(main.state)).toBe(WIDE);
  });

  test("opens from an open cell, and writes no line break", () => {
    const main = solo();
    const view = open(main, 1, 1);
    view.contentDOM.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      }),
    );
    expect(cellMenu()).not.toBeNull();
    expect(bytes(main.state)).toBe(WIDE);
  });

  test("Return alone still opens the cell", () => {
    const main = solo();
    navOn(main, 1, 1);
    keyOn("Enter");
    expect(cellMenu()).toBeNull();
    expect(main.state.field(openCellField)).not.toBeNull();
  });
});
