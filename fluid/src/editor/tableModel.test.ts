/**
 * The table model's contract: what it parses, what it refuses and - the part
 * that matters for collaborative editing - how little it changes.
 *
 * Every structural verb is asserted through `apply`, a round trip over the
 * emitted span-relative changes, so the tests pin the resulting TEXT rather
 * than the shape of the changeset. Where the count of changes is asserted it
 * is because the minimum is the point: one insertion per line for a column,
 * exactly one insertion for a row, exactly one replacement for an alignment,
 * and nothing at all for an already-canonical prettify.
 */

import { describe, expect, test } from "vitest";

import {
  addColumnAfter,
  addRowBelow,
  columnAt,
  deleteColumn,
  deleteRow,
  parseTable,
  prettify,
  setAlignment,
} from "./tableModel";

const SPAN = ["| a | b |", "| --- | --- |", "| 1 | 2 |"].join("\n");

/** Apply span-relative changes to a string, for round-trip assertions. */
function apply(
  span: string,
  changes: { from: number; to?: number; insert?: string }[],
): string {
  let out = span;
  for (const change of [...changes].sort((x, y) => y.from - x.from)) {
    out =
      out.slice(0, change.from) +
      (change.insert ?? "") +
      out.slice(change.to ?? change.from);
  }
  return out;
}

describe("parseTable", () => {
  test("parses cells with line-local spans", () => {
    const model = parseTable(SPAN);
    expect(model?.columns).toBe(2);
    expect(model?.lines[0]?.cells.map((c) => c.raw.trim())).toEqual(["a", "b"]);
    expect(model?.aligns).toEqual(["none", "none"]);
  });

  test("an escaped pipe stays cell content", () => {
    const model = parseTable("| a \\| b | c |\n| --- | --- |\n| 1 | 2 |");
    expect(model?.columns).toBe(2);
    expect(model?.lines[0]?.cells[0]?.raw).toContain("\\|");
  });

  test("rows without edge pipes parse", () => {
    const model = parseTable("a | b\n--- | ---\n1 | 2");
    expect(model?.columns).toBe(2);
    expect(model?.lines[0]?.leadingPipe).toBe(false);
  });

  test("a span whose second line is not a delimiter is refused", () => {
    expect(parseTable("| a | b |\n| 1 | 2 |")).toBeNull();
  });
});

describe("columnAt", () => {
  test("maps an offset into its cell", () => {
    const line = parseTable(SPAN)?.lines[0];
    if (!line) throw new Error("no line");
    expect(columnAt(line, 3)).toBe(0); // inside " a "
    expect(columnAt(line, 7)).toBe(1); // inside " b "
  });
});

describe("addColumnAfter", () => {
  test("emits exactly one insertion per line, nothing else", () => {
    const model = parseTable(SPAN);
    if (!model) throw new Error("no model");
    const changes = addColumnAfter(model, 0);
    expect(changes).toHaveLength(3);
    expect(
      changes?.every((c) => c.to === undefined && c.insert !== undefined),
    ).toBe(true);
    expect(apply(SPAN, changes ?? [])).toBe(
      ["| a | Column | b |", "| --- | --- | --- |", "| 1 |  | 2 |"].join("\n"),
    );
  });

  test("a ragged row is extended rather than skipped", () => {
    const span = "| a | b |\n| --- | --- |\n| 1 |";
    const model = parseTable(span);
    if (!model) throw new Error("no model");
    const grown = apply(span, addColumnAfter(model, 1) ?? []);
    // Every row ends with the new trailing column.
    expect(parseTable(grown)?.lines.every((l) => l.cells.length === 3)).toBe(
      true,
    );
  });
});

describe("deleteColumn and deleteRow", () => {
  test("delete column removes each cell span plus one adjoining pipe", () => {
    const model = parseTable(SPAN);
    if (!model) throw new Error("no model");
    expect(apply(SPAN, deleteColumn(model, 0) ?? [])).toBe(
      ["| b |", "| --- |", "| 2 |"].join("\n"),
    );
  });

  test("the last column and the header row are refused", () => {
    const one = parseTable("| a |\n| --- |\n| 1 |");
    if (!one) throw new Error("no model");
    expect(deleteColumn(one, 0)).toBeNull();
    expect(deleteRow(one, 0, "\n")).toBeNull();
    expect(deleteRow(one, 1, "\n")).toBeNull();
  });

  test("delete row removes the line and one separator", () => {
    const model = parseTable(SPAN);
    if (!model) throw new Error("no model");
    expect(apply(SPAN, deleteRow(model, 2, "\n") ?? [])).toBe(
      ["| a | b |", "| --- | --- |"].join("\n"),
    );
  });
});

describe("addRowBelow", () => {
  test("emits one insertion at the end of the target line", () => {
    const model = parseTable(SPAN);
    if (!model) throw new Error("no model");
    const changes = addRowBelow(model, 2, "\n");
    expect(changes).toHaveLength(1);
    expect(apply(SPAN, changes ?? [])).toBe(
      ["| a | b |", "| --- | --- |", "| 1 | 2 |", "|  |  |"].join("\n"),
    );
  });

  test("the header and delimiter rows clamp to below the delimiter", () => {
    // A data row between header and rule is not a GFM table, so rows 0 and 1
    // both insert after line 1.
    const model = parseTable(SPAN);
    if (!model) throw new Error("no model");
    const below = ["| a | b |", "| --- | --- |", "|  |  |", "| 1 | 2 |"].join(
      "\n",
    );
    expect(apply(SPAN, addRowBelow(model, 0, "\n") ?? [])).toBe(below);
    expect(apply(SPAN, addRowBelow(model, 1, "\n") ?? [])).toBe(below);
  });
});

