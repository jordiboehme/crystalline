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
 *
 * One suite goes further and drives a real `EditorState`: the module is pure,
 * but the convention for CALLING it is not free of CodeMirror, and a CRLF
 * document is where a wrong convention shows. That suite is the executable
 * statement of it - span read with `doc.sliceString`, separator passed as
 * `state.lineBreak`, offsets mapped straight onto document positions.
 */

import { EditorState } from "@codemirror/state";
import { describe, expect, test } from "vitest";

import type { SpanChange } from "./tableModel";
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

  test("a deletion that would empty a line is refused whole", () => {
    // Edge-pipeless AND ragged to a single cell: that cell IS column 0, so
    // deleting the column consumes the line down to nothing. A blank line ENDS
    // a GFM table, so the emission would split this one in two and the rows
    // below it would stop being a table - a structurally broken document from
    // one segment click. The verb refuses instead, through the channel it
    // already has.
    const span = ["a | b | c", "--- | --- | ---", "1", "x | y | z"].join("\n");
    const model = parseTable(span);
    if (!model) throw new Error("no model");
    expect(deleteColumn(model, 0)).toBeNull();
    // Only that column: the refusal is about the line the deletion would
    // empty, not about the table's shape, so the ragged row's absent columns
    // are still deletable and it is left alone as ever. The space each line
    // keeps is the one that preceded the pipe the deletion took with the last
    // cell - trailing whitespace, which changes no cell's content.
    expect(apply(span, deleteColumn(model, 2) ?? [])).toBe(
      ["a | b ", "--- | --- ", "1", "x | y "].join("\n"),
    );
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

describe("the calling convention on a CRLF document", () => {
  /** A table with content on both sides of it, in a real CRLF buffer. */
  const DOC =
    "Before\r\n\r\n| a | b |\r\n| --- | --- |\r\n| 1 | 2 |\r\n\r\nAfter\r\n";
  const TABLE = { from: 8, to: 41 };

  function crlfState(): EditorState {
    return EditorState.create({
      doc: DOC,
      extensions: [EditorState.lineSeparator.of("\r\n")],
    });
  }

  /** The sanctioned read: `doc.sliceString`, never `state.sliceDoc`. */
  function tableSpan(state: EditorState): string {
    return state.doc.sliceString(TABLE.from, TABLE.to);
  }

  /** The mapping Task 2 performs: span offsets are document offsets. */
  function dispatch(
    state: EditorState,
    changes: SpanChange[] | null,
  ): EditorState {
    if (!changes) throw new Error("refused");
    return state.update({
      changes: changes.map((change) => {
        const from = TABLE.from + change.from;
        const to = TABLE.from + (change.to ?? change.from);
        return change.insert === undefined
          ? { from, to }
          : { from, to, insert: change.insert };
      }),
    }).state;
  }

  test("doc.sliceString gives document offsets where sliceDoc inflates them", () => {
    const state = crlfState();
    expect(tableSpan(state)).toBe(SPAN);
    // The read that must NOT be used: it re-joins with the two-character
    // break, so its string is longer than the range it came from and every
    // offset past the first line is wrong by one per break.
    expect(state.sliceDoc(TABLE.from, TABLE.to)).toHaveLength(
      TABLE.to - TABLE.from + 2,
    );
    const model = parseTable(tableSpan(state));
    expect(model?.lines[2]?.start).toBe(state.doc.lineAt(32).from - TABLE.from);
  });

  test("every verb edits the right characters and keeps the breaks", () => {
    const state = crlfState();
    const model = parseTable(tableSpan(state));
    if (!model) throw new Error("no model");
    const separator = state.lineBreak;

    expect(dispatch(state, addRowBelow(model, 2, separator)).sliceDoc()).toBe(
      "Before\r\n\r\n| a | b |\r\n| --- | --- |\r\n| 1 | 2 |\r\n|  |  |\r\n\r\nAfter\r\n",
    );
    // The last row's break comes from the model's own line spans: taking it
    // from the separator's LENGTH ate the delimiter's closing pipe here.
    expect(dispatch(state, deleteRow(model, 2, separator)).sliceDoc()).toBe(
      "Before\r\n\r\n| a | b |\r\n| --- | --- |\r\n\r\nAfter\r\n",
    );
    expect(dispatch(state, addColumnAfter(model, 0)).sliceDoc()).toBe(
      "Before\r\n\r\n| a | Column | b |\r\n| --- | --- | --- |\r\n| 1 |  | 2 |\r\n\r\nAfter\r\n",
    );
    expect(dispatch(state, deleteColumn(model, 0)).sliceDoc()).toBe(
      "Before\r\n\r\n| b |\r\n| --- |\r\n| 2 |\r\n\r\nAfter\r\n",
    );
    expect(dispatch(state, setAlignment(model, 1, "center")).sliceDoc()).toBe(
      "Before\r\n\r\n| a | b |\r\n| --- | :---: |\r\n| 1 | 2 |\r\n\r\nAfter\r\n",
    );
    expect(dispatch(state, prettify(model, separator)).sliceDoc()).toBe(
      "Before\r\n\r\n| a   | b   |\r\n| --- | --- |\r\n| 1   | 2   |\r\n\r\nAfter\r\n",
    );
  });

  test("the new row's caret is two characters into the line below the change", () => {
    const state = crlfState();
    const model = parseTable(tableSpan(state));
    if (!model) throw new Error("no model");
    const changes = addRowBelow(model, 2, state.lineBreak);
    const change = changes?.[0];
    if (!change) throw new Error("refused");

    const next = dispatch(state, changes);
    const inserted = next.doc.lineAt(TABLE.from + change.from + 1);
    const caret = inserted.from + (model.lines[2]?.indent.length ?? 0) + 2;
    expect(inserted.text).toBe("|  |  |");
    expect(caret - inserted.from).toBe(2);
    expect(next.sliceDoc(caret - 2, caret + 2)).toBe("|  |");
    // Why the formula is stated against the new LINE rather than against the
    // separator: a break costs one position whatever it spells, so the
    // separator-length spelling lands one character too far here.
    expect(
      TABLE.from + change.from + state.lineBreak.length + 2 - inserted.from,
    ).toBe(3);
  });
});

describe("tolerances the verbs inherit from the parse", () => {
  test("a raw CR-bearing slice still parses, as a safety net", () => {
    // Not a sanctioned read - the verbs' offsets are only document offsets
    // under the LF-joined convention above - but a stray CR must not explode.
    const model = parseTable("| a | b |\r\n| --- | --- |\r\n| 1 | 2 |");
    expect(model?.columns).toBe(2);
    expect(model?.lines[1]?.trailingPipe).toBe(true);
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
