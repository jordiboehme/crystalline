/** `apply.collab.test.ts` again, with the marks layer on (marks hidden). */

import { expect, test } from "vitest";

import { cellMarksMode, runWithCellMarks } from "../../test/cellMarksMode";

runWithCellMarks("hidden");

test("this run has its mode", () => {
  expect(cellMarksMode()).toBe("hidden");
});

await import("./apply.collab.test");
