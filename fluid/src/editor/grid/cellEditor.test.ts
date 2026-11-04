/**
 * The cell editor: one table cell edited in a second view that holds the
 * whole main document, with edits clamped to the cell, pipes escaped and undo
 * run on the main view's own stack. Every probe runs solo (CodeMirror
 * history) and in a room (Y.UndoManager).
 */

import { syntaxTree } from "@codemirror/language";
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
import { PIPELESS_FIXTURES, withTable } from "../../test/pipelessTables";
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

  // R23: every cell of a table without outer pipes edits in place, a typed
  // pipe stored escaped, and nothing outside the cell changes.
  describe.each(PIPELESS_FIXTURES)("without outer pipes: $name", (f) => {
    const places = f.cells.flatMap((row, r) => row.map((_, c) => [r, c]));
    test.each(places)(
      "typing a pipe into cell %i/%i changes only that cell",
      async (row, column) => {
        const main = make(f.text);
        const at = cellOf(main, row ?? 0, column ?? 0).content.to;
        const cell = openAt(main, at);
        typeIn(cell, "x|y");
        expect(bytes(main.state)).toBe(
          `${f.text.slice(0, at)}x\\|y${f.text.slice(at)}`,
        );
        expect(cell.view.contentDOM.textContent).toBe(
          `${f.cells[row ?? 0]?.[column ?? 0] ?? ""}x|y`,
        );
        const table = gridTables(parsedState(main.state))[0];
        expect(table?.rows.map((r) => r.length)).toEqual([2, 2, 2]);
        expect(table?.rows[row ?? 0]?.[column ?? 0]?.text).toBe(
          `${f.cells[row ?? 0]?.[column ?? 0] ?? ""}x\\|y`,
        );
        closeCell(main, "user", "none");
        await Promise.resolve();
        expect(activeCell(main)).toBeNull();
        expect(bytes(main.state)).toBe(
          `${f.text.slice(0, at)}x\\|y${f.text.slice(at)}`,
        );
      },
    );
  });

  // R24: an edit that would cost a bare edge cell its cellhood writes the
  // edge pipe it needs in the same change, and the cell stays open.
  describe.each(PIPELESS_FIXTURES)("R24 without outer pipes: $name", (f) => {
    const row1 = (line: string) => {
      const lines = [...f.lines];
      lines[2] = line;
      return withTable(f, lines);
    };
    // Read off a whole parse: right after a change the state's own tree has
    // had one 20 ms slice of wall clock, which a busy machine can cut short
    // of the table (D54). The grid draws through that (`gridField`); a test
    // that reads the tree itself asks for the parse, as `parsedState` says.
    const shape = (main: EditorView) =>
      gridTables(parsedState(main.state))[0]?.rows.map((r) =>
        r.map((c) => c.text),
      );

    test("Backspace on a bare first cell's only character keeps the cell open", () => {
      const main = make(f.text);
      const cell = openAt(main, cellOf(main, 1, 0).content.to);
      expect(press(cell.view, "Backspace")).toBe(true);
      expect(bytes(main.state)).toBe(row1("|  | 2"));
      expect(shape(main)).toEqual([
        ["a", "b"],
        ["", "2"],
        ["3", "4"],
      ]);
      expect(activeCell(main)).toBe(cell);
      expect(main.state.field(openCellField)?.head).toBe(
        cellOf(main, 1, 0).content.from,
      );
      typeIn(cell, "z");
      expect(bytes(main.state)).toBe(row1("| z | 2"));
      expect(openText(main)).toBe("z");
    });

    test("Backspace on a bare last cell's only character keeps the cell open", () => {
      const main = make(f.text);
      const cell = openAt(main, cellOf(main, 1, 1).content.to);
      expect(press(cell.view, "Backspace")).toBe(true);
      expect(bytes(main.state)).toBe(row1("1 |  |"));
      expect(shape(main)).toEqual([
        ["a", "b"],
        ["1", ""],
        ["3", "4"],
      ]);
      expect(activeCell(main)).toBe(cell);
      typeIn(cell, "z");
      expect(bytes(main.state)).toBe(row1("1 | z |"));
      expect(openText(main)).toBe("z");
    });

    test.each([
      ["- ", "| - 1 | 2"],
      ["* ", "| * 1 | 2"],
      ["+ ", "| + 1 | 2"],
      ["1. ", "| 1. 1 | 2"],
      ["1) ", "| 1) 1 | 2"],
      ["# ", "| # 1 | 2"],
      ["### ", "| ### 1 | 2"],
      ["> ", "| > 1 | 2"],
      ["```", "| ```1 | 2"],
    ])(
      "typing %s at the start of a bare first cell adds the leading pipe",
      (marker, line) => {
        const main = make(f.text);
        const cell = openAt(main, cellOf(main, 1, 0).content.from);
        typeIn(cell, marker);
        expect(bytes(main.state)).toBe(row1(line));
        expect(shape(main)).toEqual([
          ["a", "b"],
          [`${marker}1`, "2"],
          ["3", "4"],
        ]);
        expect(activeCell(main)).toBe(cell);
        expect(cell.view.state.selection.main.head).toBe(
          cellOf(main, 1, 0).content.from + marker.length,
        );
        // Lezer agrees: one table, every data row with its two cells.
        let tables = 0;
        let rows = 0;
        syntaxTree(parsedState(main.state)).iterate({
          enter: (node) => {
            if (node.name === "Table") tables += 1;
            if (node.name === "TableRow") rows += 1;
          },
        });
        expect([tables, rows]).toEqual([1, 2]);
      },
    );

    test("an ideographic or no-break space at the start is text, not indent", () => {
      const main = make(f.text);
      const cell = openAt(main, cellOf(main, 1, 0).content.from);
      typeIn(cell, "\u3000");
      expect(bytes(main.state)).toBe(row1("\u30001 | 2"));
      const other = openAt(main, cellOf(main, 2, 0).content.from);
      typeIn(other, "\u00a0");
      const lines = [...f.lines];
      lines[2] = "\u30001 | 2";
      lines[3] = (f.lines[3] ?? "").replace("3", "\u00a03");
      expect(bytes(main.state)).toBe(withTable(f, lines));
    });

    test("an edit away from the cell's edges leaves the row's spelling alone", () => {
      // `<b>` opens a block to the server's merge, but this row was written so
      // and the edit only reaches the cell's end: no pipe is added for it.
      const text = withTable(f, [
        ...f.lines.slice(0, 2),
        "<b>1</b> | 2",
        f.lines[3] ?? "",
      ]);
      const main = make(text);
      const cell = openAt(main, cellOf(main, 1, 0).content.to);
      typeIn(cell, "x");
      expect(bytes(main.state)).toBe(text.replace("<b>1</b> |", "<b>1</b>x |"));
    });

    /** Marks the cell view as composing, as CodeMirror does under an IME. */
    function composing(cell: CellEditor, on: boolean): void {
      if (on) {
        Object.defineProperty(cell.view, "composing", {
          get: () => true,
          configurable: true,
        });
        return;
      }
      delete (cell.view as unknown as { composing?: boolean }).composing;
      cell.view.contentDOM.dispatchEvent(
        new CompositionEvent("compositionend"),
      );
    }

    function compose(cell: CellEditor, from: number, to: number, text: string) {
      cell.view.dispatch({
        changes: { from, to, insert: text },
        selection: { anchor: from + text.length },
        userEvent: "input.type.compose",
      });
    }

    /** CodeMirror may flush a composition's last change after its end. */
    const settled = () => new Promise<void>((done) => setTimeout(done, 0));

    test.each([
      ["<b>", "| <b>1 | 2"],
      [" ", "|  1 | 2"],
    ])(
      "a composition opening a bare first cell with %j gets its pipe once it ends",
      async (marker, line) => {
        const main = make(f.text);
        const { content } = cellOf(main, 1, 0);
        const cell = openAt(main, content.to);
        composing(cell, true);
        compose(cell, content.from, content.from, marker.slice(0, 1));
        if (marker.length > 1)
          compose(cell, content.from, content.from + 1, marker);
        // Mid-composition no pipe: nothing outside the composed text moves,
        // and the cell stays open.
        expect(bytes(main.state)).toBe(row1(`${marker}1 | 2`));
        expect(activeCell(main)).toBe(cell);
        composing(cell, false);
        await settled();
        expect(bytes(main.state)).toBe(row1(line));
        expect(openText(main)).toBe(`${marker}1`);
        expect(activeCell(main)).toBe(cell);
        const head = cell.view.state.selection.main.head;
        const now = cellOf(main, 1, 0).content;
        expect(head >= now.from && head <= now.to).toBe(true);
        expect(press(cell.view, "z", { mod: true })).toBe(true);
        expect(bytes(main.state)).toBe(f.text);
      },
    );

    test("a composition of plain text, or one opening with U+3000, never gets a pipe", async () => {
      const main = make(f.text);
      const { content } = cellOf(main, 1, 0);
      const cell = openAt(main, content.to);
      composing(cell, true);
      compose(cell, content.from, content.to, "\u3042");
      compose(cell, content.from, content.from + 1, "\u3000\u65e5\u672c");
      composing(cell, false);
      await settled();
      expect(bytes(main.state)).toBe(row1("\u3000\u65e5\u672c | 2"));
      expect(openText(main)).toBe("\u3000\u65e5\u672c");
      expect(activeCell(main)).toBe(cell);
    });

    // The accepted limit: a composition whose own text ends the cell before
    // it ends closes the cell as gone, as before R24. Its bytes are exactly
    // the composed text, and no pipe is added afterwards.
    test.each([
      ["empties a bare last cell", 1, "", "1 | "],
      ["opens a bare first cell with a list marker", 0, "- ", "- 1 | 2"],
    ])(
      "LIMIT: a composition that %s closes the cell, no pipe",
      async (_what, column, text, line) => {
        const main = make(f.text);
        const { content } = cellOf(main, 1, column);
        const cell = openAt(main, content.to);
        const heard = vi.fn();
        onCellClosed(main, heard);
        composing(cell, true);
        if (text === "") compose(cell, content.from, content.to, "");
        else compose(cell, content.from, content.from, text);
        await Promise.resolve();
        expect(heard).toHaveBeenCalledWith("gone");
        expect(activeCell(main)).toBeNull();
        composing(cell, false);
        await settled();
        expect(bytes(main.state)).toBe(row1(line));
      },
    );

    test("text that opens no block keeps the row bare", () => {
      const main = make(f.text);
      const cell = openAt(main, cellOf(main, 1, 0).content.from);
      typeIn(cell, "-x");
      expect(bytes(main.state)).toBe(row1("-x1 | 2"));
    });

    test("one undo gives back the exact bytes, pipe and all", () => {
      const main = make(f.text);
      const cell = openAt(main, cellOf(main, 1, 0).content.to);
      press(cell.view, "Backspace");
      expect(bytes(main.state)).toBe(row1("|  | 2"));
      expect(press(cell.view, "z", { mod: true })).toBe(true);
      expect(bytes(main.state)).toBe(f.text);
      const other = openAt(main, cellOf(main, 1, 1).content.to);
      press(other.view, "Backspace");
      expect(bytes(main.state)).toBe(row1("1 |  |"));
      expect(press(other.view, "z", { mod: true })).toBe(true);
      expect(bytes(main.state)).toBe(f.text);
    });
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
    expect(gridTables(parsedState(main.state))[0]?.rows[1]?.length).toBe(2);
    expect(cell.view.contentDOM.textContent).toBe("1\\");
    // Backspace over the visible backslash removes the pair.
    expect(press(cell.view, "Backspace")).toBe(true);
    expect(bytes(main.state)).toBe("|A|B|\n|-|-|\n|1|2|\n");
  });

  test("a deletion that exposes a backslash before the pipe doubles it", () => {
    const { main, cell } = open("|A|B|\n|-|-|\n|a\\b|2|\n");
    expect(press(cell.view, "Backspace")).toBe(true);
    expect(bytes(main.state)).toBe("|A|B|\n|-|-|\n|a\\\\|2|\n");
    expect(gridTables(parsedState(main.state))[0]?.rows[1]?.length).toBe(2);
    expect(cell.view.contentDOM.textContent).toBe("a\\");
  });

  test("a pipe typed after a literal backslash stays inside the cell", () => {
    const { main } = open("| a | b |\n| - | - |\n| x\\y | 2 |\n");
    const content = cellOf(main, 1, 0).content;
    const cell = openAt(main, content.from + 2);
    typeIn(cell, "|");
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| x\\\\\\|y | 2 |\n");
    expect(gridTables(parsedState(main.state))[0]?.rows[1]?.length).toBe(2);
    expect(cell.view.contentDOM.textContent).toBe("x\\|y");
  });

  test("deleting between a backslash and an escaped pipe keeps the cell", () => {
    const { main } = open("| a | b |\n| - | - |\n| a\\x\\|b | 2 |\n");
    const content = cellOf(main, 1, 0).content;
    const cell = openAt(main, content.from + 3);
    expect(cell.view.contentDOM.textContent).toBe("a\\x|b");
    expect(press(cell.view, "Backspace")).toBe(true);
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| a\\\\\\|b | 2 |\n");
    expect(gridTables(parsedState(main.state))[0]?.rows[1]?.length).toBe(2);
    expect(cell.view.contentDOM.textContent).toBe("a\\|b");
  });

  test("Backspace on an escaped pipe after a literal backslash shows the backslash once", () => {
    const { main } = open("| a | b |\n| - | - |\n| x\\\\\\|y | 2 |\n");
    const content = cellOf(main, 1, 0).content;
    const cell = openAt(main, content.from + 5);
    expect(cell.view.contentDOM.textContent).toBe("x\\|y");
    expect(press(cell.view, "Backspace")).toBe(true);
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| x\\\\y | 2 |\n");
    expect(cell.view.contentDOM.textContent).toBe("x\\y");
  });

  test("a letter typed between a visible backslash and a pipe keeps the backslash single", () => {
    const { main } = open("| a | b |\n| - | - |\n| x\\\\\\|y | 2 |\n");
    const content = cellOf(main, 1, 0).content;
    const cell = openAt(main, content.from + 3);
    typeIn(cell, "Z");
    expect(bytes(main.state)).toBe(
      "| a | b |\n| - | - |\n| x\\\\Z\\|y | 2 |\n",
    );
    expect(gridTables(parsedState(main.state))[0]?.rows[1]?.length).toBe(2);
    expect(cell.view.contentDOM.textContent).toBe("x\\Z|y");
  });

  test("a backslash and a letter typed into a compact cell show as typed", () => {
    const { main, cell } = open("|A|B|\n|-|-|\n|C:|2|\n");
    typeIn(cell, "\\");
    typeIn(cell, "U");
    expect(bytes(main.state)).toBe("|A|B|\n|-|-|\n|C:\\\\U|2|\n");
    expect(gridTables(parsedState(main.state))[0]?.rows[1]?.length).toBe(2);
    expect(cell.view.contentDOM.textContent).toBe("C:\\U");
  });

  test("a hand-written backslash pair shows once and Backspace takes the pair", () => {
    const { main } = open("| a | b |\n| - | - |\n| a\\\\b | 2 |\n");
    const content = cellOf(main, 1, 0).content;
    const cell = openAt(main, content.from + 3);
    expect(cell.view.contentDOM.textContent).toBe("a\\b");
    expect(press(cell.view, "Backspace")).toBe(true);
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| ab | 2 |\n");
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