describe("setAlignment", () => {
  test("touches only the delimiter cell", () => {
    const model = parseTable(SPAN);
    if (!model) throw new Error("no model");
    const changes = setAlignment(model, 1, "center");
    expect(changes).toHaveLength(1);
    expect(apply(SPAN, changes ?? [])).toBe(
      ["| a | b |", "| --- | :---: |", "| 1 | 2 |"].join("\n"),
    );
  });
});

describe("prettify", () => {
  test("pads pipes to column width, cell text verbatim", () => {
    const span = "| name | n |\n| :--- | ---: |\n| a much longer cell | 2 |";
    const model = parseTable(span);
    if (!model) throw new Error("no model");
    const pretty = apply(span, prettify(model, "\n"));
    expect(pretty).toBe(
      [
        "| name               | n    |",
        "| :----------------- | ---: |",
        "| a much longer cell |    2 |",
      ].join("\n"),
    );
  });

  test("an already-pretty table emits no changes", () => {
    // Genuinely canonical under the plan's own padded style: every cell is
    // exactly the column width (3, which is also the rule row's minimum), so
    // all three lines are pipe-aligned at 13 characters and a correct
    // prettify has nothing to do. (The previous fixture here was NOT
    // canonical - unequal cell widths - and would have failed the correct
    // implementation; the review's recomputation is the check to repeat if
    // this fixture is ever edited.)
    const span = "| aaa | bbb |\n| --- | --- |\n| 111 | 222 |";
    const model = parseTable(span);
    if (!model) throw new Error("no model");
    expect(prettify(model, "\n")).toHaveLength(0);
  });

  test("the header pads left while data follows the column alignment", () => {
    // The shipped canonical form, pinned: a centered column centers its data
    // cells and stretches its colons, and the header cell pads left whatever
    // the column's alignment is (the plan's own prettify fixture, where a
    // right-aligned column's header "n" pads right while its data "2" pads
    // left).
    const span = "| h | v |\n| --- | :---: |\n| x | longer |";
    const model = parseTable(span);
    if (!model) throw new Error("no model");
    expect(apply(span, prettify(model, "\n"))).toBe(
      ["| h   | v      |", "| --- | :----: |", "| x   | longer |"].join("\n"),
    );
  });

  test("an escaped pipe is content, and counts toward its column's width", () => {
    const span = "| a \\| b | c |\n| --- | --- |\n| 1 | 2 |";
    const model = parseTable(span);
    if (!model) throw new Error("no model");
    expect(apply(span, prettify(model, "\n"))).toBe(
      ["| a \\| b | c   |", "| ------ | --- |", "| 1      | 2   |"].join("\n"),
    );
  });
});

describe("tolerances the verbs inherit from the parse", () => {
  test("a CRLF span parses and a new row carries the CRLF separator", () => {
    const span = "| a | b |\r\n| --- | --- |\r\n| 1 | 2 |";
    const model = parseTable(span);
    if (!model) throw new Error("no model");
    expect(model.columns).toBe(2);
    expect(model.lines[1]?.trailingPipe).toBe(true);
    expect(apply(span, addRowBelow(model, 2, "\r\n") ?? [])).toBe(
      `${span}\r\n|  |  |`,
    );
  });

  test("an indented table keeps its indent in the row it gains", () => {
    const span = "  | a | b |\n  | --- | --- |\n  | 1 | 2 |";
    const model = parseTable(span);
    if (!model) throw new Error("no model");
    expect(apply(span, addRowBelow(model, 2, "\n") ?? [])).toBe(
      `${span}\n  |  |  |`,
    );
  });

  test("deleting a middle row takes exactly one separator with it", () => {
    const span = "| a |\n| --- |\n| 1 |\n| 2 |";
    const model = parseTable(span);
    if (!model) throw new Error("no model");
    expect(apply(span, deleteRow(model, 2, "\n") ?? [])).toBe(
      "| a |\n| --- |\n| 2 |",
    );
  });

  test("a column appended past an edge-pipeless row closes it with a pipe", () => {
    const span = "a | b\n--- | ---\n1 | 2";
    const model = parseTable(span);
    if (!model) throw new Error("no model");
    expect(apply(span, addColumnAfter(model, 1) ?? [])).toBe(
      ["a | b | Column |", "--- | --- | --- |", "1 | 2 |  |"].join("\n"),
    );
  });

  test("out-of-range columns and rows are refused", () => {
    const model = parseTable(SPAN);
    if (!model) throw new Error("no model");
    expect(addColumnAfter(model, 2)).toBeNull();
    expect(deleteColumn(model, 2)).toBeNull();
    expect(setAlignment(model, 2, "left")).toBeNull();
    expect(addRowBelow(model, 3, "\n")).toBeNull();
    expect(deleteRow(model, 3, "\n")).toBeNull();
  });
});
