/**
 * Pasting into the open cell: text as typed, blocks of cells (TSV) filling
 * cells rightward and downward and growing the table, one undo step.
 */

import type { Extension } from "@codemirror/state";
import { EditorState, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { bytes, cleanupCells, roomMain, soloMain } from "../../test/cellKit";
import { PIPELESS_FIXTURES, withTable } from "../../test/pipelessTables";
import { fencePreviews } from "../fencePreviews";
import { activeCell } from "./cellEditor";
import { hostOpenCell } from "./gridWidget";
import { gridExtension } from "./index";
import { openCellField, openCellRange, setOpenCell } from "./openCell";
import { gridTables } from "./structure";
import { cellForwarding } from "./cellEditor";

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

function layers(): Extension[] {
  return [gridExtension(), fencePreviews(false)];
}

function open(main: EditorView, row: number, column: number): EditorView {
  const cell = gridTables(main.state)[0]?.rows[row]?.[column];
  if (!cell) throw new Error("no cell");
  const pos = cell.content.to;
  main.dispatch({
    effects: setOpenCell.of({ anchor: pos, head: pos, focused: true }),
  });
  const editor = hostOpenCell(main, "now");
  if (!editor) throw new Error("not hosted");
  return editor.view;
}

interface Clip {
  plain?: string;
  html?: string;
  files?: File[];
}

/** Fires a paste on `view`; returns whether the default was prevented. */
function paste(view: EditorView, clip: Clip): boolean {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  const data = {
    getData: (type: string) =>
      type === "text/plain"
        ? (clip.plain ?? "")
        : type === "text/html"
          ? (clip.html ?? "")
          : "",
    files: clip.files ?? [],
    items: [],
    types: [],
  };
  Object.defineProperty(event, "clipboardData", { value: data });
  view.contentDOM.dispatchEvent(event);
  return event.defaultPrevented;
}

function where(main: EditorView): [number, number] | null {
  const cell = openCellRange(main.state);
  return cell ? [cell.row, cell.column] : null;
}

function undo(main: EditorView, view: EditorView): void {
  const event = new KeyboardEvent("keydown", {
    key: "z",
    keyCode: 90,
    ctrlKey: !/Mac/.test(navigator.platform),
    metaKey: /Mac/.test(navigator.platform),
    bubbles: true,
    cancelable: true,
  });
  view.contentDOM.dispatchEvent(event);
  expect(main.state.doc.length).toBeGreaterThan(0);
}

describe("multi-line text without a tab (finding 8)", () => {
  test("two lines of prose are one value, typed at the caret", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 0, 0);
    view.dispatch({ selection: { anchor: view.state.selection.main.head } });
    paste(view, { plain: "First point.\nSecond point." });
    expect(bytes(main.state)).toBe(
      "| aFirst point. Second point. | b |\n| - | - |\n| 1 | 2 |\n",
    );
    expect(where(main)).toEqual([0, 0]);
  });

  test("the cell below is never overwritten, and no row is added", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 1);
    paste(view, { plain: "one\ntwo\nthree\n" });
    expect(bytes(main.state)).toBe(
      "| a | b |\n| - | - |\n| 1 | 2one two three |\n",
    );
  });

  test("a one-column HTML table beside it is still one value (round 2 E)", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    paste(view, {
      plain: "x\ny",
      html: "<table><tr><td>x</td></tr><tr><td>y</td></tr></table>",
    });
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1x y | 2 |\n");
  });

  test("with a two-column HTML table beside it, the same text is a block", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 0, 1);
    paste(view, {
      plain: "x\ny",
      html: "<meta charset='utf-8'><table><tr><th>x</th><th>z</th></tr><tr><td>y</td><td>w</td></tr></table>",
    });
    expect(gridTables(main.state)[0]?.rows.map((r) => r[1]?.text)).toEqual([
      "x",
      "y",
    ]);
  });

  test("other HTML does not make it a block", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "p\nq", html: "<p>p</p><p>q</p>" });
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1p q | 2 |\n");
  });
});

