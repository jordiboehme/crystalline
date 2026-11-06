/** `actions.test.ts` again, with the marks show. */

import { expect, test } from "vitest";

import { cellMarksMode, runWithCellMarks } from "../../../test/cellMarksMode";

runWithCellMarks("show");

test("this run has its mode", () => {
  expect(cellMarksMode()).toBe("show");
});

await import("./actions.test");
