import { EditorState } from "@codemirror/state";
import { describe, expect, test } from "vitest";

import { onlyEditedBytes, untouchedLinesEqual } from "../../test/fidelity";
import { docText, lineSeparatorFor } from "../setup";
import {
  NOTICES,
  REASONS,
  type RefusalCode,
  type TextEdit,
  applyEdits,
  reasonFor,
  toChanges,
} from "./edits";

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
  test("with the body, a deletion of the last lines takes the last line's break instead of the first's", () => {
    const body = "a\nb\nc";
    expect(toChanges(10, [{ from: 3, to: 5, insert: "" }], "\n", body)).toEqual(
      [{ from: 14, to: 16, insert: "" }],
    );
    expect(toChanges(10, [{ from: 1, to: 5, insert: "" }], "\n", body)).toEqual(
      [{ from: 12, to: 16, insert: "" }],
    );
  });

  test("with the body, any other edit stays where it is", () => {
    const body = "a\nb\nc;d";
    for (const edit of [
      { from: 1, to: 3, insert: "" }, // not to the body's end
      { from: 3, to: 7, insert: "x" }, // not a pure deletion
      { from: 5, to: 7, insert: "" }, // does not start at a line break
    ]) {
      expect(toChanges(10, [edit], "\n", body)).toEqual([
        { from: 10 + edit.from, to: 10 + edit.to, insert: edit.insert },
      ]);
    }
  });

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
      invalid: "That change would break the diagram, so nothing was changed.",
      group:
        "This arrow is drawn together with others, so it cannot be changed on its own.",
      chain:
        "This arrow is part of a chain, so it cannot be reversed on its own.",
      unsupported:
        "This diagram uses something the editor does not know yet, so nothing was changed.",
      unchecked:
        "The diagram could not be checked, so nothing was changed. Try again.",
      breaksFence: "That would break a diagram here, so nothing was changed.",
      needsLineBelow:
        "A diagram at the end of a page needs a line below it, so nothing was changed.",
      arrowChanged:
        "This arrow changed, so the label was not written. Press Escape and open it again.",
      messageChanged:
        "This message changed, so what you typed was not written. Press Escape and open it again.",
      emptiesBlock:
        "That would leave a block with nothing in it, so nothing was changed.",
      leavesActive:
        "That would leave an activation open, so nothing was changed.",
      participantEnded:
        "A participant that has ended can take no more messages, so nothing was changed.",
      keptBox:
        "The new box stays. Press Delete, or undo once, to take it back.",
      keptParticipant:
        "The new participant stays. Press Delete, or undo once, to take it back.",
      keptMessage:
        "The new message stays. Press Delete, or undo once, to take it back.",
    });
  });

  test("a disabled item's reasons say what would happen, not what was done", () => {
    expect(REASONS).toEqual({
      invalid: "That change would break the diagram.",
      unsupported: "This diagram uses something the editor does not know yet.",
      emptiesBlock: "That would leave a block with nothing in it.",
      leavesActive: "That would leave an activation open.",
      participantEnded:
        "A participant that has ended can take no more messages.",
    });
    // Every code an op's compile can answer, which is what a menu shows ahead.
    const compiled: RefusalCode[] = [
      "gone",
      "invalid",
      "group",
      "chain",
      "unsupported",
      "emptiesBlock",
      "leavesActive",
      "participantEnded",
    ];
    for (const code of compiled) {
      const reason = reasonFor(code);
      expect(reason, code).not.toMatch(/nothing was changed/);
      expect(reason, code).not.toMatch(/\b(raw|text)\b/i);
    }
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
