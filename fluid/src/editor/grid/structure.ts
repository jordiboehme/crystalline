/**
 * The cells of a pipe table, in document positions.
 *
 * `parseTable` works in offsets local to a span and joins lines with `\n`.
 * A CodeMirror document counts any line break as ONE position whatever it
 * spells, so a cell's position is its line's start (`doc.line(n).from`) plus
 * its offset inside the line text - never a running sum of separator lengths.
 */

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Text } from "@codemirror/state";

import type { Align, TableModel } from "../tableModel";
import { parseTable } from "../tableModel";
import { gridDraws } from "./draws";

export interface GridCell {
  row: number;
  column: number;
  /** The text between the cell's pipes (or the line edge). */
  span: { from: number; to: number };
  /** The span less one padding space on each side, when present. */
  content: { from: number; to: number };
  text: string;
}

/** `rows` exclude the delimiter line; `rows[0]` is the header. */
export interface GridTable {
  from: number;
  to: number;
  model: TableModel;
  aligns: Align[];
  rows: GridCell[][];
}

/**
 * The cells of the table spanning `from` to `to` of `doc`, in document
 * positions. Works on any document, so a caller can read a scratch copy.
 */
export function tableIn(doc: Text, from: number, to: number): GridTable {
  const model = parseTable(doc.sliceString(from, to));
  if (!model) throw new Error("gridDraws accepted a table parseTable refuses");
  const firstLine = doc.lineAt(from).number;
  const rows: GridCell[][] = [];
  model.lines.forEach((line, index) => {
    if (index === 1) return; // the delimiter line is not a row
    // The first line of the span starts at the node, which may sit after an
    // indent; every other line starts at its own line start.
    const lineStart = index === 0 ? from : doc.line(firstLine + index).from;
    const row = rows.length;
    rows.push(
      line.cells.map((cell, column) => {
        const spanFrom = lineStart + cell.from;
        const spanTo = lineStart + cell.to;
        const padLeft = cell.raw.startsWith(" ") ? 1 : 0;
        const padRight = cell.raw.endsWith(" ") && cell.raw.length > 1 ? 1 : 0;
        const contentFrom = spanFrom + padLeft;
        const contentTo = Math.max(contentFrom, spanTo - padRight);
        return {
          row,
          column,
          span: { from: spanFrom, to: spanTo },
          content: { from: contentFrom, to: contentTo },
          text: doc.sliceString(contentFrom, contentTo),
        };
      }),
    );
  });
  return { from, to, model, aligns: model.aligns, rows };
}

/** Every table the grid draws, in document order. */
export function gridTables(state: EditorState): GridTable[] {
  const found: GridTable[] = [];
  syntaxTree(state).iterate({
    enter: (node) => {
      if (node.name === "Table" && gridDraws(node, state.doc)) {
        found.push(tableIn(state.doc, node.from, node.to));
      }
    },
  });
  return found;
}

/** The grid table holding `pos`, or null (also for a table the grid skips). */
export function gridTableAt(state: EditorState, pos: number): GridTable | null {
  return (
    gridTables(state).find((table) => table.from <= pos && pos <= table.to) ??
    null
  );
}

/**
 * The cell whose span holds `pos`, inclusive of both ends: the inner edge of
 * each pipe belongs to the cell beside it, and a position exactly on a pipe's
 * left edge belongs to the cell on its left.
 */
export function cellAt(table: GridTable, pos: number): GridCell | null {
  for (const row of table.rows) {
    for (const cell of row) {
      if (cell.span.from <= pos && pos <= cell.span.to) return cell;
    }
  }
  return null;
}
