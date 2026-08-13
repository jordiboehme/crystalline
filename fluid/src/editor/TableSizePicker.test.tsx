/**
 * The size picker as a control: that it opens where the keyboard already is,
 * that the arrows mean "bigger table" rather than "next item", and that
 * whichever way it is closed the caret goes back to the buffer.
 *
 * What the insertion DOES is `toolbar.test.ts`'s job, on plain views with no
 * React in sight. What is under test here is the one extra step the picker
 * adds in front of it - open, choose, insert - and the focus discipline that
 * makes that step free for a person who never leaves the keyboard.
 */

import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test } from "vitest";

import { TableSizePicker } from "./TableSizePicker";
import { docText, lineSeparatorFor } from "./setup";

let view: EditorView | null = null;
afterEach(() => {
  view?.destroy();
  view = null;
});

/** A live buffer with the picker above it, as the bar mounts them. */
function mount(doc = "prose line", anchor = doc.length): EditorView {
  view = new EditorView({
    state: EditorState.create({
      doc,
      selection: EditorSelection.single(anchor),
      extensions: [...lineSeparatorFor(doc)],
    }),
    parent: document.body,
  });
  render(<TableSizePicker view={view} />);
  return view;
}

function open(): Promise<void> {
  return userEvent.click(screen.getByRole("button", { name: "Insert table" }));
}

/** What the caret is sitting on, which is the point of the token selection. */
function selected(v: EditorView): string {
  const { from, to } = v.state.selection.main;
  return v.state.sliceDoc(from, to);
}

