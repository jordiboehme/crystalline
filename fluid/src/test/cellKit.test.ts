/**
 * `openAt` mounts on a main view whose last edit was parsed only partly. An
 * edit parses within a 20 millisecond `Date.now()` budget, and a loaded
 * machine runs out of it before the table; the clock here jumps a second on
 * every read to stand in for that, so the race is the same every run.
 */

import { syntaxTree } from "@codemirror/language";
import { afterEach, expect, test, vi } from "vitest";

import { cellForwarding } from "../editor/grid/cellEditor";
import { gridTables } from "../editor/grid/structure";
import { cleanupCells, openAt, soloMain } from "./cellKit";

afterEach(() => {
  vi.restoreAllMocks();
  cleanupCells();
});

const TEXT = [
  "# Title",
  "",
  "Some text.",
  "",
  "More text.",
  "",
  "| a | b |",
  "| --- | --- |",
  "| 1 | 2 |",
  "",
].join("\n");

test("opens a cell after an edit whose parse ran out of time", () => {
  const main = soloMain(TEXT, [cellForwarding]);
  const cell = gridTables(main.state)[0]?.rows[1]?.[0];
  if (!cell) throw new Error("no cell");
  let now = Date.now();
  vi.spyOn(Date, "now").mockImplementation(() => (now += 1000));
  main.dispatch({ changes: { from: 0, insert: "Z" } });
  vi.restoreAllMocks();
  // The precondition: the edit's parse stopped before the table.
  expect(syntaxTree(main.state).length).toBeLessThan(TEXT.indexOf("|"));
  const editor = openAt(main, cell.content.to + 1, false);
  expect(editor.view.state.doc.toString()).toBe(main.state.doc.toString());
});
