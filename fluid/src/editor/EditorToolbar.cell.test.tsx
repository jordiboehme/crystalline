/**
 * The sticky Formatting bar while a table cell is open (final review
 * finding 1). Main's selection is stale then, because the cell sends only its
 * caret: an inline verb runs in the cell, and a verb on main's line or an
 * insert into main is off, so no click changes bytes where the person is not
 * looking. Finding 12 rides along: main's stale caret line keeps its marks
 * folded while the cell is open.
 */

import type { EditorView } from "@codemirror/view";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test } from "vitest";

import { Tooltips } from "../components/primitives";
import { bytes, cleanupCells, soloMain } from "../test/cellKit";
import { EditorToolbar } from "./EditorToolbar";
import { fencePreviews } from "./fencePreviews";
import { floatingContextListener, openCellView } from "./floatingContext";
import { activeCell } from "./grid/cellEditor";
import { hostOpenCell } from "./grid/gridWidget";
import { gridExtension } from "./grid/index";
import { mountedCell, setOpenCell } from "./grid/openCell";
import { gridTables } from "./grid/structure";
import { livePreview } from "./preview";

const DOC = "## Crew\n\n| a | b |\n| - | - |\n| Ada | x |\n\nAfter\n";

afterEach(() => {
  cleanupCells();
});

/** Main with its caret on the heading, and the cell "Ada" open. */
function mainWithOpenCell(): { main: EditorView; cell: EditorView } {
  const main = soloMain(DOC, [
    gridExtension(),
    fencePreviews(false),
    livePreview(),
    floatingContextListener(() => undefined),
  ]);
  main.dispatch({ selection: { anchor: 4 } }); // inside "## Crew"
  const ada = gridTables(main.state)[0]?.rows[1]?.[0];
  if (!ada) throw new Error("no cell");
  main.dispatch({
    effects: setOpenCell.of({
      anchor: ada.content.to,
      head: ada.content.to,
      focused: true,
    }),
  });
  const editor = hostOpenCell(main, "now");
  if (!editor) throw new Error("not hosted");
  editor.view.dispatch({
    selection: { anchor: ada.content.from, head: ada.content.to },
  });
  return { main, cell: editor.view };
}

function bar(main: EditorView, cellOpen: boolean) {
  return render(<EditorToolbar view={main} cellOpen={cellOpen} />, {
    wrapper: Tooltips,
  });
}

function button(name: string): HTMLElement {
  return screen.getByRole("button", { name });
}

describe("the sticky bar with a cell open", () => {
  test("the getter finds the open cell's view", () => {
    const { main, cell } = mainWithOpenCell();
    expect(openCellView(main)).toBe(cell);
  });

  test("Bold wraps the cell's selection, not main's stale line", async () => {
    const { main } = mainWithOpenCell();
    bar(main, true);
    await userEvent.click(button("Bold"));
    expect(bytes(main.state)).toBe(DOC.replace("| Ada |", "| **Ada** |"));
  });

  test.each(["Italic", "Strikethrough", "Inline code", "Link", "Wiki link"])(
    "%s edits the cell, not the heading",
    async (name) => {
      const { main } = mainWithOpenCell();
      bar(main, true);
      await userEvent.click(button(name));
      const after = bytes(main.state);
      expect(after.startsWith("## Crew\n")).toBe(true);
      expect(after).not.toBe(DOC);
      expect(after.endsWith("|\n\nAfter\n")).toBe(true);
    },
  );

  test("the line and insert verbs are off", () => {
    const { main } = mainWithOpenCell();
    bar(main, true);
    for (const name of [
      "Heading",
      "Bulleted list",
      "Numbered list",
      "Task list",
      "Blockquote",
      "Insert table",
      "Insert",
    ]) {
      expect(button(name)).toBeDisabled();
    }
    expect(button("Bold")).toBeEnabled();
  });

  test("a stale render's list click changes nothing", async () => {
    const { main } = mainWithOpenCell();
    // The bar has not heard of the cell yet: the button is on, and the
    // click is refused when it runs.
    bar(main, false);
    await userEvent.click(button("Bulleted list"));
    await userEvent.click(button("Blockquote"));
    expect(bytes(main.state)).toBe(DOC);
  });

  test("after the cell closes, the bar acts on main again", async () => {
    const { main } = mainWithOpenCell();
    activeCell(main)?.destroy();
    expect(openCellView(main)).toBeNull();
    bar(main, false);
    await userEvent.click(button("Bulleted list"));
    expect(bytes(main.state).startsWith("- ## Crew")).toBe(true);
  });
});

describe("the edges of the rule", () => {
  test("main holding the focus itself is main's turn, a mounted cell or not", async () => {
    const { main, cell } = mainWithOpenCell();
    main.focus(); // Shift+Tab back in: no selection change, the cell stays
    expect(mountedCell(main)?.view).toBe(cell);
    expect(openCellView(main)).toBeNull();
    bar(main, false);
    await userEvent.click(button("Bold"));
    // Main's caret (in "## Crew", after "## C") got the marks, the cell none.
    expect(bytes(main.state)).toBe(DOC.replace("## Crew", "## C****rew"));
  });

  test("a cell whose editor was torn down (scrolled away) takes no verb", async () => {
    const { main } = mainWithOpenCell();
    // What a table leaving the viewport does: the editor goes, the intent stays.
    mountedCell(main)?.teardown();
    expect(openCellView(main)).toBeNull();
    bar(main, false);
    await userEvent.click(button("Bold"));
    await userEvent.click(button("Bulleted list"));
    await userEvent.click(button("Link"));
    expect(bytes(main.state)).toBe(DOC);
  });
});

describe("main's stale caret line while a cell is open (finding 12)", () => {
  test("the heading mark is folded while the cell is open", () => {
    const { main } = mainWithOpenCell();
    const heading = main.contentDOM.querySelector(".cm-line");
    expect(heading?.textContent).toBe("Crew");
  });

  test("and shows again once the cell closes", () => {
    const { main } = mainWithOpenCell();
    activeCell(main)?.destroy();
    const heading = main.contentDOM.querySelector(".cm-line");
    expect(heading?.textContent).toBe("## Crew");
  });
});
