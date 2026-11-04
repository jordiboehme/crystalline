/**
 * The table assist verbs, exercised on real views: what the toolbar's context
 * segment runs, against the syntax tree the rest of the editor already trusts.
 *
 * The detection edges are the load-bearing part. A caret at the table's very
 * first character resolves OUTSIDE the table on side -1 and is only found by
 * the side 1 retry, and the position just past its last character is only
 * found by side -1 - an ordinary Home-key caret and an ordinary End-key caret,
 * one on each side of the implementation's blind spots.
 */

import { undo } from "@codemirror/commands";
import { syntaxTree } from "@codemirror/language";
import { Compartment, EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, test, vi } from "vitest";
import { yCollab } from "y-codemirror.next";
import { Awareness } from "y-protocols/awareness";
import * as Y from "yjs";

import { parsedState } from "../test/parse";
import { PIPELESS_FIXTURES, tableStart } from "../test/pipelessTables";
import { cellOf, cleanupCells, openAt, soloMain } from "../test/cellKit";
import { cellForwarding } from "./grid/cellEditor";
import { gridLoaded } from "./grid/draws";
import { gridExtension } from "./grid/index";
import type { Align } from "./tableModel";
import { parseTable } from "./tableModel";
import { baseExtensions, docText, lineSeparatorFor } from "./setup";
import {
  tableAddColumnAfter,
  tableAddColumnBefore,
  tableAddRowAbove,
  tableAddRowBelow,
  tableAlignColumn,
  tableContextAt,
  tableContextListener,
  tableDeleteColumn,
  tableDeleteRow,
  tableMoveColumn,
  tableMoveRow,
  tablePrettify,
} from "./tableVerbs";

const DOC = "Before\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\nAfter\n";
const IN_TABLE = DOC.indexOf("| 1") + 2; // inside the "1" cell
let view: EditorView | null = null;
afterEach(() => {
  view?.destroy();
  view = null;
});

function editor(
  doc: string,
  anchor: number,
  extra: unknown[] = [],
): EditorView {
  view = new EditorView({
    // `parsedState`, because a new state's first parse is cut off after 20ms of
    // wall clock and a half-parsed table is no table at all here.
    state: parsedState(
      EditorState.create({
        doc,
        selection: EditorSelection.cursor(anchor),
        // baseExtensions carries the markdown language: the syntax tree is
        // what decides what a table is, exactly as fencePreviews trusts it.
        extensions: [
          ...lineSeparatorFor(doc),
          ...baseExtensions(false),
          ...(extra as never[]),
        ],
      }),
    ),
    parent: document.body,
  });
  return view;
}

