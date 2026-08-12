/**
 * The bar as a component: that every label reaches the verb it promises, and
 * that the context segment appears and disappears without taking the caret
 * with it.
 *
 * What the verbs DO is `toolbar.test.ts`'s and `tableVerbs.test.ts`'s job, on
 * plain views with no React in sight. What is under test here is the wiring
 * between the two - an accessible name, a click, and the buffer edit that
 * follows - which is exactly the seam a renamed constant or a copied `onClick`
 * breaks without any command test noticing.
 */

import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { RenderResult } from "@testing-library/react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test } from "vitest";

import { EditorToolbar } from "./EditorToolbar";
import { baseExtensions, docText, lineSeparatorFor } from "./setup";

const TABLE_DOC = "Before\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\nAfter\n";
const IN_TABLE = TABLE_DOC.indexOf("| 1") + 2;

/** The segment's six controls, by the names that are the contract. */
const SEGMENT = [
  "Add row below",
  "Add column after",
  "Delete row",
  "Delete column",
  "Align column",
  "Prettify table",
];

let view: EditorView | null = null;
/** The bar `mount` put up, so a test can redraw that ONE rather than add another. */
let bar: RenderResult | null = null;
afterEach(() => {
  view?.destroy();
  view = null;
  bar = null;
});

/** A live buffer with the bar above it, exactly as a screen mounts them. */
function mount(
  doc: string,
  anchor: number,
  head = anchor,
  tableActive = false,
): EditorView {
  view = new EditorView({
    state: EditorState.create({
      doc,
      selection: EditorSelection.single(anchor, head),
      // The markdown language, because table detection reads the syntax tree.
      extensions: [...lineSeparatorFor(doc), ...baseExtensions(false)],
    }),
    parent: document.body,
  });
  bar = render(<EditorToolbar view={view} tableActive={tableActive} />);
  return view;
}

/** The same bar again with the segment flipped - what a screen's state does. */
function flip(tableActive: boolean): void {
  bar?.rerender(<EditorToolbar view={view} tableActive={tableActive} />);
}

function press(name: string): Promise<void> {
  return userEvent.click(screen.getByRole("button", { name }));
}

/** Open the align menu and pick one of its items. */
async function align(label: string): Promise<void> {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Align column" }));
  await user.click(await screen.findByRole("menuitem", { name: label }));
}

describe("the format buttons", () => {
  test("Strikethrough wraps the selection", async () => {
    const v = mount("hello world", 0, 5);
    await press("Strikethrough");
    expect(docText(v.state)).toBe("~~hello~~ world");
  });

  test("Numbered list numbers the cursor's line", async () => {
    const v = mount("one\ntwo\n", 0);
    await press("Numbered list");
    expect(docText(v.state)).toBe("1. one\ntwo\n");
  });

  test("Blockquote quotes the cursor's line", async () => {
    const v = mount("one\ntwo\n", 0);
    await press("Blockquote");
    expect(docText(v.state)).toBe("> one\ntwo\n");
  });

  test("Link inserts a link with its text selected", async () => {
    const v = mount("", 0);
    await press("Link");
    expect(docText(v.state)).toBe("[text](url)");
    const { from, to } = v.state.selection.main;
    expect(v.state.sliceDoc(from, to)).toBe("text");
  });

  test("Insert table opens the size picker and inserts from it", async () => {
    // The pin for the one event-path question this bar raises: it cancels its
    // own mousedown to keep the selection in the buffer, and a Popover trigger
    // opens on CLICK, which a cancelled mousedown never suppresses. If that
    // were wrong the button would look dead.
    const v = mount("prose\n", 5);
    await press("Insert table");
    expect(screen.getByRole("group", { name: "Table size" })).not.toBeNull();
    await press("2 columns by 2 rows");
    expect(docText(v.state)).toBe(
      "prose\n\n| Column | Column |\n| --- | --- |\n|  |  |\n\n",
    );
  });

  test("Insert code block inserts a bare fence", async () => {
    const v = mount("prose\n", 5);
    await press("Insert code block");
    // A blank line, the bare fence, its body line and the close - dropped
    // below the cursor's line, which keeps its own trailing break.
    expect(docText(v.state)).toBe("prose\n\n```\n\n```\n\n");
  });
});