describe("a block paste after a slow parse (finding 13)", () => {
  test("lands in the open cell's new place", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    let now = Date.now();
    const clock = vi.spyOn(Date, "now").mockImplementation(() => {
      now += 21;
      return now;
    });
    try {
      paste(view, { plain: "x\ty\nz\tw" });
    } finally {
      clock.mockRestore();
    }
    expect(bytes(main.state)).toBe(
      "| a | b |\n| - | - |\n| x | y |\n| z | w |\n",
    );
    expect(activeCell(main)).not.toBeNull();
    expect(where(main)).toEqual([1, 0]);
  });
});

describe("paste of one value", () => {
  test("plain text goes in as typed", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "a" });
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1a | 2 |\n");
  });

  test("a pipe is escaped", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "a|b" });
    expect(bytes(main.state)).toContain("| 1a\\|b | 2 |");
  });

  test("HTML only is converted with line breaks as spaces", async () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    paste(view, { html: "<p>one</p><p>two</p>" });
    await vi.waitFor(() => {
      expect(bytes(main.state)).toContain("| 1one two |");
    });
  });

  test("files alone change nothing", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    const file = new File(["x"], "x.png", { type: "image/png" });
    expect(paste(view, { files: [file] })).toBe(true);
    expect(bytes(main.state)).toBe(DOC);
  });
});

describe("paste of a block", () => {
  test("fills rightward and adds a row", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "x\ty\nz\tw" });
    expect(bytes(main.state)).toBe(
      "| a | b |\n| - | - |\n| x | y |\n| z | w |\n",
    );
  });

  test("adds rows at the bottom, one per extra line", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "x\ty\nz\tw" });
    const text = bytes(main.state);
    expect(text).toBe("| a | b |\n| - | - |\n| x | y |\n| z | w |\n");
    expect(gridTables(main.state)[0]?.rows).toHaveLength(3);
  });

  test("adds a column for a wide block", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "x\ty\tz" });
    const table = gridTables(main.state)[0];
    expect(table?.model.columns).toBe(3);
    expect(table?.rows[1]?.map((c) => c.text)).toEqual(["x", "y", "z"]);
    expect(table?.rows[0]?.map((c) => c.text)).toEqual(["a", "b", "Column"]);
  });

  test("a CRLF clipboard gives two rows, the trailing empty line is dropped", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    // No tab, but an HTML table of two columns beside the text.
    paste(view, {
      plain: "x\r\ny\r\n",
      html: "<table><tr><td>x</td><td></td></tr><tr><td>y</td><td></td></tr></table>",
    });
    const table = gridTables(main.state)[0];
    expect(table?.rows.map((r) => r[0]?.text)).toEqual(["a", "x", "y"]);
  });

  test("pipes are escaped and cells hold no line break", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "p|q\tr\\" });
    const table = gridTables(main.state)[0];
    expect(table?.rows[1]?.map((c) => c.text)).toEqual(["p\\|q", "r\\"]);
    expect(table?.model.columns).toBe(2);
  });

  test("keeps the cell open where the paste started, caret at its end", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "x\ty\nz\tw" });
    expect(where(main)).toEqual([1, 0]);
    const cell = openCellRange(main.state);
    expect(main.state.field(openCellField)?.head).toBe(cell?.content.to);
    expect(activeCell(main)).not.toBeNull();
  });

  test("one undo restores the exact bytes", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 1);
    paste(view, { plain: "x\ty\tz\nq\tr\ts" });
    expect(bytes(main.state)).not.toBe(DOC);
    undo(main, activeCell(main)?.view ?? view);
    expect(bytes(main.state)).toBe(DOC);
  });

  test("one undo in a room restores the exact bytes", () => {
    const { main } = roomMain(DOC, [cellForwarding, ...layers()]);
    const view = open(main, 1, 1);
    paste(view, { plain: "x\ty\tz\nq\tr\ts" });
    expect(bytes(main.state)).not.toBe(DOC);
    undo(main, activeCell(main)?.view ?? view);
    expect(bytes(main.state)).toBe(DOC);
  });

  test("a read-only page changes nothing", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 0);
    main.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    expect(paste(view, { plain: "x\ty\nz\tw" })).toBe(true);
    expect(bytes(main.state)).toBe(DOC);
  });
});

/**
 * R23: a block pasted into a table without outer pipes fills its cells in
 * place, and the rows and columns it adds are written in the table's own
 * spelling: a row keeps the edge pipes it had, a new row copies the row above
 * it, and an edge pipe is added only where its cell is empty.
 */