describe("tableContextAt", () => {
  test("resolves the cell under the caret", () => {
    const v = editor(DOC, IN_TABLE);
    const context = tableContextAt(v.state);
    expect(context).not.toBeNull();
    expect(context?.row).toBe(2);
    expect(context?.column).toBe(0);
  });

  test("prose, fences and a pipe line inside a fence are not tables", () => {
    const v = editor(DOC, 2);
    expect(tableContextAt(v.state)).toBeNull();
    const fenced = editor("```text\n| a | b |\n|---|---|\n```\n", 12);
    expect(tableContextAt(fenced.state)).toBeNull();
  });

  test("the caret at the table's FIRST character is in the table", () => {
    // Load-bearing edge, probed on the real parser: at the first character
    // resolveInner(pos, -1) resolves OUTSIDE the table and only the side 1
    // retry finds it. A side -1-only implementation passes every interior
    // test and fails this ordinary Home-key caret position.
    const v = editor(DOC, DOC.indexOf("| a"));
    const context = tableContextAt(v.state);
    expect(context).not.toBeNull();
    expect(context?.row).toBe(0);
    expect(context?.column).toBe(0);
  });

  test("the boundary just past the table's last character is still in it", () => {
    // Side -1 carries this edge; one position further (the next line's
    // start) is the null case the prose test above already covers.
    const v = editor(DOC, DOC.indexOf("| 1 | 2 |") + "| 1 | 2 |".length);
    expect(tableContextAt(v.state)).not.toBeNull();
  });

  test("the delimiter row is row 1, and it is not a row to delete", () => {
    const v = editor(DOC, DOC.indexOf("| --- ") + 2);
    expect(tableContextAt(v.state)?.row).toBe(1);
    // The rule is structure, not content: deleting it would stop the table
    // being a table, so the verb refuses rather than obliging.
    expect(tableDeleteRow(v)).toBe(false);
    expect(docText(v.state)).toBe(DOC);
  });

  test("two tables a blank line apart do not leak into each other", () => {
    // What a naive line loop breaks silently: the second table's caret must
    // resolve to the SECOND span, and a verb must edit only that one.
    const doc = "| a | b |\n| --- | --- |\n\n| c | d |\n| --- | --- |\n";
    const first = tableContextAt(editor(doc, 2).state);
    const second = tableContextAt(editor(doc, doc.indexOf("| c") + 2).state);
    expect(first?.from).toBe(0);
    expect(second?.from).toBe(doc.indexOf("| c"));
    expect(
      tableContextAt(editor(doc, doc.indexOf("\n\n") + 1).state),
    ).toBeNull();

    const v = editor(doc, doc.indexOf("| c") + 2);
    expect(tableAddColumnAfter(v)).toBe(true);
    expect(docText(v.state)).toBe(
      "| a | b |\n| --- | --- |\n\n| c | Column | d |\n| --- | --- | --- |\n",
    );
  });

  test("a table carrying a quote mark inside its span is refused", () => {
    // CommonMark's lazy continuation: the middle line may drop the `> ` and
    // still belong to the quote, so the parser opens the table after the
    // FIRST line's mark and the third line's mark sits inside the span. The
    // model would read that mark as cell content - the caret in `1` would
    // report column 1 - so the whole table is refused instead.
    const doc = "> | a | b |\n| --- | --- |\n> | 1 | 2 |\n";
    const v = editor(doc, doc.indexOf("| 1") + 3);
    expect(tableContextAt(v.state)).toBeNull();
    expect(tableAddColumnAfter(v)).toBe(false);
    expect(tablePrettify(v)).toBe(false);
    expect(docText(v.state)).toBe(doc);
  });

  test("the quote guard costs neither the lazy table nor a pipe-less one", () => {
    // The boundary the guard is drawn at. A quote whose mark is written once
    // leaves the span clean, so it keeps working; and the question is asked of
    // the TREE rather than of the text, so a GFM table that drops its leading
    // pipes - where every line has content before its first pipe - is still an
    // ordinary table here.
    const lazy = "> | a | b |\n| --- | --- |\n| 1 | 2 |\n";
    const quoted = editor(lazy, lazy.indexOf("| 1") + 2);
    expect(tableAddColumnAfter(quoted)).toBe(true);
    expect(docText(quoted.state)).toBe(
      "> | a | Column | b |\n| --- | --- | --- |\n| 1 |  | 2 |\n",
    );

    const bare = "abc | def\n--- | ---\nbar | baz\n";
    const v = editor(bare, bare.indexOf("bar") + 1);
    expect(tableContextAt(v.state)?.column).toBe(0);
    expect(tableAddColumnAfter(v)).toBe(true);
    expect(docText(v.state)).toBe(
      "abc | Column | def\n--- | --- | ---\nbar |  | baz\n",
    );
  });
});

