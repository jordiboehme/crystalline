/** `labelEditor.test.ts` again, with the marks shown. */

import { expect, test } from "vitest";

import { cellMarksMode, runWithCellMarks } from "../../test/cellMarksMode";

runWithCellMarks("show");

test("this run has its mode", () => {
  expect(cellMarksMode()).toBe("show");
});

await import("./labelEditor.test");