describe("the table segment", () => {
  test("its controls are there only while the caret is in a table", () => {
    const { rerender } = render(<EditorToolbar view={null} />);
    for (const name of SEGMENT) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
    rerender(<EditorToolbar view={null} tableActive />);
    for (const name of SEGMENT) {
      expect(screen.getByRole("button", { name })).not.toBeNull();
    }
    // And the format buttons never move: the segment is an addition.
    expect(screen.getByRole("button", { name: "Bold" })).not.toBeNull();
  });

  /*
   * Every one of the six is pinned to the verb its label promises, and each
   * assertion names a result no OTHER control in the segment produces: a
   * rewiring that swapped two of them - "Delete column" running the row
   * delete, say - would otherwise ship with every gate green.
   */

  test("Add row below edits the buffer", async () => {
    const v = mount(TABLE_DOC, IN_TABLE, IN_TABLE, true);
    await press("Add row below");
    expect(docText(v.state)).toBe(
      "Before\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n|  |  |\n\nAfter\n",
    );
  });

  test("Add column after edits the buffer", async () => {
    const v = mount(TABLE_DOC, IN_TABLE, IN_TABLE, true);
    await press("Add column after");
    expect(docText(v.state)).toContain("| a | Column | b |");
  });

  test("Delete row takes the caret's row and only that", async () => {
    const v = mount(TABLE_DOC, IN_TABLE, IN_TABLE, true);
    await press("Delete row");
    expect(docText(v.state)).toBe(
      "Before\n\n| a | b |\n| --- | --- |\n\nAfter\n",
    );
  });

  test("Delete column takes the caret's column and only that", async () => {
    const v = mount(TABLE_DOC, IN_TABLE, IN_TABLE, true);
    await press("Delete column");
    expect(docText(v.state)).toBe("Before\n\n| b |\n| --- |\n| 2 |\n\nAfter\n");
  });

  test("Prettify table pads the whole table", async () => {
    const doc = "| name | n |\n| --- | --- |\n| longer | 2 |\n";
    const v = mount(doc, doc.indexOf("longer"), doc.indexOf("longer"), true);
    await press("Prettify table");
    // Three, not one, for the narrow column: see `tableVerbs.test.ts`, where
    // the rule-width floor this follows from is spelled out.
    expect(docText(v.state)).toContain("| name   | n   |");
  });

  /*
   * The align menu is the one place a user-visible label is bound to a typed
   * enum value, so all three mappings are pinned separately: an array that was
   * reordered rather than relabelled would still put the right colons in the
   * wrong place, and only a per-alignment assertion catches that.
   */
  const RULES: [string, string][] = [
    ["Align left", "| :--- | --- |"],
    ["Align center", "| :---: | --- |"],
    ["Align right", "| ---: | --- |"],
  ];
  for (const [label, rule] of RULES) {
    test(`${label} rewrites the caret's rule cell`, async () => {
      const v = mount(TABLE_DOC, IN_TABLE, IN_TABLE, true);
      await align(label);
      // The caret's column is the FIRST, so the second rule cell must stay
      // exactly as it was - alignment touches one delimiter cell, no more.
      expect(docText(v.state)).toContain(rule);
    });
  }

  test("appearing moves no focus", () => {
    const v = mount(TABLE_DOC, IN_TABLE);
    v.focus();
    const focused = document.activeElement;
    // The SAME bar redrawn with the segment on, which is what a screen's
    // state does - rendering a second bar would prove nothing about the
    // appearance path.
    flip(true);
    expect(
      screen.getByRole("button", { name: "Add column after" }),
    ).not.toBeNull();
    expect(document.activeElement).toBe(focused);
  });
});
