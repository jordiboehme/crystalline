/**
 * The cell editor: one table cell edited in a second view that holds the
 * whole main document, with edits clamped to the cell, pipes escaped and undo
 * run on the main view's own stack. Every probe runs solo (CodeMirror
 * history) and in a room (Y.UndoManager).
 */

import { EditorState, StateEffect } from "@codemirror/state";
import { EditorView, ViewPlugin } from "@codemirror/view";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import {
  bytes,
  cellOf,
  cleanupCells,
  openAt,
  openText,
  press,
  roomMain,
  soloMain,
  typeIn,
} from "../../test/cellKit";
import type { CellEditor } from "./cellEditor";
import {
  activeCell,
  cellForwarding,
  fromCell,
  mountCellEditor,
  runMainUndo,
} from "./cellEditor";
import { baseExtensions } from "../setup";
import { parsedState } from "../../test/parse";
import { gridTables } from "./structure";
import { closeCell, onCellClosed, openCellField } from "./openCell";

const DOC = "| a | b |\n| - | - |\n| 1 | 2 |\n";

let errors: MockInstance<typeof console.error>;

beforeEach(() => {
  errors = vi.spyOn(console, "error");
});

afterEach(() => {
  cleanupCells();
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

const setups = {
  solo: (text: string) => soloMain(text, [cellForwarding]),
  room: (text: string) => roomMain(text).main,
};

describe.each(Object.entries(setups))("cell editor (%s)", (_, make) => {
  /** A main view on `text` with the cell at row/column open, caret at its end. */
  function open(text = DOC, row = 1, column = 0) {
    const main = make(text);
    const cell = openAt(main, cellOf(main, row, column).content.to);
    return { main, cell };
  }

  test("typing reaches the main document inside the cell", () => {
    const { main, cell } = open();
    typeIn(cell, "x");
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1x | 2 |\n");
    expect(cell.view.state.doc.eq(main.state.doc)).toBe(true);
    expect(openText(main)).toBe("1x");
  });

  test("an insert at position 0 through the cell is dropped", () => {
    const { main, cell } = open();
    cell.view.dispatch({ changes: { from: 0, insert: "y" } });
    expect(bytes(main.state)).toBe(DOC);
    expect(cell.view.state.doc.eq(main.state.doc)).toBe(true);
  });

  test("a typed line break changes nothing", () => {
    const { main, cell } = open();
    typeIn(cell, "\n");
    expect(bytes(main.state)).toBe(DOC);
    press(cell.view, "Enter");
    expect(bytes(main.state)).toBe(DOC);
  });

  test("a typed pipe is stored escaped and shown bare", () => {
    const { main, cell } = open("| a | b |\n| - | - |\n|  | 2 |\n");
    typeIn(cell, "a|b");
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| a\\|b | 2 |\n");
    expect(openText(main)).toBe("a\\|b");
    expect(cell.view.contentDOM.textContent).toBe("a|b");
    // Backspace over the visible pipe takes its hidden backslash with it.
    expect(press(cell.view, "Backspace")).toBe(true);
    expect(press(cell.view, "Backspace")).toBe(true);
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| a | 2 |\n");
  });

  test("Delete in front of an escaped pipe removes the pair", () => {
    const { main, cell } = open("| a | b |\n| - | - |\n| x\\|y | 2 |\n");
    const content = cellOf(main, 1, 0).content;
    cell.view.dispatch({ selection: { anchor: content.from + 1 } });
    expect(press(cell.view, "Delete")).toBe(true);
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| xy | 2 |\n");
  });

  test("Shift+Enter stores a line break tag", () => {
    const { main, cell } = open();
    expect(press(cell.view, "Enter", { shift: true })).toBe(true);
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1<br> | 2 |\n");
    expect(
      cell.view.contentDOM.querySelector(".cm-cell-break br"),
    ).not.toBeNull();
    expect(cell.view.contentDOM.textContent).toBe("1");
  });

  test("Mod-z from the cell restores the previous bytes", () => {
    const { main, cell } = open();
    typeIn(cell, "x");
    expect(press(cell.view, "z", { mod: true })).toBe(true);
    expect(bytes(main.state)).toBe(DOC);
    expect(cell.view.state.doc.eq(main.state.doc)).toBe(true);
    expect(openText(main)).toBe("1");
    expect(press(cell.view, "z", { mod: true, shift: true })).toBe(true);
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1x | 2 |\n");
    expect(press(cell.view, "z", { mod: true })).toBe(true);
    expect(press(cell.view, "y", { mod: true })).toBe(true);
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1x | 2 |\n");
  });

  test("three quick keystrokes are one undo step", () => {
    const { main, cell } = open();
    typeIn(cell, "x");
    typeIn(cell, "y");
    typeIn(cell, "z");
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1xyz | 2 |\n");
    expect(press(cell.view, "z", { mod: true })).toBe(true);
    expect(bytes(main.state)).toBe(DOC);
  });

  test.each([
    ["|  |", "| a |\n| - |\n|  |\n", "| a |\n| - |\n| q |\n"],
    ["||", "| a |\n| - |\n||\n", "| a |\n| - |\n|q|\n"],
  ])("an empty %s cell takes typing at the right offset", (_, text, want) => {
    const { main, cell } = open(text, 1, 0);
    typeIn(cell, "q");
    expect(bytes(main.state)).toBe(want);
    expect(openText(main)).toBe("q");
  });

  test("a backslash typed at the end of an unpadded cell is doubled", () => {
    const { main, cell } = open("|A|B|\n|-|-|\n|1|2|\n");
    typeIn(cell, "\\");
    expect(bytes(main.state)).toBe("|A|B|\n|-|-|\n|1\\\\|2|\n");
    expect(gridTables(main.state)[0]?.rows[1]?.length).toBe(2);
    expect(cell.view.contentDOM.textContent).toBe("1\\");
    // Backspace over the visible backslash removes the pair.
    expect(press(cell.view, "Backspace")).toBe(true);
    expect(bytes(main.state)).toBe("|A|B|\n|-|-|\n|1|2|\n");
  });

  test("a deletion that exposes a backslash before the pipe doubles it", () => {
    const { main, cell } = open("|A|B|\n|-|-|\n|a\\b|2|\n");
    expect(press(cell.view, "Backspace")).toBe(true);
    expect(bytes(main.state)).toBe("|A|B|\n|-|-|\n|a\\\\|2|\n");
    expect(gridTables(main.state)[0]?.rows[1]?.length).toBe(2);
    expect(cell.view.contentDOM.textContent).toBe("a\\");
  });

  test("a second mount destroys the first", () => {
    const { main, cell } = open();
    expect(activeCell(main)).toBe(cell);
    const second = openAt(main, cellOf(main, 1, 1).content.to);
    expect(activeCell(main)).toBe(second);
    expect(cell.view.dom.isConnected).toBe(false);
    typeIn(second, "w");
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1 | 2w |\n");
  });

  test("the open cell follows the cell's caret", () => {
    const { main, cell } = open();
    typeIn(cell, "x");
    const content = cellOf(main, 1, 0).content;
    expect(main.state.field(openCellField)).toMatchObject({
      anchor: content.to,
      head: content.to,
    });
    cell.view.dispatch({ selection: { anchor: content.from } });
    expect(main.state.field(openCellField)?.head).toBe(content.from);
  });

  test("closeCell tears the editor down and tells the listeners once", async () => {
    const { main, cell } = open();
    const heard = vi.fn();
    onCellClosed(main, heard);
    closeCell(main, "user", "main");
    await Promise.resolve();
    expect(activeCell(main)).toBeNull();
    expect(cell.view.dom.isConnected).toBe(false);
    expect(main.state.field(openCellField)).toBeNull();
    expect(heard).toHaveBeenCalledTimes(1);
    expect(heard).toHaveBeenCalledWith("user");
    expect(document.activeElement).toBe(main.contentDOM);
    closeCell(main, "user", "main");
    expect(heard).toHaveBeenCalledTimes(1);
  });

  test("main view destroy closes the cell with reason destroy", () => {
    const { main, cell } = open();
    const heard = vi.fn();
    onCellClosed(main, heard);
    main.destroy();
    expect(cell.view.dom.isConnected).toBe(false);
    expect(heard).toHaveBeenCalledWith("destroy");
  });
});

describe("cell editor resync", () => {
  test("a main filter that rejects a forwarded change resyncs the cell", async () => {
    const refuseZ = EditorState.transactionFilter.of((tr) => {
      if (!tr.annotation(fromCell)) return tr;
      let z = false;
      tr.changes.iterChanges((_a, _b, _c, _d, text) => {
        if (text.toString().includes("Z")) z = true;
      });
      return z ? [] : tr;
    });
    const main = soloMain(DOC, [cellForwarding, refuseZ]);
    const cell = openAt(main, cellOf(main, 1, 0).content.to);
    typeIn(cell, "Z");
    await Promise.resolve();
    expect(bytes(main.state)).toBe(DOC);
    expect(cell.view.state.doc.eq(main.state.doc)).toBe(true);
    typeIn(cell, "x");
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1x | 2 |\n");
    expect(cell.view.state.doc.eq(main.state.doc)).toBe(true);
  });
});

describe("closeCell while its dispatch waits", () => {
  const poke = StateEffect.define<"twice" | "reopen">();

  function withProbe() {
    const host = document.createElement("div");
    document.body.appendChild(host);
    let reopened: CellEditor | null = null;
    const probe = ViewPlugin.fromClass(
      class {
        update(update: {
          view: EditorView;
          transactions: readonly { effects: readonly StateEffect<unknown>[] }[];
        }) {
          for (const tr of update.transactions) {
            for (const effect of tr.effects) {
              if (!effect.is(poke)) continue;
              closeCell(update.view, "user", "none");
              if (effect.value === "twice")
                closeCell(update.view, "user", "none");
              else reopened = mountCellEditor(update.view, host);
            }
          }
        }
      },
    );
    const main = soloMain(DOC, [cellForwarding, probe]);
    const cell = openAt(main, cellOf(main, 1, 0).content.to);
    const heard = vi.fn();
    onCellClosed(main, heard);
    return { main, cell, heard, reopened: () => reopened, host };
  }

  test("two closes inside one update notify once", async () => {
    const { main, heard } = withProbe();
    main.dispatch({ effects: poke.of("twice") });
    await Promise.resolve();
    await Promise.resolve();
    expect(heard).toHaveBeenCalledTimes(1);
    expect(main.state.field(openCellField)).toBeNull();
  });

  test("open, close and open again within one update keeps the new cell", async () => {
    const { main, cell, heard, reopened, host } = withProbe();
    main.dispatch({ effects: poke.of("reopen") });
    await Promise.resolve();
    await Promise.resolve();
    const fresh = reopened();
    expect(fresh).not.toBeNull();
    expect(activeCell(main)).toBe(fresh);
    expect(cell.view.dom.isConnected).toBe(false);
    expect(main.state.field(openCellField)).not.toBeNull();
    expect(heard).toHaveBeenCalledTimes(1);
    if (fresh) typeIn(fresh, "w");
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1w | 2 |\n");
    fresh?.destroy();
    host.remove();
  });
});

describe("undo with no stack", () => {
  test("Mod-z with neither history nor a room returns false", () => {
    const main = new EditorView({
      state: parsedState(
        EditorState.create({
          doc: DOC,
          extensions: [
            EditorState.lineSeparator.of("\n"),
            ...baseExtensions(false, { history: false }),
            cellForwarding,
          ],
        }),
      ),
      parent: document.body,
    });
    const cell = openAt(main, cellOf(main, 1, 0).content.to);
    typeIn(cell, "x");
    expect(runMainUndo(main, "undo")).toBe(false);
    expect(runMainUndo(main, "redo")).toBe(false);
    // The key is still taken (preventDefault), so the browser's own undo
    // never runs on the cell's content; it changes nothing and never throws.
    expect(() => press(cell.view, "z", { mod: true })).not.toThrow();
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1x | 2 |\n");
    main.destroy();
  });
});
