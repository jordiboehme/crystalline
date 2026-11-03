/**
 * The cell structure of a pipe table, in document positions. Every range is
 * read back through `doc.sliceString`, never through separator lengths, so the
 * CRLF case proves positions come from line starts.
 */

import { EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";

import { parsedState } from "../../test/parse";
import {
  LIST_ITEM_NOT_A_TABLE,
  PIPELESS_FIXTURES,
} from "../../test/pipelessTables";
import { baseExtensions } from "../setup";
import { cellAt, gridTableAt, gridTables } from "./structure";

function stateOf(doc: string): EditorState {
  return parsedState(
    EditorState.create({ doc, extensions: baseExtensions(false) }),
  );
}

function only(doc: string) {
  const [table] = gridTables(stateOf(doc));
  if (!table) throw new Error("no table");
  return table;
}

/** Row 0 of a table, as [span text, content text] pairs read off the doc. */
function readRow(state: EditorState, row: number) {
  const table = gridTables(state)[0];
  const cells = table?.rows[row] ?? [];
  return cells.map((cell) => [
    state.doc.sliceString(cell.span.from, cell.span.to),
    state.doc.sliceString(cell.content.from, cell.content.to),
  ]);
}

describe("gridTables", () => {
  it("reads a padded row", () => {
    const state = stateOf("| a | b |\n| - | - |\n| 1 | 2 |");
    expect(readRow(state, 0)).toEqual([
      [" a ", "a"],
      [" b ", "b"],
    ]);
    expect(readRow(state, 1)).toEqual([
      [" 1 ", "1"],
      [" 2 ", "2"],
    ]);
    const table = gridTables(state)[0];
    expect(table?.rows.length).toBe(2);
    expect(table?.rows[0]?.[0]).toMatchObject({ row: 0, column: 0, text: "a" });
    expect(table?.rows[1]?.[1]).toMatchObject({ row: 1, column: 1, text: "2" });
  });

  it("reads an unpadded row", () => {
    const state = stateOf("|a|b|\n|-|-|\n|1|2|");
    expect(readRow(state, 0)).toEqual([
      ["a", "a"],
      ["b", "b"],
    ]);
  });

  it("gives an empty cell with no padding an empty content at its span start", () => {
    const state = stateOf("||\n|-|\n||");
    const cell = gridTables(state)[0]?.rows[0]?.[0];
    expect(cell?.span.from).toBe(cell?.span.to);
    expect(cell?.content).toEqual(cell?.span);
  });

  it("gives a cell holding one space an empty content after the space", () => {
    const state = stateOf("|  |\n|-|\n||");
    const cell = gridTables(state)[0]?.rows[0]?.[0];
    expect(cell?.span.to).toBe((cell?.span.from ?? 0) + 2);
    expect(cell?.content.from).toBe((cell?.span.from ?? 0) + 1);
    expect(cell?.content.to).toBe(cell?.content.from);
  });

  it("keeps a single-space cell's content empty and inside the span", () => {
    const state = stateOf("| |\n|-|\n||");
    const cell = gridTables(state)[0]?.rows[0]?.[0];
    expect(cell?.span.to).toBe((cell?.span.from ?? 0) + 1);
    expect(cell?.content.from).toBe((cell?.span.from ?? 0) + 1);
    expect(cell?.content.to).toBe(cell?.content.from);
  });

  it("reads rows without outer pipes", () => {
    const state = stateOf("a | b\n--|--\n1 | 2");
    expect(readRow(state, 0)).toEqual([
      ["a ", "a"],
      [" b", "b"],
    ]);
    expect(readRow(state, 1)).toEqual([
      ["1 ", "1"],
      [" 2", "2"],
    ]);
  });

  it.each(PIPELESS_FIXTURES)("reads every cell of $name (R23)", (f) => {
    const table = only(f.text);
    expect(table.rows.map((row) => row.map((cell) => cell.text))).toEqual(
      f.cells,
    );
    expect(table.aligns).toEqual(
      f.aligns.map((align) => (align === "" ? "none" : align)),
    );
  });

  it("reads `- | -` under a header as no table at all", () => {
    expect(gridTables(stateOf(LIST_ITEM_NOT_A_TABLE))).toEqual([]);
  });

  it("leaves a ragged row's missing cells out of its array", () => {
    const table = only("| a | b | c |\n| - | - | - |\n| 1 |");
    expect(table.rows[0]?.length).toBe(3);
    expect(table.rows[1]?.length).toBe(1);
    expect(table.model.columns).toBe(3);
  });

  it("keeps an escaped pipe inside one cell", () => {
    const table = only("| a | b |\n| - | - |\n| x \\| y | 2 |");
    expect(table.rows[1]?.length).toBe(2);
    expect(table.rows[1]?.[0]?.text).toBe("x \\| y");
  });

  it("carries the alignments and the table's range", () => {
    const doc = "| a | b |\n|:-:| -:|\n| 1 | 2 |";
    const table = only(doc);
    expect(table.aligns).toEqual(["center", "right"]);
    expect(table.from).toBe(0);
    expect(table.to).toBe(doc.length);
  });

  it("gives a CRLF document the LF document's positions plus one per earlier break", () => {
    const lines = ["| a | b |", "| - | - |", "| 1 | 2 |"];
    const lf = stateOf(lines.join("\n"));
    const crlf = stateOf(lines.join("\r\n"));
    // Same document positions: CodeMirror counts a break as one position.
    const flat = (state: EditorState) =>
      (gridTables(state)[0]?.rows ?? []).flat().map((cell) => ({
        span: cell.span,
        content: cell.content,
        text: cell.text,
      }));
    expect(flat(crlf)).toEqual(flat(lf));
    expect(readRow(crlf, 1)).toEqual([
      [" 1 ", "1"],
      [" 2 ", "2"],
    ]);
  });

  it("does not list a table inside a blockquote or a list item", () => {
    expect(gridTables(stateOf("> | a |\n> | - |\n> | 1 |"))).toEqual([]);
    expect(gridTables(stateOf("- item\n\n  | a |\n  | - |\n  | 1 |"))).toEqual(
      [],
    );
  });
});

describe("gridTableAt and cellAt", () => {
  const doc = "| a | b |\n| - | - |\n| 1 | 2 |";

  it("finds the table holding a position", () => {
    const state = stateOf(`intro\n\n${doc}\n\nafter`);
    expect(gridTableAt(state, 0)).toBeNull();
    expect(gridTableAt(state, 10)?.rows.length).toBe(2);
    expect(gridTableAt(state, state.doc.length)).toBeNull();
  });

  it("is null for a table inside a blockquote", () => {
    const state = stateOf("> | a |\n> | - |\n> | 1 |");
    expect(gridTableAt(state, 5)).toBeNull();
  });

  it("returns the cell whose span holds the position, pipe edges included", () => {
    const table = only(doc);
    const [a, b] = table.rows[0] ?? [];
    expect(cellAt(table, 3)).toBe(a);
    // the inner edge of the pipe between the cells, on either side
    expect(cellAt(table, a?.span.to ?? -1)).toBe(a);
    expect(cellAt(table, b?.span.from ?? -1)).toBe(b);
    expect(cellAt(table, b?.span.to ?? -1)).toBe(b);
  });

  it("is null outside every cell", () => {
    const table = only(doc);
    expect(cellAt(table, 0)).toBeNull();
  });
});
