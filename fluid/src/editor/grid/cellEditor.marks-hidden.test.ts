/**
 * `cellEditor.test.ts` again, with the hidden-mark layer in the cell and marks
 * hidden on the edited line (Task 12, F5).
 */

import { expect, test } from "vitest";

import { cleanupCells, openAt, soloMain } from "../../test/cellKit";
import { runWithCellMarks } from "../../test/cellMarksMode";
import { hiddenMarkLayerOn, marksOnEditLine } from "../marks/mode";
import { cellForwarding } from "./cellEditor";

runWithCellMarks("hidden");

test("the cells of this run carry the layer, marks hidden", () => {
  const main = soloMain("| a | **b** |\n| - | - |\n", [cellForwarding]);
  const cell = openAt(main, 6, false).view;
  expect(cell.state.facet(hiddenMarkLayerOn)).toBe(true);
  expect(cell.state.facet(marksOnEditLine)).toBe(false);
  cleanupCells();
});

await import("./cellEditor.test");
