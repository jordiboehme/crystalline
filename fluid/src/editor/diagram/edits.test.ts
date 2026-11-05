import { EditorState } from "@codemirror/state";
import { describe, expect, test } from "vitest";

import { onlyEditedBytes, untouchedLinesEqual } from "../../test/fidelity";
import { docText, lineSeparatorFor } from "../setup";
import { NOTICES, type TextEdit, applyEdits, toChanges } from "./edits";

describe("applyEdits", () => {
  test("a line break in an insert is a new line", () => {
    expect(applyEdits("a\nb", [{ from: 1, to: 1, insert: "\nc" }])).toBe(
      "a\nc\nb",
    );
  });

  test("edits apply in source order whatever order they are given in", () => {
    const edits: TextEdit[] = [
      { from: 4, to: 5, insert: "Z" },
      { from: 0, to: 1, insert: "X" },
    ];
    expect(applyEdits("abcdef", edits)).toBe("XbcdZf");
  });

  test("an insert at the start of a replaced range goes before it", () => {
    const edits: TextEdit[] = [
      { from: 1, to: 3, insert: "R" },
      { from: 1, to: 1, insert: "I" },
    ];
    expect(applyEdits("abcd", edits)).toBe("aIRd");
  });

  test("touching ranges are fine, overlapping ones throw", () => {
    expect(
      applyEdits("abcd", [
        { from: 0, to: 2, insert: "x" },
        { from: 2, to: 4, insert: "y" },
      ]),
    ).toBe("xy");
    expect(() =>
      applyEdits("abcd", [
        { from: 0, to: 3, insert: "x" },
        { from: 2, to: 4, insert: "y" },
      ]),
    ).toThrow(/overlap/);
    expect(() =>
      applyEdits("abcd", [
        { from: 0, to: 4, insert: "x" },
        { from: 2, to: 2, insert: "y" },
      ]),
    ).toThrow(/overlap/);
  });

  test("a range outside the body or backwards throws", () => {
    expect(() => applyEdits("ab", [{ from: 1, to: 3, insert: "" }])).toThrow();
    expect(() => applyEdits("ab", [{ from: -1, to: 0, insert: "" }])).toThrow();
    expect(() => applyEdits("ab", [{ from: 2, to: 1, insert: "" }])).toThrow();
  });
});

describe("toChanges", () => {
  test("shifts by the body start and writes the document's line break", () => {
    expect(toChanges(10, [{ from: 1, to: 1, insert: "\nx" }], "\r\n")).toEqual([
      { from: 11, to: 11, insert: "\r\nx" },
    ]);
  });

  test("keeps LF inserts as they are and sorts like applyEdits", () => {
    expect(
      toChanges(
        3,
        [
          { from: 4, to: 5, insert: "a\nb" },
          { from: 0, to: 1, insert: "" },
        ],
        "\n",
      ),
    ).toEqual([
      { from: 3, to: 4, insert: "" },
      { from: 7, to: 8, insert: "a\nb" },
    ]);
    expect(() =>
      toChanges(
        0,
        [
          { from: 0, to: 3, insert: "" },
          { from: 1, to: 2, insert: "" },
        ],
        "\n",
      ),
    ).toThrow(/overlap/);
  });

  test("on a CRLF state the dispatched body is applyEdits of the body", () => {
    const text =
      "intro\r\n```mermaid\r\nflowchart TD\r\n    A --> B\r\n```\r\n";
    const state = EditorState.create({
      doc: text,
      extensions: lineSeparatorFor(text),
    });
    const bodyFrom = state.doc.line(3).from;
    const bodyTo = state.doc.line(4).to;
    const body = state.doc.sliceString(bodyFrom, bodyTo);
    const edits: TextEdit[] = [
      { from: body.length, to: body.length, insert: "\n    B --> C" },
      { from: 17, to: 18, insert: "Start" },
    ];
    const next = state.update({
      changes: toChanges(bodyFrom, edits, state.lineBreak),
    }).state;
    const expected = applyEdits(body, edits);
    expect(next.doc.sliceString(bodyFrom, bodyFrom + expected.length)).toBe(
      expected,
    );
    expect(docText(next)).toBe(
      "intro\r\n```mermaid\r\nflowchart TD\r\n    Start --> B\r\n    B --> C\r\n```\r\n",
    );
  });
});

describe("NOTICES", () => {
  test("hold the exact texts", () => {
    expect(NOTICES).toEqual({
      stale: "The diagram just changed. Try again.",
      gone: "That part of the diagram is no longer there.",
      invalid: "That change would break the diagram, so it was not made.",
      group:
        "This arrow is drawn together with others, so it cannot be changed on its own.",
      chain:
        "This arrow is part of a chain, so it cannot be reversed on its own.",
      unsupported:
        "This diagram uses something the editor does not know yet, so it was not changed.",
      emptiesBlock:
        "That would leave a block with nothing in it, so it was not deleted.",
      leavesActive:
        "That would leave an activation open, so it was not deleted.",
    });
  });

  test("none mentions Raw or text", () => {
    for (const notice of Object.values(NOTICES)) {
      expect(notice).not.toMatch(/\b(raw|text)\b/i);
    }
  });
});

describe("fidelity helpers", () => {
  const before = "flowchart TD\n    A --> B\n    B --> C";

  test("hold for an honest edit", () => {
    const edits: TextEdit[] = [{ from: 17, to: 18, insert: "Start" }];
    const after = applyEdits(before, edits);
    expect(onlyEditedBytes(before, after, edits)).toBe(true);
    expect(untouchedLinesEqual(before, after, edits)).toEqual([]);
  });

  test("hold for an insert of a new line at a line end", () => {
    const edits: TextEdit[] = [
      { from: 12, to: 12, insert: "\n    %% note" },
      { from: before.length, to: before.length, insert: "\n    C --> D" },
    ];
    const after = applyEdits(before, edits);
    expect(onlyEditedBytes(before, after, edits)).toBe(true);
    expect(untouchedLinesEqual(before, after, edits)).toEqual([]);
  });

  test("catch a changed byte outside the edits", () => {
    const edits: TextEdit[] = [{ from: 17, to: 18, insert: "Start" }];
    const after = applyEdits(before, edits).replace("B --> C", "B --> X");
    expect(onlyEditedBytes(before, after, edits)).toBe(false);
    expect(untouchedLinesEqual(before, after, edits)).toEqual(["    B --> C"]);
  });

  test("catch a lost line", () => {
    const edits: TextEdit[] = [{ from: 17, to: 18, insert: "Start" }];
    const after = applyEdits(before, edits).replace("\n    B --> C", "");
    expect(onlyEditedBytes(before, after, edits)).toBe(false);
    expect(untouchedLinesEqual(before, after, edits)).toEqual(["    B --> C"]);
  });
});