describe("the verbs", () => {
  test("add column after dispatches ONE transaction, one undo step", () => {
    let transactions = 0;
    const v = editor(DOC, IN_TABLE, [
      EditorView.updateListener.of((u) => {
        if (u.docChanged) transactions += 1;
      }),
    ]);
    expect(tableAddColumnAfter(v)).toBe(true);
    expect(transactions).toBe(1);
    expect(docText(v.state)).toContain("| a | Column | b |");
    // One undo restores the document exactly: the userEvent "input" tag keeps
    // the verb its own undo step, never joined into adjacent typing.
    undo(v);
    expect(docText(v.state)).toBe(DOC);
  });

  test("outside a table every verb refuses without touching the doc", () => {
    const v = editor(DOC, 2);
    expect(tableAddRowBelow(v)).toBe(false);
    expect(tableAddColumnAfter(v)).toBe(false);
    expect(tableDeleteRow(v)).toBe(false);
    expect(tableDeleteColumn(v)).toBe(false);
    expect(tableAlignColumn(v, "center")).toBe(false);
    expect(tablePrettify(v)).toBe(false);
    expect(docText(v.state)).toBe(DOC);
  });

  test("delete row takes the caret's row, align takes its column", () => {
    const v = editor(DOC, IN_TABLE);
    expect(tableAlignColumn(v, "center")).toBe(true);
    // The caret is in the first column, so it is the first rule cell that
    // gains the colons and the second that does not.
    expect(docText(v.state)).toContain("| :---: | --- |");
    expect(tableDeleteRow(v)).toBe(true);
    expect(docText(v.state)).toBe(
      "Before\n\n| a | b |\n| :---: | --- |\n\nAfter\n",
    );
  });

  test("add row below lands the caret in the new row's first cell", () => {
    const v = editor(DOC, IN_TABLE);
    expect(tableAddRowBelow(v)).toBe(true);
    const line = v.state.doc.lineAt(v.state.selection.main.head);
    expect(line.text).toMatch(/^\|\s+\|\s+\|$/);
    // The OFFSET, not just the line: a caret at position 0 sits before the
    // pipe and typing would prepend outside the cell. "|  |  |" puts the
    // first cell's interior at line.from + 2.
    expect(v.state.selection.main.head).toBe(line.from + 2);
  });

  test("prettify pads the whole table in one transaction", () => {
    const doc = "| name | n |\n| --- | --- |\n| longer | 2 |\n";
    const v = editor(doc, doc.indexOf("longer"));
    expect(tablePrettify(v)).toBe(true);
    // The second column pads to THREE, not to one: a column is never narrower
    // than the rule cell GFM requires, which for an unaligned column is the
    // model's `minRuleWidth("none")`, three dashes. (Its own `| :--- | ---: |`
    // fixture shows the same floor one wider, where a colon joins the dashes.)
    // The whole table is asserted rather than a fragment, so the width rule is
    // stated where a future reader can check it.
    expect(docText(v.state)).toBe(
      "| name   | n   |\n| ------ | --- |\n| longer | 2   |\n",
    );
  });

  test("a CRLF buffer is edited at the right places", () => {
    // The dispatch layer's own statement of the calling convention, which is
    // where a wrong one shows: the span is read with `doc.sliceString`, so a
    // span offset IS a document offset and every insertion past the first
    // line break lands where it was meant to. Read with `sliceDoc` instead,
    // the string would be longer than the range it came from and the second
    // and third insertions would drift by a character each.
    const doc = "Before\r\n\r\n| 1 | 2 |\r\n| --- | --- |\r\n| a | b |\r\n";
    const v = editor(doc, 0);
    // The caret goes in through the document's own line API rather than
    // through a string index: `doc.indexOf` counts each CRLF as two where the
    // document counts one, so a string offset is not a position at all here.
    v.dispatch({
      selection: EditorSelection.cursor(v.state.doc.line(5).from + 2),
    });
    expect(tableAddColumnAfter(v)).toBe(true);
    expect(docText(v.state)).toBe(
      "Before\r\n\r\n| 1 | Column | 2 |\r\n| --- | --- | --- |\r\n| a |  | b |\r\n",
    );
    // And the caret the new row gets is measured on the line the change made,
    // never from the separator's length: two characters past the line start
    // whether that break spelled one character or two.
    expect(tableAddRowBelow(v)).toBe(true);
    const line = v.state.doc.lineAt(v.state.selection.main.head);
    expect(line.text).toBe("|  |  |  |");
    expect(v.state.selection.main.head).toBe(line.from + 2);
  });
});

describe("tableContextListener", () => {
  test("fires only on transitions", () => {
    const seen: boolean[] = [];
    const v = editor(DOC, 2, [
      tableContextListener((context) => seen.push(context.inTable)),
    ]);
    v.dispatch({ selection: EditorSelection.cursor(IN_TABLE) });
    v.dispatch({ selection: EditorSelection.cursor(IN_TABLE + 1) }); // still inside: no event
    v.dispatch({ selection: EditorSelection.cursor(2) });
    expect(seen).toEqual([true, false]);
  });
});

