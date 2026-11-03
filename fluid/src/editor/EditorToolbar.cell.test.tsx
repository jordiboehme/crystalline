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
import { afterEach, describe, expect, test, vi } from "vitest";

import { Tooltips } from "../components/primitives";
import { bytes, cleanupCells, soloMain } from "../test/cellKit";
import { typeText } from "../test/markKit";
import { EditorToolbar } from "./EditorToolbar";
import { InsertMenu } from "./InsertMenu";
import { TableSizePicker } from "./TableSizePicker";
import type { FloatingContext } from "./floatingContext";
import { fencePreviews } from "./fencePreviews";
import { floatingContextListener, openCellView } from "./floatingContext";
import { activeCell } from "./grid/cellEditor";
import { hostOpenCell } from "./grid/gridWidget";
import { gridExtension } from "./grid/index";
import { mountedCell, setOpenCell } from "./grid/openCell";
import { gridTables } from "./grid/structure";
import { formatStateListener, hiddenMarkLayer } from "./marks";
import { marksOnEditLine } from "./marks/mode";
import { livePreview } from "./preview";

/** The hidden-mark layer in show mode: these tests pin a touched line's marks. */
const showModeMarks = [hiddenMarkLayer(), marksOnEditLine.of(true)];

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
    ...showModeMarks,
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
    // Main's caret (in "## Crew", after "## C") got the bold, the cell none.
    // Where marks are hidden or shown in place, Bold with no selection waits
    // for typing rather than writing an empty pair.
    expect(bytes(main.state)).toBe(DOC);
    typeText(main, "x");
    expect(bytes(main.state)).toBe(DOC.replace("## Crew", "## C**x**rew"));
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

describe("a torn-down cell's parked selection (round 2 A and B)", () => {
  /** The cell's editor gone with its table scrolled away; the intent kept. */
  function tornDown() {
    const seen: FloatingContext[] = [];
    const main = soloMain(DOC, [
      gridExtension(),
      fencePreviews(false),
      livePreview(),
      ...showModeMarks,
      floatingContextListener((context) => {
        seen.push(context);
      }),
    ]);
    // A non-empty main selection outside the table, from before the cell.
    main.dispatch({ selection: { anchor: 3, head: 7 } });
    const ada = gridTables(main.state)[0]?.rows[1]?.[0];
    if (!ada) throw new Error("no cell");
    main.dispatch({
      effects: setOpenCell.of({
        anchor: ada.content.to,
        head: ada.content.to,
        focused: true,
      }),
    });
    if (!hostOpenCell(main, "now")) throw new Error("not hosted");
    mountedCell(main)?.teardown();
    return { main, last: () => seen.at(-1) };
  }

  test("the insert verbs show off, with the screen's cellOpen false", () => {
    const { main } = tornDown();
    render(
      <EditorToolbar view={main} cellOpen={false} onAttach={() => undefined} />,
      { wrapper: Tooltips },
    );
    for (const name of ["Insert table", "Insert", "Attach a file", "Heading"]) {
      expect(button(name)).toBeDisabled();
    }
  });

  test("an attach that arrives anyway inserts nothing", async () => {
    const { main } = tornDown();
    const picked: File[][] = [];
    // Drawn while main held the focus (its turn), picked after it left.
    main.focus();
    const { container } = render(
      <EditorToolbar
        view={main}
        cellOpen={false}
        onAttach={(files) => {
          picked.push(files);
        }}
      />,
      { wrapper: Tooltips },
    );
    main.contentDOM.blur();
    const input =
      container.querySelector<HTMLInputElement>('input[type="file"]');
    await userEvent.upload(
      input as HTMLInputElement,
      new File(["x"], "shot.png", { type: "image/png" }),
    );
    expect(picked).toEqual([]);
  });

  test("no floating bar stands by main's parked selection", async () => {
    const { last } = tornDown();
    await vi.waitFor(() => {
      expect(last()).toBeNull();
    });
  });
});

describe("the insert components' runtime guard (round 2 A)", () => {
  test("the Insert menu refuses an entry when not allowed", async () => {
    const main = soloMain("Done \n");
    main.dispatch({ selection: { anchor: 5 } });
    render(<InsertMenu view={main} allow={() => false} />, {
      wrapper: Tooltips,
    });
    const user = userEvent.setup();
    await user.click(button("Insert"));
    await user.click(
      await screen.findByRole("menuitem", { name: "Status label" }),
    );
    expect(bytes(main.state)).toBe("Done \n");
  });

  test("the table size picker refuses an insert when not allowed", async () => {
    const main = soloMain("x\n");
    render(<TableSizePicker view={main} allow={() => false} />, {
      wrapper: Tooltips,
    });
    await userEvent.click(button("Insert table"));
    await userEvent.click(button("2 columns by 2 rows"));
    expect(bytes(main.state)).toBe("x\n");
  });
});

describe("main's stale caret line while a cell is open (finding 12)", () => {
  test("the heading mark is folded while the cell is open", () => {
    const { main } = mainWithOpenCell();
    const heading = main.contentDOM.querySelector(".cm-line");
    expect(heading?.textContent).toBe("Crew");
  });

  test("main taking the focus back shows its caret line's marks (round 2 D)", async () => {
    const { main } = mainWithOpenCell();
    main.focus(); // Shift+Tab back in: the cell stays mounted
    await vi.waitFor(() => {
      expect(main.contentDOM.querySelector(".cm-line")?.textContent).toBe(
        "## Crew",
      );
    });
  });

  test("and shows again once the cell closes", () => {
    const { main } = mainWithOpenCell();
    activeCell(main)?.destroy();
    const heading = main.contentDOM.querySelector(".cm-line");
    expect(heading?.textContent).toBe("## Crew");
  });
});

describe("the pressed format buttons with a cell open (C8)", () => {
  const BOLD_CELL =
    "**Crew** x\n\n| a | b |\n| - | - |\n| *Ada* | x |\n\nAfter\n";

  function withFormats() {
    const seen: ReadonlySet<string>[] = [];
    const main = soloMain(BOLD_CELL, [
      gridExtension(),
      fencePreviews(false),
      livePreview(),
      hiddenMarkLayer(),
      floatingContextListener(() => undefined),
      formatStateListener((formats) => {
        seen.push(formats);
      }),
    ]);
    // Main's caret in the bold word, from before the cell.
    main.dispatch({ selection: { anchor: 4 } });
    const ada = gridTables(main.state)[0]?.rows[1]?.[0];
    if (!ada) throw new Error("no cell");
    main.dispatch({
      effects: setOpenCell.of({
        anchor: ada.content.from + 2,
        head: ada.content.from + 2,
        focused: true,
      }),
    });
    const editor = hostOpenCell(main, "now");
    if (!editor) throw new Error("not hosted");
    editor.view.dispatch({
      selection: { anchor: ada.content.from + 2 },
    });
    return { main, cell: editor.view, last: () => [...(seen.at(-1) ?? [])] };
  }

  test("are the open cell's, never main's parked caret", () => {
    const { last } = withFormats();
    expect(last()).toEqual(["em"]);
  });

  test("are none while main is parked with no cell mounted", async () => {
    const { main, last } = withFormats();
    mountedCell(main)?.teardown();
    await vi.waitFor(() => {
      expect(last()).toEqual([]);
    });
  });

  test("are main's again once main takes the focus back", async () => {
    const { main, last } = withFormats();
    main.focus();
    await vi.waitFor(() => {
      expect(last()).toEqual(["strong"]);
    });
  });
});