describe.each(PIPELESS_FIXTURES)("paste without outer pipes: $name", (f) => {
  const GROWN: Record<string, string[]> = {
    "pipeless.md": [
      "a | b | Column",
      "--|-- | ---",
      "p | q | r",
      "s | t | u",
      "v | w | x",
    ],
    "mixed.md": [
      "| a | b | Column |",
      "|:--|--:| --- |",
      "p | q | r",
      "| s | t | u |",
      "| v | w | x |",
    ],
  };

  test("a block inside the table replaces the cells and nothing else", () => {
    const main = soloMain(f.text, layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "p\tq\ns\tt" });
    const lines = [...f.lines];
    lines[2] = lines[2]?.replace("1", "p").replace("2", "q") ?? "";
    lines[3] = lines[3]?.replace("3", "s").replace("4", "t") ?? "";
    expect(bytes(main.state)).toBe(withTable(f, lines));
    expect(where(main)).toEqual([1, 0]);
    undo(main, activeCell(main)?.view ?? view);
    expect(bytes(main.state)).toBe(f.text);
  });

  test("a block that grows the table writes its spelling, one undo", () => {
    const main = soloMain(f.text, layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "p\tq\tr\ns\tt\tu\nv\tw\tx" });
    expect(bytes(main.state)).toBe(withTable(f, GROWN[f.name] ?? []));
    const table = gridTables(main.state)[0];
    expect(table?.rows.map((row) => row.map((c) => c.text))).toEqual([
      ["a", "b", "Column"],
      ["p", "q", "r"],
      ["s", "t", "u"],
      ["v", "w", "x"],
    ]);
    expect(where(main)).toEqual([1, 0]);
    undo(main, activeCell(main)?.view ?? view);
    expect(bytes(main.state)).toBe(f.text);
  });

  test("an empty value at a bare edge keeps its cell with the pipe it needs", () => {
    const main = soloMain(f.text, layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "\tq\ns\t" });
    const table = gridTables(main.state)[0];
    expect(table?.rows.map((row) => row.map((c) => c.text))).toEqual([
      ["a", "b"],
      ["", "q"],
      ["s", ""],
    ]);
    const lines = [...f.lines];
    lines[2] = "| | q";
    lines[3] = f.name === "mixed.md" ? "| s |  |" : "s | |";
    expect(bytes(main.state)).toBe(withTable(f, lines));
  });
});

describe("ragged rows, CRLF and the undo landing", () => {
  test("a short row with outer pipes grows to hold the paste", () => {
    const main = soloMain("| a | b |\n| - | - |\n| 1 |\n", layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "x\ty" });
    const table = gridTables(main.state)[0];
    expect(table?.rows[1]?.map((c) => c.text)).toEqual(["x", "y"]);
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| x | y |\n");
  });

  test("a row without a closing pipe keeps its spelling and every value", () => {
    const doc = "| a | b | c |\n| - | - | - |\n| 1 | 2\n";
    const main = soloMain(doc, layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "x\ty\tz" });
    const table = gridTables(main.state)[0];
    expect(table?.rows[1]?.map((c) => c.text)).toEqual(["x", "y", "z"]);
    expect(bytes(main.state)).not.toContain("||");
    undo(main, activeCell(main)?.view ?? view);
    expect(bytes(main.state)).toBe(doc);
  });

  test("a CRLF document: exact bytes and one undo", () => {
    const doc = "| a | b |\r\n| - | - |\r\n| 1 | 2 |\r\n";
    const main = soloMain(doc, layers());
    const view = open(main, 1, 0);
    paste(view, { plain: "x\ty\nz\tw" });
    expect(bytes(main.state)).toBe(
      "| a | b |\r\n| - | - |\r\n| x | y |\r\n| z | w |\r\n",
    );
    undo(main, activeCell(main)?.view ?? view);
    expect(bytes(main.state)).toBe(doc);
  });

  test("undo of a reshaping paste reopens the start cell", () => {
    const main = soloMain(DOC, layers());
    const view = open(main, 1, 1);
    paste(view, { plain: "x\ty\nz\tw" });
    undo(main, activeCell(main)?.view ?? view);
    expect(bytes(main.state)).toBe(DOC);
    expect(where(main)).toEqual([1, 1]);
    expect(activeCell(main)).not.toBeNull();
  });
});