describe("tableContextListener and the grid", () => {
  const QUOTED = "- item\n\n  | a | b |\n  | - | - |\n  | 1 | 2 |\n\nAfter\n";

  function seenBy(doc: string, at: number, grid: boolean) {
    const seen: { inTable: boolean; drawn: boolean }[] = [];
    const v = editor(doc, 2, [
      ...(grid ? [gridLoaded.of(true)] : []),
      tableContextListener((context) => seen.push(context)),
    ]);
    v.dispatch({ selection: EditorSelection.cursor(at) });
    return seen;
  }

  test("a table the loaded grid draws reports drawn", () => {
    expect(seenBy(DOC, IN_TABLE, true).at(-1)).toEqual({
      inTable: true,
      drawn: true,
    });
  });

  test("without the grid loaded (Raw, a MANIFEST) nothing is drawn", () => {
    expect(seenBy(DOC, IN_TABLE, false).at(-1)).toEqual({
      inTable: true,
      drawn: false,
    });
  });

  test("a table in a list item is in a table the grid does not draw", () => {
    expect(seenBy(QUOTED, QUOTED.indexOf("1 |"), true).at(-1)).toEqual({
      inTable: true,
      drawn: false,
    });
  });

  test("the grid loading while the caret rests in a table reports again", () => {
    const seen: { inTable: boolean; drawn: boolean }[] = [];
    const slot = new Compartment();
    const v = editor(DOC, IN_TABLE, [
      slot.of([]),
      tableContextListener((context) => seen.push(context)),
    ]);
    v.dispatch({ selection: EditorSelection.cursor(IN_TABLE + 1) });
    v.dispatch({ effects: slot.reconfigure(gridLoaded.of(true)) });
    expect(seen.at(-1)).toEqual({ inTable: true, drawn: true });
  });
});

describe("in a room", () => {
  test("a table verb reaches the shared text exactly once", () => {
    // The T11 harness, verbatim shape: yCollab sees one local edit.
    const ydoc = new Y.Doc();
    const ytext = ydoc.getText("content");
    ytext.insert(0, DOC);
    const awareness = new Awareness(ydoc);
    view = new EditorView({
      state: parsedState(
        EditorState.create({
          doc: ytext.toJSON(),
          selection: EditorSelection.cursor(IN_TABLE),
          extensions: [
            EditorState.lineSeparator.of("\n"),
            ...baseExtensions(false),
            yCollab(ytext, awareness),
          ],
        }),
      ),
      parent: document.body,
    });
    expect(tableAddColumnAfter(view)).toBe(true);
    const shared = ytext.toJSON(); // Y.Text read: sanctioned, LF space by design
    expect(shared.split("| a | Column | b |").length - 1).toBe(1);
    expect(shared).toBe(docText(view.state));
  });
});

