/**
 * A Cell actions verb that refuses (its `run` returns false) after the open
 * cell was closed for it (final review finding 11): the cell opens again
 * where it was, with the focus, and the document stays as it was. The verb
 * is made to refuse with a module mock, because every real refusal is
 * already ruled out by the menu's own `applies`.
 */

import type { Extension } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, test, vi } from "vitest";

import { bytes, cellOf, cleanupCells, soloMain } from "../../test/cellKit";
import { fencePreviews } from "../fencePreviews";
import { activeCell } from "./cellEditor";
import { runCellAction } from "./gridChrome";
import { hostOpenCell } from "./gridWidget";
import { gridExtension } from "./index";
import { openCellRange, setOpenCell } from "./openCell";

vi.mock("../tableVerbs", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../tableVerbs")>();
  return {
    ...actual,
    tableDeleteRow: () => false,
    tableAlignColumn: () => false,
  };
});

const DOC = "Intro\n\n| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |\n\nOutro\n";

afterEach(() => {
  cleanupCells();
});

function layers(): Extension[] {
  return [gridExtension(), fencePreviews(false)];
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

function key(el: Element, k: string): void {
  el.dispatchEvent(
    new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }),
  );
}

function item(label: string): HTMLElement {
  const found = [
    ...document.querySelectorAll<HTMLElement>('[role^="menuitem"]'),
  ].find((el) => el.textContent === label);
  if (!found) throw new Error(`no menu item ${label}`);
  return found;
}

describe("a refused verb", () => {
  test("from the menu, the closed cell opens again with the focus", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 2, 1);
    key(view.contentDOM, "ContextMenu");
    item("Delete row").click();
    expect(bytes(main.state)).toBe(DOC);
    const cell = openCellRange(main.state);
    expect(cell && [cell.row, cell.column]).toEqual([2, 1]);
    const editor = activeCell(main);
    expect(editor?.view.contentDOM.contains(document.activeElement)).toBe(true);
  });

  test("from the floating bar's path, the same", () => {
    const main = soloMain(DOC, layers());
    open(main, 1, 0);
    expect(runCellAction(main, "Delete row")).toBe(true);
    expect(bytes(main.state)).toBe(DOC);
    const editor = activeCell(main);
    expect(editor?.view.contentDOM.contains(document.activeElement)).toBe(true);
  });

  test("an alignment that refuses gives the focus back to the cell", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 1);
    key(view.contentDOM, "ContextMenu");
    item("Align right").click();
    expect(bytes(main.state)).toBe(DOC);
    expect(view.contentDOM.contains(document.activeElement)).toBe(true);
  });
});