describe("TableSizePicker", () => {
  test("opens on 2x2 and Enter reproduces the historical insert", async () => {
    const v = mount();
    await open();
    const cell = screen.getByRole("button", { name: "2 columns by 2 rows" });
    expect(cell).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    expect(docText(v.state)).toBe(
      "prose line\n\n| Column | Column |\n| --- | --- |\n|  |  |\n",
    );
    // The one deliberate difference from the button this replaced: the caret
    // arrives on the first placeholder rather than at the end of the row.
    expect(selected(v)).toBe("Column");
  });

  test("arrows resize, Enter inserts the resized table", async () => {
    const v = mount();
    await open();
    await userEvent.keyboard("{ArrowRight}{ArrowDown}");
    expect(screen.getByText("3 x 3")).not.toBeNull();
    await userEvent.keyboard("{Enter}");
    expect(docText(v.state)).toContain("| Column | Column | Column |");
    expect(docText(v.state).split("|  |  |  |")).toHaveLength(3);
  });

  test("the arrows move the focus with the size", async () => {
    mount();
    await open();
    await userEvent.keyboard("{ArrowRight}{ArrowRight}{ArrowDown}");
    // Focus follows the size rather than staying behind it: Enter is a
    // native activation of the focused cell, so the two must not diverge.
    expect(
      screen.getByRole("button", { name: "4 columns by 3 rows" }),
    ).toHaveFocus();
  });

  test("the grid clamps at its corners", async () => {
    const v = mount();
    await open();
    // Six presses each way from 2x2, which is well past the edge in both.
    await userEvent.keyboard("{ArrowLeft>6/}{ArrowUp>6/}");
    expect(screen.getByText("1 x 1")).not.toBeNull();
    await userEvent.keyboard("{Enter}");
    expect(docText(v.state)).toBe("prose line\n\n| Column |\n| --- |\n");
  });

  test("Tab carries the size with it, so Enter inserts what the caption says", async () => {
    // The third mover, and the one this component never hears about: the
    // popover traps no focus, so all 48 cells are ordinary tab stops. A Tab
    // that moved the focus alone would light four cells, caption "2 x 2" and
    // insert four columns from the cell it actually left the keyboard on.
    const v = mount();
    await open();
    await userEvent.tab();
    await userEvent.tab();
    expect(
      screen.getByRole("button", { name: "4 columns by 2 rows" }),
    ).toHaveFocus();
    expect(screen.getByText("4 x 2")).not.toBeNull();
    await userEvent.keyboard("{Enter}");
    expect(docText(v.state)).toContain("| Column | Column | Column | Column |");
    expect(docText(v.state).split("|  |  |  |  |")).toHaveLength(2);
  });

  test("the highlight is every cell up and to the left of the size", async () => {
    mount();
    await open();
    await userEvent.keyboard("{ArrowRight}");
    // Tolerant of the plural, because the first column's cells say "1 column".
    const cells = screen.getAllByRole("button", { name: /columns? by/ });
    expect(cells).toHaveLength(48);
    // The ON face is a whole class string, so the accent border is what tells
    // the two apart - and counting them pins decision 4's rule rather than
    // merely that something somewhere lit up.
    const lit = cells.filter((cell) =>
      cell.className.includes("border-accent-600"),
    );
    expect(lit).toHaveLength(6); // 3 columns by 2 rows
    expect(lit.map((cell) => cell.getAttribute("aria-label"))).toContain(
      "3 columns by 2 rows",
    );
    expect(
      screen.getByRole("button", { name: "4 columns by 2 rows" }).className,
    ).toContain("border-transparent");
  });

  test("the top row is drawn as the header row it inserts", async () => {
    // Decision 3's rule - the top row IS the header row, so a 2x2 pick means a
    // header and one row to fill - is otherwise a thing a person has to be
    // told. Both faces carry the cue, because the header row is lit for every
    // size the grid can express.
    mount();
    await open();
    const face = (name: string) =>
      screen.getByRole("button", { name }).className;
    // Lit: the header row is the heavier wash of the two accent faces.
    expect(face("2 columns by 1 row")).toContain("bg-accent-400");
    expect(face("2 columns by 2 rows")).toContain("bg-accent-100");
    // Unlit: the same step, in the resting pair.
    expect(face("8 columns by 1 row")).toContain("bg-slate-400");
    expect(face("8 columns by 2 rows")).toContain("bg-slate-200");
  });

  test("a size of one says column and row, not columns and rows", async () => {
    // The corner cell is the one a screen reader reaches first and the one it
    // read out as "1 columns by 1 rows".
    mount();
    await open();
    await userEvent.keyboard("{ArrowLeft}{ArrowUp}");
    expect(
      screen.getByRole("button", { name: "1 column by 1 row" }),
    ).toHaveFocus();
    // One of each still pluralises the other: the two counts are independent.
    expect(
      screen.getByRole("button", { name: "1 column by 4 rows" }),
    ).not.toBeNull();
    expect(
      screen.getByRole("button", { name: "5 columns by 1 row" }),
    ).not.toBeNull();
  });

  test("a hover carries the focus, so Enter inserts what is highlighted", async () => {
    const v = mount();
    await open();
    await userEvent.hover(
      screen.getByRole("button", { name: "5 columns by 4 rows" }),
    );
    expect(screen.getByText("5 x 4")).not.toBeNull();
    // The pointer highlighted a size and the keyboard is still open on the
    // grid: pressing Enter has to insert the highlighted one, not the cell
    // the arrows were last on.
    await userEvent.keyboard("{Enter}");
    expect(docText(v.state)).toContain(
      "| Column | Column | Column | Column | Column |",
    );
    expect(docText(v.state).split("|  |  |  |  |  |")).toHaveLength(4);
  });

  test("clicking a cell inserts that size", async () => {
    const v = mount();
    await open();
    await userEvent.click(
      screen.getByRole("button", { name: "4 columns by 2 rows" }),
    );
    expect(docText(v.state)).toContain("| Column | Column | Column | Column |");
    expect(screen.queryByRole("group", { name: "Table size" })).toBeNull();
    expect(v.hasFocus).toBe(true);
  });

  test("Escape closes and the buffer keeps focus", async () => {
    const v = mount();
    await open();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("group", { name: "Table size" })).toBeNull();
    // Radix hands focus back to the trigger on close; this bar always returns
    // an author to the buffer instead, having inserted or having changed
    // their mind.
    expect(v.hasFocus).toBe(true);
    expect(docText(v.state)).toBe("prose line");
  });

  test("reopening starts from 2x2 again", async () => {
    const v = mount();
    await open();
    await userEvent.keyboard("{ArrowRight}{ArrowRight}{Escape}");
    await open();
    expect(screen.getByText("2 x 2")).not.toBeNull();
    await userEvent.keyboard("{Enter}");
    expect(docText(v.state)).toContain("| Column | Column |\n| --- | --- |");
    expect(docText(v.state)).not.toContain("| Column | Column | Column |");
  });

  test("without a buffer the trigger is disabled and opens nothing", async () => {
    render(<TableSizePicker view={null} />);
    const trigger = screen.getByRole("button", { name: "Insert table" });
    expect(trigger).toBeDisabled();
    await userEvent.click(trigger);
    expect(screen.queryByRole("group", { name: "Table size" })).toBeNull();
  });
});