describe("insert-before and move verbs, and the `at` position", () => {
  const TWO =
    "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |\n\nmiddle\n\n| x | y |\n| --- | --- |\n| 5 | 6 |\n";
  const FIRST = TWO.indexOf("4"); // inside the "4" cell, second column
  const SECOND = TWO.indexOf("6"); // inside the "6" cell, second column

  const verbs: [string, (v: EditorView) => boolean, string][] = [
    [
      "add row above",
      (v) => tableAddRowAbove(v),
      TWO.replace("| 3 | 4 |", "|  |  |\n| 3 | 4 |"),
    ],
    [
      "add column before",
      (v) => tableAddColumnBefore(v),
      TWO.replace(
        "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |",
        "| a | Column | b |\n| --- | --- | --- |\n| 1 |  | 2 |\n| 3 |  | 4 |",
      ),
    ],
    [
      "move row up",
      (v) => tableMoveRow(v, -1),
      TWO.replace("| 1 | 2 |\n| 3 | 4 |", "| 3 | 4 |\n| 1 | 2 |"),
    ],
    [
      "move column left",
      (v) => tableMoveColumn(v, -1),
      TWO.replace(
        "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |",
        "| b | a |\n| --- | --- |\n| 2 | 1 |\n| 4 | 3 |",
      ),
    ],
  ];

  for (const [name, run, expected] of verbs) {
    test(`${name} is one transaction, one undo step, exact bytes back`, () => {
      let transactions = 0;
      const v = editor(TWO, FIRST, [
        EditorView.updateListener.of((u) => {
          if (u.docChanged) transactions += 1;
        }),
      ]);
      expect(run(v)).toBe(true);
      expect(transactions).toBe(1);
      expect(docText(v.state)).toBe(expected);
      undo(v);
      expect(docText(v.state)).toBe(TWO);
    });
  }

  test("the caret lands in the new or moved cell", () => {
    const v = editor(TWO, FIRST);
    expect(tableAddRowAbove(v)).toBe(true);
    let line = v.state.doc.lineAt(v.state.selection.main.head);
    expect(line.text).toBe("|  |  |");
    expect(v.state.selection.main.head).toBe(line.from + 2);

    const w = editor(TWO, FIRST);
    expect(tableMoveRow(w, -1)).toBe(true);
    line = w.state.doc.lineAt(w.state.selection.main.head);
    expect(line.text).toBe("| 3 | 4 |");
    expect(w.state.selection.main.head).toBe(line.from + 6);

    const c = editor(TWO, FIRST);
    expect(tableMoveColumn(c, -1)).toBe(true);
    line = c.state.doc.lineAt(c.state.selection.main.head);
    expect(line.text).toBe("| 4 | 3 |");
    expect(c.state.selection.main.head).toBe(line.from + 2);

    const b = editor(TWO, FIRST);
    expect(tableAddColumnBefore(b)).toBe(true);
    line = b.state.doc.lineAt(b.state.selection.main.head);
    expect(line.text).toBe("| 3 |  | 4 |");
    expect(b.state.selection.main.head).toBe(line.from + 6);
  });

  test("refusals leave the document alone", () => {
    const header = TWO.indexOf("| a") + 2;
    const v = editor(TWO, header);
    expect(tableAddRowAbove(v)).toBe(false);
    expect(tableMoveRow(v, 1)).toBe(false);
    const first = editor(TWO, TWO.indexOf("| 1") + 2);
    expect(tableMoveRow(first, -1)).toBe(false);
    expect(tableMoveColumn(first, -1)).toBe(false);
    expect(docText(first.state)).toBe(TWO);
  });

  test("a CRLF buffer is edited at the right places", () => {
    const doc =
      "x\r\n\r\n| a | b |\r\n| --- | --- |\r\n| 1 | 2 |\r\n| 3 | 4 |\r\n";
    const v = editor(doc, 0);
    v.dispatch({
      selection: EditorSelection.cursor(v.state.doc.line(6).from + 2),
    });
    expect(tableMoveRow(v, -1)).toBe(true);
    expect(docText(v.state)).toBe(
      "x\r\n\r\n| a | b |\r\n| --- | --- |\r\n| 3 | 4 |\r\n| 1 | 2 |\r\n",
    );
    expect(tableAddRowAbove(v)).toBe(true);
    expect(v.state.doc.lineAt(v.state.selection.main.head).text).toBe(
      "|  |  |",
    );
    expect(tableMoveColumn(v, 1)).toBe(true);
    expect(docText(v.state)).toBe(
      "x\r\n\r\n| b | a |\r\n| --- | --- |\r\n|  |  |\r\n| 4 | 3 |\r\n| 2 | 1 |\r\n",
    );
  });

  test("`at` names the cell, so a verb can edit a table the caret is not in", () => {
    const v = editor(TWO, FIRST);
    expect(tableAddRowBelow(v, SECOND)).toBe(true);
    expect(docText(v.state)).toBe(
      TWO.replace("| 5 | 6 |", "| 5 | 6 |\n|  |  |"),
    );
    undo(v);
    expect(tableDeleteColumn(v, SECOND)).toBe(true);
    expect(docText(v.state)).toBe(
      TWO.replace(
        "| x | y |\n| --- | --- |\n| 5 | 6 |",
        "| x |\n| --- |\n| 5 |",
      ),
    );
    undo(v);
    expect(tableAlignColumn(v, "right", SECOND)).toBe(true);
    expect(docText(v.state)).toContain("| --- | ---: |");
    undo(v);
    expect(tableMoveColumn(v, 1, SECOND)).toBe(false); // already the last column
    expect(tableAddColumnAfter(v, 2)).toBe(true);
    expect(docText(v.state)).toContain("| a | Column | b |");
    undo(v);
    expect(tableAddRowAbove(v, SECOND)).toBe(true);
    expect(tablePrettify(v, 2)).toBe(true);
    undo(v);
    undo(v);
    expect(tableDeleteRow(v, SECOND)).toBe(true);
    expect(docText(v.state)).toBe(TWO.replace("\n| 5 | 6 |", ""));
  });
});

