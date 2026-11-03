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
import { PIPELESS_FIXTURES } from "../../test/pipelessTables";
import { activeCell } from "./cellEditor";
import { gridTables } from "./structure";
import {
  closeCell,
  onCellClosed,
  openCellField,
  openCellRange,
} from "./openCell";

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

  test("a peer emptying a pipeless last cell closes it as gone (R10)", async () => {
    const { main, rtext } = roomMain("a | b\n--|--\n0 | 9\n1 | 23");
    openAt(main, cellOf(main, 2, 1).content.to);
    const heard = vi.fn();
    onCellClosed(main, heard);
    rtext.delete(cellOf(main, 2, 1).content.from, 2);
    await Promise.resolve();
    expect(heard).toHaveBeenCalledWith("gone");
    expect(activeCell(main)).toBeNull();
  });

  test("a peer deleting only the line break after a pipeless last cell keeps it open", async () => {
    // Pinned on purpose: the content stays, the next line joins the row, and
    // the parsed table still has the cell.
    const { main, rtext } = roomMain("a | b\n--|--\n1 | 23\n | 4\n");
    const cell = openAt(main, cellOf(main, 1, 1).content.to);
    const heard = vi.fn();
    onCellClosed(main, heard);
    rtext.delete(rtext.toJSON().indexOf("23\n") + 2, 1);
    await Promise.resolve();
    expect(rtext.toJSON()).toBe("a | b\n--|--\n1 | 23 | 4\n");
    expect(heard).not.toHaveBeenCalled();
    expect(activeCell(main)).toBe(cell);
    expect(openText(main)).toBe("23");
  });

  // R23 with R7 and R10 on the two fixtures: a peer's row delete closes an
  // open cell of that row as gone, emptying a last cell without a trailing
  // pipe closes it too (the parse no longer has it), and a partial delete or
  // a last cell with its trailing pipe stays open.
  describe.each(PIPELESS_FIXTURES)("without outer pipes: $name", (f) => {
    test.each([
      [1, 0],
      [1, 1],
      [2, 0],
      [2, 1],
    ])(
      "a peer deleting the row closes cell %i/%i as gone (R7)",
      async (row, column) => {
        const { main, rtext } = roomMain(f.text);
        openAt(main, cellOf(main, row, column).content.to);
        const heard = vi.fn();
        onCellClosed(main, heard);
        const line = `${f.lines[row + 1] ?? ""}\n`;
        rtext.delete(rtext.toJSON().indexOf(line), line.length);
        await Promise.resolve();
        expect(heard).toHaveBeenCalledTimes(1);
        expect(heard).toHaveBeenCalledWith("gone");
        expect(activeCell(main)).toBeNull();
        expect(main.state.field(openCellField)).toBeNull();
      },
    );

    test.each([1, 2])(
      "a peer emptying the last cell of row %i follows R10",
      async (row) => {
        const { main, rtext } = roomMain(f.text);
        const cell = openAt(main, cellOf(main, row, 1).content.to);
        const heard = vi.fn();
        onCellClosed(main, heard);
        const { content } = cellOf(main, row, 1);
        rtext.delete(content.from, content.to - content.from);
        await Promise.resolve();
        if ((f.lines[row + 1] ?? "").endsWith("|")) {
          // The trailing pipe keeps an empty last cell a cell.
          expect(heard).not.toHaveBeenCalled();
          expect(activeCell(main)).toBe(cell);
          expect(openText(main)).toBe("");
        } else {
          expect(heard).toHaveBeenCalledWith("gone");
          expect(activeCell(main)).toBeNull();
        }
      },
    );

    test.each([
      [0, "|  | 2"],
      [1, "1 |  |"],
    ])(
      "clearing bare cell 1/%i reaches the peer as one change with its pipe (R24)",
      async (column, line) => {
        const { main, rtext } = roomMain(f.text);
        const cell = openAt(main, cellOf(main, 1, column).content.to);
        let changes = 0;
        rtext.observe(() => {
          changes += 1;
        });
        press(cell.view, "Backspace");
        await Promise.resolve();
        const lines = [...f.lines];
        lines[2] = line;
        expect(changes).toBe(1);
        expect(rtext.toJSON()).toBe(
          f.text.replace(f.lines.join("\n"), lines.join("\n")),
        );
        expect(activeCell(main)).toBe(cell);
        expect(press(cell.view, "z", { mod: true })).toBe(true);
        expect(rtext.toJSON()).toBe(f.text);
      },
    );

    test("LIMIT: two people clearing the same bare first cell at once leave two pipes", async () => {
      // Accepted limit (R24, fix round 2): each side writes its own `| `, the
      // merge keeps both, and the row reads `| |  | 2`: three cells, so the
      // `2` falls past the table's two columns. No remote normaliser.
      const { main, rtext, remote } = roomMain(f.text);
      const offline = new Y.Doc();
      Y.applyUpdate(offline, Y.encodeStateAsUpdate(remote));
      const otext = offline.getText("content");
      const at = otext.toJSON().indexOf(`\n${f.lines[2] ?? ""}\n`) + 1;
      // The other person's R24 result: `1` gone, `| ` in its place.
      otext.delete(at, 1);
      otext.insert(at, "| ");
      const cell = openAt(main, cellOf(main, 1, 0).content.to);
      press(cell.view, "Backspace");
      await Promise.resolve();
      Y.applyUpdate(remote, Y.encodeStateAsUpdate(offline), "offline");
      await Promise.resolve();
      const lines = [...f.lines];
      lines[2] = "| |  | 2";
      const merged = f.text.replace(f.lines.join("\n"), lines.join("\n"));
      expect(rtext.toJSON()).toBe(merged);
      expect(main.state.doc.toString()).toBe(merged);
      const table = gridTables(main.state)[0];
      expect(table?.rows[1]?.map((c) => c.text)).toEqual(["", "", "2"]);
      expect(table?.model.columns).toBe(2);
      // The local cell stays open on one of the two empty cells of row 1:
      // which one depends on how Yjs orders the two equal inserts (client
      // ids), so only that much is pinned.
      expect(activeCell(main)).toBe(cell);
      const open = openCellRange(main.state);
      expect(open?.row).toBe(1);
      expect([0, 1]).toContain(open?.column);
      expect(open?.text).toBe("");
    });

    test("a peer deleting part of a bare last cell leaves it open", async () => {
      const { main, rtext } = roomMain(f.text);
      const cell = openAt(main, cellOf(main, 1, 1).content.to);
      typeIn(cell, "x");
      const heard = vi.fn();
      onCellClosed(main, heard);
      rtext.delete(cellOf(main, 1, 1).content.from, 1);
      await Promise.resolve();
      expect(heard).not.toHaveBeenCalled();
      expect(activeCell(main)).toBe(cell);
      expect(openText(main)).toBe("x");
      expect(cell.view.state.doc.eq(main.state.doc)).toBe(true);
    });
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

  // An agent's change to another cell of the open cell's row, in the two
  // shapes a server can send it: the characters it changed, or a delete of
  // the whole row line with the new line inserted (the old line-based edit
  // script). The agent's doc is a third peer that forked before the local
  // keystroke, so its change is concurrent with it.
  function agentFork(remote: Y.Doc) {
    const agent = new Y.Doc();
    Y.applyUpdate(agent, Y.encodeStateAsUpdate(remote));
    const since = Y.encodeStateVector(agent);
    return {
      atext: agent.getText("content"),
      deliver: () =>
        Y.applyUpdate(remote, Y.encodeStateAsUpdate(agent, since), "agent"),
    };
  }

  test("an agent's character script for another cell of the row leaves the cell open", async () => {
    const { main, ytext, rtext, remote } = roomMain(DOC);
    const cell = openAt(main, cellOf(main, 1, 0).content.to);
    const heard = vi.fn();
    onCellClosed(main, heard);
    const { atext, deliver } = agentFork(remote);
    typeIn(cell, "x");
    atext.insert(atext.toJSON().indexOf("| 2 |") + 3, "Z");
    deliver();
    await Promise.resolve();
    expect(heard).not.toHaveBeenCalled();
    expect(activeCell(main)).toBe(cell);
    expect(openText(main)).toBe("1x");
    const content = cellOf(main, 1, 0).content;
    expect(cell.view.state.selection.main.head).toBe(content.to);
    expect(main.state.field(openCellField)?.head).toBe(content.to);
    expect(ytext.toJSON()).toBe("| a | b |\n| - | - |\n| 1x | 2Z |\n");
    expect(rtext.toJSON()).toBe(ytext.toJSON());
    typeIn(cell, "y");
    expect(rtext.toJSON()).toBe("| a | b |\n| - | - |\n| 1xy | 2Z |\n");
  });

  test("an agent's row line replace closes the cell and keeps every character", async () => {
    const { main, ytext, rtext, remote } = roomMain(DOC);
    const cell = openAt(main, cellOf(main, 1, 0).content.to);
    typeIn(cell, "x");
    const heard = vi.fn();
    onCellClosed(main, heard);
    const row = rtext.toJSON().indexOf("| 1x |");
    remote.transact(() => {
      rtext.delete(row, "| 1x | 2 |\n".length);
      rtext.insert(row, "| 1x | 2Z |\n");
    }, "agent");
    await Promise.resolve();
    expect(heard).toHaveBeenCalledTimes(1);
    expect(heard).toHaveBeenCalledWith("gone");
    expect(activeCell(main)).toBeNull();
    expect(ytext.toJSON()).toBe("| a | b |\n| - | - |\n| 1x | 2Z |\n");
    expect(main.state.doc.toString()).toBe(ytext.toJSON());
  });

  test("a keystroke concurrent with an agent's row line replace is kept once", async () => {
    // Yjs keeps an insert into a deleted range next to the deleted text, so
    // the keystroke outlives the old row but not in its cell: this is why
    // the server sends the character script instead (Task 11).
    const { main, ytext, rtext, remote } = roomMain(DOC);
    const cell = openAt(main, cellOf(main, 1, 0).content.to);
    const heard = vi.fn();
    onCellClosed(main, heard);
    const { atext, deliver } = agentFork(remote);
    typeIn(cell, "x");
    const row = atext.toJSON().indexOf("| 1 |");
    atext.doc?.transact(() => {
      atext.delete(row, "| 1 | 2 |\n".length);
      atext.insert(row, "| 1 | 2Z |\n");
    });
    deliver();
    await Promise.resolve();
    expect(heard).toHaveBeenCalledWith("gone");
    expect(activeCell(main)).toBeNull();
    const text = ytext.toJSON();
    expect(text.split("x").length - 1).toBe(1);
    expect(text).toContain("| 1 | 2Z |\n");
    expect(rtext.toJSON()).toBe(text);
    expect(main.state.doc.toString()).toBe(text);
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
