/** `labelEditor.test.ts` again, with the marks hidden on the edited line. */

import { expect, test } from "vitest";

import { cellMarksMode, runWithCellMarks } from "../../test/cellMarksMode";

runWithCellMarks("hidden");

test("this run has its mode", () => {
  expect(cellMarksMode()).toBe("hidden");
});

await import("./labelEditor.test");
