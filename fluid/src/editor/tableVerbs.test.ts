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
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, test } from "vitest";
import { yCollab } from "y-codemirror.next";
import { Awareness } from "y-protocols/awareness";
import * as Y from "yjs";

import { baseExtensions, docText, lineSeparatorFor } from "./setup";
import {
  tableAddColumnAfter,
  tableAddRowBelow,
  tableAlignColumn,
  tableContextAt,
  tableContextListener,
  tableDeleteColumn,
  tableDeleteRow,
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
    state: EditorState.create({
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
      tableContextListener((inTable) => seen.push(inTable)),
    ]);
    v.dispatch({ selection: EditorSelection.cursor(IN_TABLE) });
    v.dispatch({ selection: EditorSelection.cursor(IN_TABLE + 1) }); // still inside: no event
    v.dispatch({ selection: EditorSelection.cursor(2) });
    expect(seen).toEqual([true, false]);
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
      state: EditorState.create({
        doc: ytext.toJSON(),
        selection: EditorSelection.cursor(IN_TABLE),
        extensions: [
          EditorState.lineSeparator.of("\n"),
          ...baseExtensions(false),
          yCollab(ytext, awareness),
        ],
      }),
      parent: document.body,
    });
    expect(tableAddColumnAfter(view)).toBe(true);
    const shared = ytext.toJSON(); // Y.Text read: sanctioned, LF space by design
    expect(shared.split("| a | Column | b |").length - 1).toBe(1);
    expect(shared).toBe(docText(view.state));
  });
});