describe("insert-before and move verbs in a room", () => {
  const ROOM = "| a | b |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |\n";

  function room(): {
    v: EditorView;
    ytext: Y.Text;
    undoManager: Y.UndoManager;
    shared: { transactions: number; observed: number };
  } {
    const ydoc = new Y.Doc();
    const ytext = ydoc.getText("content");
    ytext.insert(0, ROOM);
    const undoManager = new Y.UndoManager(ytext);
    const awareness = new Awareness(ydoc);
    // Counted after the room is seeded, so only the verb's own writes show.
    const shared = { transactions: 0, observed: 0 };
    ydoc.on("afterTransaction", (tr: Y.Transaction) => {
      if (tr.changed.size > 0) shared.transactions += 1;
    });
    ytext.observe(() => {
      shared.observed += 1;
    });
    view = new EditorView({
      state: parsedState(
        EditorState.create({
          doc: ytext.toJSON(),
          selection: EditorSelection.cursor(ROOM.indexOf("4")),
          extensions: [
            EditorState.lineSeparator.of("\n"),
            ...baseExtensions(false),
            yCollab(ytext, awareness, { undoManager }),
          ],
        }),
      ),
      parent: document.body,
    });
    return { v: view, ytext, undoManager, shared };
  }

  const verbs: [string, (v: EditorView) => boolean][] = [
    ["add row above", (v) => tableAddRowAbove(v)],
    ["add column before", (v) => tableAddColumnBefore(v)],
    ["move row up", (v) => tableMoveRow(v, -1)],
    ["move column left", (v) => tableMoveColumn(v, -1)],
  ];

  for (const [name, run] of verbs) {
    test(`${name} reaches the shared text once and one undo restores it`, () => {
      const { v, ytext, undoManager, shared } = room();
      expect(run(v)).toBe(true);
      // One verb is exactly one Y transaction touching the text, and the text
      // observer fires once for it.
      expect(shared.transactions).toBe(1);
      expect(shared.observed).toBe(1);
      expect(ytext.toJSON()).toBe(docText(v.state));
      expect(ytext.toJSON()).not.toBe(ROOM);
      // The verb is a single shared transaction, so one undo takes it all back.
      undoManager.stopCapturing();
      undoManager.undo();
      expect(ytext.toJSON()).toBe(ROOM);
      expect(docText(v.state)).toBe(ROOM);
    });
  }
});

describe("focus option", () => {
  const verbs: [string, (v: EditorView, focus?: boolean) => boolean][] = [
    ["add row below", (v, f) => tableAddRowBelow(v, IN_TABLE, opts(f))],
    ["add row above", (v, f) => tableAddRowAbove(v, IN_TABLE, opts(f))],
    ["add column after", (v, f) => tableAddColumnAfter(v, IN_TABLE, opts(f))],
    ["add column before", (v, f) => tableAddColumnBefore(v, IN_TABLE, opts(f))],
    ["delete column", (v, f) => tableDeleteColumn(v, IN_TABLE, opts(f))],
    ["align", (v, f) => tableAlignColumn(v, "center", IN_TABLE, opts(f))],
    ["prettify", (v, f) => tablePrettify(v, IN_TABLE, opts(f))],
  ];

  function opts(focus: boolean | undefined) {
    return focus === undefined ? {} : { focus };
  }

  test.each(verbs)("%s focuses the view by default", (_name, run) => {
    const v = editor(DOC, 0);
    const outside = document.createElement("input");
    document.body.appendChild(outside);
    outside.focus();
    expect(run(v)).toBe(true);
    expect(v.hasFocus).toBe(true);
    outside.remove();
  });

  test.each(verbs)(
    "%s leaves the focus alone with focus: false",
    (_name, run) => {
      const v = editor(DOC, 0);
      const outside = document.createElement("input");
      document.body.appendChild(outside);
      outside.focus();
      expect(run(v, false)).toBe(true);
      expect(v.hasFocus).toBe(false);
      expect(document.activeElement).toBe(outside);
      outside.remove();
    },
  );
});

