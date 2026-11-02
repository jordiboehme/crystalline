/**
 * The cell editor under co-editing: two Y.Docs stand in for two peers, kept
 * in step by forwarding every update to the other doc with a `remote` origin
 * (which the local Y.UndoManager does not track). The open cell is re-read
 * from the parsed table after every remote change, so a change elsewhere in
 * the table never throws the local user out of a cell that still exists.
 */

import * as Y from "yjs";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import {
  cellOf,
  cleanupCells,
  openAt,
  openText,
  press,
  roomMain,
  typeIn,
} from "../../test/cellKit";
import { activeCell } from "./cellEditor";
import { gridTables } from "./structure";
import { closeCell, onCellClosed, openCellField } from "./openCell";

const DOC = "| a | b |\n| - | - |\n| 1 | 2 |\n";
const TWO_ROWS = "| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |\n";

let errors: MockInstance<typeof console.error>;

beforeEach(() => {
  errors = vi.spyOn(console, "error");
});

afterEach(() => {
  cleanupCells();
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

describe("cell editor under co-editing", () => {
  test("a remote insert inside the focused cell keeps the caret at its end", () => {
    const { main, rtext } = roomMain(DOC);
    const cell = openAt(main, cellOf(main, 1, 0).content.to);
    typeIn(cell, "x");
    rtext.insert(cellOf(main, 1, 0).content.from, "Q");
    expect(openText(main)).toBe("Q1x");
    const content = cellOf(main, 1, 0).content;
    expect(cell.view.state.selection.main.head).toBe(content.to);
    expect(main.state.field(openCellField)?.head).toBe(content.to);
    expect(cell.view.state.doc.eq(main.state.doc)).toBe(true);
  });

  test("undo from the cell leaves the remote edit", () => {
    const { main, ytext, rtext } = roomMain(DOC);
    const cell = openAt(main, cellOf(main, 1, 0).content.to);
    typeIn(cell, "xy");
    rtext.insert(cellOf(main, 1, 1).content.from, "R");
    expect(ytext.toJSON()).toBe("| a | b |\n| - | - |\n| 1xy | R2 |\n");
    expect(press(cell.view, "z", { mod: true })).toBe(true);
    expect(ytext.toJSON()).toBe("| a | b |\n| - | - |\n| 1 | R2 |\n");
    expect(rtext.toJSON()).toBe(ytext.toJSON());
    expect(openText(main)).toBe("1");
    expect(cell.view.state.doc.eq(main.state.doc)).toBe(true);
  });

  test("a remote delete of the focused row closes the cell once as gone", async () => {
    const { main, rtext } = roomMain(DOC);
    const cell = openAt(main, cellOf(main, 1, 0).content.to);
    const heard = vi.fn();
    onCellClosed(main, heard);
    const row = rtext.toJSON().indexOf("| 1 |");
    rtext.delete(row, "| 1 | 2 |\n".length);
    await Promise.resolve();
    expect(heard).toHaveBeenCalledTimes(1);
    expect(heard).toHaveBeenCalledWith("gone");
    expect(activeCell(main)).toBeNull();
    expect(cell.view.dom.isConnected).toBe(false);
    expect(main.state.field(openCellField)).toBeNull();
    expect(document.activeElement).toBe(main.contentDOM);
  });

  test("a remote delete of the whole row above leaves the cell open", async () => {
    const { main, rtext } = roomMain(TWO_ROWS);
    const cell = openAt(main, cellOf(main, 2, 0).content.to);
    typeIn(cell, "x");
    const heard = vi.fn();
    onCellClosed(main, heard);
    const row = rtext.toJSON().indexOf("| 1 |");
    rtext.delete(row, "| 1 | 2 |\n".length);
    await Promise.resolve();
    expect(heard).not.toHaveBeenCalled();
    expect(activeCell(main)).toBe(cell);
    expect(openText(main)).toBe("3x");
    expect(cell.view.state.doc.eq(main.state.doc)).toBe(true);
    typeIn(cell, "y");
    expect(rtext.toJSON()).toBe("| a | b |\n| - | - |\n| 3xy | 4 |\n");
  });

  test("a remote escaped pipe inside the cell leaves it open", async () => {
    const { main, rtext } = roomMain(DOC);
    const cell = openAt(main, cellOf(main, 1, 0).content.to);
    const heard = vi.fn();
    onCellClosed(main, heard);
    rtext.insert(cellOf(main, 1, 0).content.from, "a\\|");
    await Promise.resolve();
    expect(heard).not.toHaveBeenCalled();
    expect(activeCell(main)).toBe(cell);
    expect(openText(main)).toBe("a\\|1");
  });

  test("a remote bare pipe that splits the cell keeps the caret's half", async () => {
    const { main, rtext } = roomMain(DOC);
    const cell = openAt(main, cellOf(main, 1, 0).content.to);
    typeIn(cell, "xy");
    const heard = vi.fn();
    onCellClosed(main, heard);
    // "| 1xy |" becomes "| 1|xy |": the caret sits after "xy".
    rtext.insert(cellOf(main, 1, 0).content.from + 1, "|");
    await Promise.resolve();
    expect(heard).not.toHaveBeenCalled();
    expect(activeCell(main)).toBe(cell);
    expect(openText(main)).toBe("xy");
    typeIn(cell, "z");
    expect(rtext.toJSON()).toBe("| a | b |\n| - | - |\n| 1|xyz | 2 |\n");
  });

  test("after a gone close with focus to main the peer sees no cursor in the table", async () => {
    const { main, rtext, remoteAwareness, local } = roomMain(DOC);
    openAt(main, cellOf(main, 1, 0).content.to);
    rtext.delete(rtext.toJSON().indexOf("| 1 |"), "| 1 | 2 |\n".length);
    await Promise.resolve();
    expect(document.activeElement).toBe(main.contentDOM);
    // Any later update of main publishes its caret while it has focus.
    main.dispatch({ selection: main.state.selection });
    const table = gridTables(main.state)[0];
    if (!table) throw new Error("the header table is gone");
    const cursor = (
      remoteAwareness.getStates().get(local.clientID) as
        { cursor?: { head: Y.RelativePosition } | null } | undefined
    )?.cursor;
    if (cursor) {
      const head = Y.createAbsolutePositionFromRelativePosition(
        cursor.head,
        local,
      );
      expect(head?.index).toBeGreaterThan(table.to);
    }
    expect(main.state.selection.main.head).toBeGreaterThan(table.to);
  });

  test("a remote row delete in a table without edge pipes closes the cell as gone", async () => {
    const { main, rtext } = roomMain("a | b\n--|--\n1 | 2\n3 | 4\n");
    openAt(main, cellOf(main, 1, 0).content.to);
    const heard = vi.fn();
    onCellClosed(main, heard);
    rtext.delete(rtext.toJSON().indexOf("1 | 2"), "1 | 2\n".length);
    await Promise.resolve();
    expect(heard).toHaveBeenCalledWith("gone");
    expect(activeCell(main)).toBeNull();
  });

  test("a remote row delete takes the pipeless last cell too", async () => {
    const { main, rtext } = roomMain("a | b\n--|--\n1 | 2\n3 | 4\n");
    openAt(main, cellOf(main, 1, 1).content.to);
    const heard = vi.fn();
    onCellClosed(main, heard);
    rtext.delete(rtext.toJSON().indexOf("1 | 2"), "1 | 2\n".length);
    await Promise.resolve();
    expect(heard).toHaveBeenCalledWith("gone");
  });

  test("a remote delete of the last row with the break before closes its pipeless last cell", async () => {
    const { main, rtext } = roomMain("a | b\n--|--\n0 | 9\n1 | 2");
    const cell = openAt(main, cellOf(main, 2, 1).content.to);
    const heard = vi.fn();
    onCellClosed(main, heard);
    rtext.delete(rtext.toJSON().indexOf("\n1 | 2"), "\n1 | 2".length);
    await Promise.resolve();
    expect(heard).toHaveBeenCalledWith("gone");
    expect(activeCell(main)).toBeNull();
    expect(cell.view.dom.isConnected).toBe(false);
    expect(rtext.toJSON()).toBe("a | b\n--|--\n0 | 9");
  });

  test("a remote row delete closes a pipeless last cell with trailing spaces", async () => {
    const { main, rtext } = roomMain("a | b\n--|--\n1 | 2  \n3 | 4\n");
    openAt(main, cellOf(main, 1, 1).content.to);
    const heard = vi.fn();
    onCellClosed(main, heard);
    rtext.delete(rtext.toJSON().indexOf("1 | 2"), "1 | 2  \n".length);
    await Promise.resolve();
    expect(heard).toHaveBeenCalledWith("gone");
    expect(activeCell(main)).toBeNull();
  });

  test("a peer deleting text in a pipeless last cell leaves it open", async () => {
    // Emptying it fully is another matter: "1 | " ends in a trailing pipe,
    // so the parse has no last cell any more (see the report).
    const { main, rtext } = roomMain("a | b\n--|--\n0 | 9\n1 | 23");
    const cell = openAt(main, cellOf(main, 2, 1).content.to);
    const heard = vi.fn();
    onCellClosed(main, heard);
    rtext.delete(cellOf(main, 2, 1).content.from, 1);
    await Promise.resolve();
    expect(heard).not.toHaveBeenCalled();
    expect(activeCell(main)).toBe(cell);
    expect(openText(main)).toBe("3");
    typeIn(cell, "q");
    expect(rtext.toJSON()).toBe("a | b\n--|--\n0 | 9\n1 | 3q");
  });

  test("a peer clearing a compact cell leaves it open", async () => {
    const { main, rtext } = roomMain("|A|B|\n|-|-|\n|1|2|\n");
    const cell = openAt(main, cellOf(main, 1, 0).content.to);
    const heard = vi.fn();
    onCellClosed(main, heard);
    rtext.delete(cellOf(main, 1, 0).content.from, 1);
    await Promise.resolve();
    expect(heard).not.toHaveBeenCalled();
    expect(activeCell(main)).toBe(cell);
    expect(openText(main)).toBe("");
    typeIn(cell, "q");
    expect(rtext.toJSON()).toBe("|A|B|\n|-|-|\n|q|2|\n");
  });

  test("closeCell clears the local cursor the peer sees", () => {
    const { main, ytext, awareness, remoteAwareness, local } = roomMain(DOC);
    openAt(main, cellOf(main, 1, 0).content.to);
    // What Task 6's publishing does while the cell has focus.
    const at = cellOf(main, 1, 0).content.to;
    awareness.setLocalStateField("cursor", {
      anchor: Y.createRelativePositionFromTypeIndex(ytext, at),
      head: Y.createRelativePositionFromTypeIndex(ytext, at),
    });
    const seen = () =>
      (
        remoteAwareness.getStates().get(local.clientID) as
          { cursor?: unknown } | undefined
      )?.cursor;
    expect(seen()).toBeTruthy();
    closeCell(main, "user", "none");
    expect(seen() ?? null).toBeNull();
  });
});