/**
 * R23: every verb on a table without outer pipes (and on a mixed one) leaves
 * a table that every reader still takes for the same table: Lezer finds ONE
 * `Table` covering every row, the model reads each row with the table's
 * column count, and every row is a row to the server's merge too (it holds an
 * unescaped pipe and does not start another block). The page around the
 * table keeps its bytes.
 */
describe("every verb on tables without outer pipes (R23)", () => {
  type Run = (v: EditorView, at: number) => boolean;
  const off = { focus: false };
  const aligns: Align[] = ["left", "center", "right", "none"];
  const verbs: [string, Run][] = [
    ["add row below", (v, at) => tableAddRowBelow(v, at, off)],
    ["add row above", (v, at) => tableAddRowAbove(v, at, off)],
    ["add column after", (v, at) => tableAddColumnAfter(v, at, off)],
    ["add column before", (v, at) => tableAddColumnBefore(v, at, off)],
    ["delete row", (v, at) => tableDeleteRow(v, at, off)],
    ["delete column", (v, at) => tableDeleteColumn(v, at, off)],
    ["move row up", (v, at) => tableMoveRow(v, -1, at, off)],
    ["move row down", (v, at) => tableMoveRow(v, 1, at, off)],
    ["move column left", (v, at) => tableMoveColumn(v, -1, at, off)],
    ["move column right", (v, at) => tableMoveColumn(v, 1, at, off)],
    ...aligns.map((align): [string, Run] => [
      `align ${align}`,
      (v, at) => tableAlignColumn(v, align, at, off),
    ]),
  ];

  /** A merge row: an unescaped pipe, and no block start (merge.rs). */
  function mergeRow(text: string): boolean {
    const trimmed = text.trim();
    const pipe = /(^|[^\\])(\\\\)*\|/.test(trimmed);
    const block =
      /^[><]/.test(trimmed) ||
      /^#{1,6}(\s|$)/.test(trimmed) ||
      /^[-*+](\s|$)/.test(trimmed) ||
      /^\d{1,9}[.)](\s|$)/.test(trimmed) ||
      /^(`{3,}|~{3,})/.test(trimmed);
    return pipe && !block && !/^( {4}|\t)/.test(text);
  }

  /**
   * One table every reader agrees on, between the page's own `before` and
   * `after`; returns the model. `shape` also pins the row and column count.
   */
  function agree(
    state: EditorState,
    before: string,
    after: string,
    label: string,
    shape?: { lines: number; columns: number },
  ) {
    const text = docText(state);
    expect(text.startsWith(before), label).toBe(true);
    expect(text.endsWith(after), label).toBe(true);
    const span = text.slice(before.length, text.length - after.length);
    const model = parseTable(span);
    expect(model, `${label}: ${span}`).not.toBeNull();
    if (!model) return;
    expect(
      model.lines.map((l) => l.cells.length),
      `${label}: ${span}`,
    ).toEqual(model.lines.map(() => model.columns));
    if (shape) {
      expect([model.lines.length, model.columns], `${label}: ${span}`).toEqual([
        shape.lines,
        shape.columns,
      ]);
    }
    expect(
      model.lines.every((l) => mergeRow(l.text)),
      `${label}: ${span}`,
    ).toBe(true);
    const tables: [number, number][] = [];
    let rows = 0;
    syntaxTree(state).iterate({
      enter: (node) => {
        if (node.name === "Table") tables.push([node.from, node.to]);
        if (node.name === "TableRow") rows += 1;
      },
    });
    expect(tables, `${label}: ${span}`).toEqual([
      [before.length, before.length + span.length],
    ]);
    expect(rows, label).toBe(model.lines.length - 2);
  }

  /**
   * R24: what the cell editor and a replace may write into a cell, at its
   * start: nothing at all (the cell cleared) or a block marker.
   */
  const EDITS: [string, string | null][] = [
    ["clear", null],
    ...[
      "- ",
      "* ",
      "+ ",
      "1. ",
      "1) ",
      "# ",
      "###### ",
      "> ",
      "```",
      "~~~",
      "<b>",
      " ",
    ].map((prefix): [string, string] => [JSON.stringify(prefix), prefix]),
  ];

  for (const fixture of PIPELESS_FIXTURES) {
    const start = tableStart(fixture);
    const end = start + fixture.lines.join("\n").length;
    const before = fixture.text.slice(0, start);
    const after = fixture.text.slice(end);
    // A position inside each cell: the first character of its text.
    const positions: [string, number][] = [];
    fixture.cells.forEach((row, r) => {
      const lineIndex = r === 0 ? 0 : r + 1;
      const lineFrom =
        start +
        fixture.lines.slice(0, lineIndex).join("\n").length +
        (lineIndex === 0 ? 0 : 1);
      const line = fixture.lines[lineIndex] ?? "";
      let from = 0;
      row.forEach((text, c) => {
        const at = line.indexOf(text, from);
        from = at + text.length;
        positions.push([`${String(r)}/${String(c)}`, lineFrom + at]);
      });
    });

    describe(fixture.name, () => {
      afterEach(() => {
        cleanupCells();
      });

      /** Every cell's content range, in the fixture's document. */
      function contents(main: EditorView) {
        return fixture.cells.flatMap((row, r) =>
          row.map((_, c) => ({
            cell: `${String(r)}/${String(c)}`,
            ...cellOf(main, r, c).content,
          })),
        );
      }

      for (const [label, prefix] of EDITS) {
        test(`the cell editor writing ${label} in every cell keeps the table (R24)`, () => {
          const probe = soloMain(fixture.text);
          for (const { cell, from, to } of contents(probe)) {
            const main = soloMain(fixture.text, [cellForwarding]);
            const editor = openAt(main, to);
            editor.view.dispatch(
              prefix === null
                ? { changes: { from, to }, userEvent: "delete.backward" }
                : {
                    changes: { from, insert: prefix },
                    selection: { anchor: from + prefix.length },
                    userEvent: "input.type",
                  },
            );
            agree(main.state, before, after, `${label} in ${cell}`, {
              lines: 4,
              columns: 2,
            });
            undo(main);
            expect(docText(main.state), `${label} in ${cell}`).toBe(
              fixture.text,
            );
          }
        });

        test(`a replace writing ${label} in every cell keeps the table (R24)`, () => {
          const probe = soloMain(fixture.text);
          for (const { cell, from, to } of contents(probe)) {
            const main = soloMain(fixture.text, [gridExtension()]);
            const text = docText(main.state).slice(from, to);
            main.dispatch({
              changes: {
                from,
                to,
                insert: prefix === null ? "" : `${prefix}${text}`,
              },
              userEvent: "input.replace",
            });
            agree(main.state, before, after, `replace ${label} in ${cell}`, {
              lines: 4,
              columns: 2,
            });
          }
        });
      }

      for (const [name, run] of verbs) {
        test(`${name} in every cell keeps one table every reader agrees on`, () => {
          for (const [cell, at] of positions) {
            const v = editor(fixture.text, 0);
            if (!run(v, at)) {
              expect(docText(v.state), `${name} at ${cell}`).toBe(fixture.text);
              v.destroy();
              continue;
            }
            agree(v.state, before, after, `${name} at ${cell}`);
            v.destroy();
          }
        });
      }
    });
  }
});

/**
 * A verb right after an undo on a busy machine (D54). An undo gives the parse
 * of the new state a 20 ms slice of wall clock; a pause of the page during it
 * leaves the tree short of the table, and the verb read "no table" there.
 * Here every look at the clock during the undo jumps a second, so the slice
 * takes one parse step, at any load.
 */
describe("a verb right after an undo whose parse fell short (D54)", () => {
  test("still finds the table", () => {
    const above = Array.from(
      { length: 400 },
      (_, i) => `Paragraph ${String(i)} of the page.`,
    ).join("\n\n");
    const text = `${above}\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n`;
    const at = text.indexOf("| 1") + 2;
    const v = editor(text, at);
    expect(tableAddRowBelow(v, at)).toBe(true);
    const now = Date.now.bind(Date);
    let ahead = 0;
    const clock = vi.spyOn(Date, "now").mockImplementation(() => {
      ahead += 1_000;
      return now() + ahead;
    });
    undo(v);
    clock.mockRestore();
    expect(syntaxTree(v.state).length).toBeLessThan(at);
    expect(tableAlignColumn(v, "right", at)).toBe(true);
    expect(docText(v.state)).toContain("| ---: | --- |");
  });
});
