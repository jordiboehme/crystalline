/**
 * Hidden marks inside the open table cell (Task 12): the cell carries the
 * hidden-mark layer, its own segment and the marks setting, so it draws,
 * steps, deletes and formats the way a line of prose does, and Part 1's cell
 * rules (the escapes, the arrows across cells, Enter going down, Shift+F10's
 * Cell actions) keep their place. The element editors open from a cell and
 * commit there, footnote references draw their numbers but do not jump, and
 * a copy carries HTML. Every case runs on the real surface with the grid's
 * own mount, solo and in a room.
 */

import { EditorState, StateEffect } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { MockInstance } from "vitest";
import {
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";

import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  setShowMarks,
  typeText,
  undoOnce,
} from "../../test/markKit";
import type { ElementEdit } from "../elementEdit";
import { commitElementEdit, linkVerb } from "../elementEdit";
import { verbTarget } from "../floatingContext";
import { activeCell } from "./cellEditor";
import { hostOpenCell } from "./gridWidget";
import { prefetchGrid } from "./loader";
import { openCellRange, setOpenCell } from "./openCell";
import { gridTables } from "./structure";

const DOC = "| a | **b** |\n| - | - |\n| 1 | 2 |\n";
/** The `**b**` cell's content, and inside it the `b`. */
const B_FROM = DOC.indexOf("**b**");
const B_TO = B_FROM + 5;

let errors: MockInstance<typeof console.error>;

beforeAll(async () => {
  await prefetchGrid();
});

beforeEach(() => {
  errors = vi.spyOn(console, "error");
});

afterEach(() => {
  cleanupMarkViews();
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

type Mode = "hidden" | "show";

interface Page {
  mode?: Mode;
  room?: boolean;
  adapter?: "engram" | "reflection";
  heard?: { edit: ElementEdit | null; view: EditorView }[];
}

function page(text: string, options: Page = {}): EditorView {
  const heard = options.heard;
  return markView(text, {
    mode: options.mode ?? "hidden",
    room: options.room ?? false,
    adapter: options.adapter ?? "engram",
    ...(heard
      ? {
          onElementEdit: (edit: ElementEdit | null, view: EditorView) => {
            heard.push({ edit, view });
          },
        }
      : {}),
  });
}

/**
 * Opens cell `row`/`column` of the first table through the grid's own mount,
 * with the caret at the content's start, end or a position.
 */
function open(
  main: EditorView,
  row: number,
  column: number,
  where: "start" | "end" | number = "end",
): EditorView {
  const cell = gridTables(main.state)[0]?.rows[row]?.[column];
  if (!cell) throw new Error(`no cell ${String(row)}/${String(column)}`);
  const at =
    where === "start"
      ? cell.content.from
      : where === "end"
        ? cell.content.to
        : where;
  main.dispatch({
    effects: setOpenCell.of({ anchor: at, head: at, focused: true }),
  });
  const editor = hostOpenCell(main, "now");
  if (!editor) throw new Error("no cell editor");
  return editor.view;
}

/** The open cell as [row, column], or null. */
function place(main: EditorView): [number, number] | null {
  const cell = openCellRange(main.state);
  return cell ? [cell.row, cell.column] : null;
}

/** What the open cell shows. */
function shown(cell: EditorView): string {
  return cell.contentDOM.textContent ?? "";
}

/** The row line `n` (1-based) of `main`. */
function line(main: EditorView, n: number): string {
  return main.state.doc.line(n).text;
}

function keydown(
  el: HTMLElement,
  key: string,
  init: KeyboardEventInit = {},
): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key,
    bubbles: true,
    cancelable: true,
    ...init,
  });
  el.dispatchEvent(event);
  return event;
}

function press(el: Element): MouseEvent {
  const event = new MouseEvent("mousedown", {
    bubbles: true,
    cancelable: true,
    button: 0,
  });
  el.dispatchEvent(event);
  return event;
}

function cellActions(): HTMLElement | null {
  return document.querySelector<HTMLElement>(
    '[role="menu"][aria-label="Cell actions"]',
  );
}

interface Copied {
  data: Map<string, string>;
}

/** A copy as the browser sends it; what reached the clipboard comes back. */
function copy(view: EditorView): Copied {
  const data = new Map<string, string>();
  const event = new Event("copy", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    configurable: true,
    value: {
      clearData: () => {
        data.clear();
      },
      setData: (kind: string, value: string) => {
        data.set(kind, value);
      },
      getData: (kind: string) => data.get(kind) ?? "",
    },
  });
  view.contentDOM.dispatchEvent(event);
  return { data };
}

/** A paste as the browser sends it, with the flavours `copied` holds. */
function paste(view: EditorView, copied: Copied): void {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    value: {
      getData: (kind: string) => copied.data.get(kind) ?? "",
      types: [...copied.data.keys()],
      files: [],
      items: [],
    },
  });
  view.contentDOM.dispatchEvent(event);
}

const ROOMS = [
  { name: "solo", room: false },
  { name: "in a room", room: true },
];

const MODES: Mode[] = ["hidden", "show"];

describe.each(ROOMS)("a bold cell in hidden mode, $name", ({ room }) => {
  it("shows no marks and steps over them", () => {
    const main = page(DOC, { room });
    const cell = open(main, 0, 1);
    expect(shown(cell)).toBe("b");
    expect(shown(cell)).not.toContain("*");
    keys(cell, "ArrowLeft");
    // One press from the end of `b` is the cell's visible start.
    expect(caret(cell)).toBe(B_FROM + 2);
    expect(place(main)).toEqual([0, 1]);
  });

  it("types inside the marks and Backspace takes the pair with the last letter", () => {
    const main = page(DOC, { room });
    const cell = open(main, 0, 1);
    typeText(cell, "x");
    expect(line(main, 1)).toBe("| a | **bx** |");
    keys(cell, "Backspace");
    expect(line(main, 1)).toBe("| a | **b** |");
    keys(cell, "Backspace");
    expect(line(main, 1)).toBe("| a |  |");
    expect(bytes(cell.state)).toBe(bytes(main.state));
  });

  it("Backspace at the visible start and Delete at the visible end change nothing", () => {
    const main = page(DOC, { room });
    const cell = open(main, 0, 1, B_FROM + 2);
    expect(caret(cell)).toBe(B_FROM + 2);
    keys(cell, "Backspace");
    expect(bytes(main.state)).toBe(DOC);
    expect(caret(cell)).toBe(B_FROM + 2);
    const end = open(main, 0, 1);
    keys(end, "Delete");
    expect(bytes(main.state)).toBe(DOC);
    expect(caret(end)).toBe(B_TO - 2);
  });

  it("entered from its left neighbour, a typed letter goes inside the marks", () => {
    const main = page(DOC, { room });
    const cell = open(main, 0, 0);
    keys(cell, "ArrowRight");
    expect(place(main)).toEqual([0, 1]);
    const next = activeCell(main)?.view;
    if (!next) throw new Error("no cell");
    typeText(next, "x");
    expect(line(main, 1)).toBe("| a | **xb** |");
  });
});

describe.each(ROOMS.flatMap((r) => MODES.map((mode) => ({ ...r, mode }))))(
  "in a cell, $mode mode, $name",
  ({ room, mode }) => {
    it("Mod-b with no selection, then a letter, is bold and one undo step", () => {
      const main = page(DOC, { room, mode });
      const cell = open(main, 1, 0);
      keys(cell, "Mod-b");
      typeText(cell, "y");
      expect(line(main, 3)).toBe("| 1**y** | 2 |");
      expect(caret(cell)).toBe(main.state.doc.line(3).from + 6);
      undoOnce(main);
      expect(bytes(main.state)).toBe(DOC);
    });

    it("Backspace after an escaped pipe takes the whole escape (Part 1's rule)", () => {
      const text = "| a\\|b | c |\n| - | - |\n";
      const main = page(text, { room, mode });
      const cell = open(main, 0, 0, text.indexOf("b"));
      keys(cell, "Backspace");
      expect(line(main, 1)).toBe("| ab | c |");
    });

    it("ArrowRight at the end and ArrowLeft at the start cross cells", () => {
      const main = page(DOC, { room, mode });
      const cell = open(main, 0, 1);
      if (mode === "hidden") expect(caret(cell)).toBe(B_TO - 2);
      keys(cell, "ArrowRight");
      expect(place(main)).toEqual([1, 0]);
      const back = open(main, 0, 1, "start");
      keys(back, "ArrowLeft");
      expect(place(main)).toEqual([0, 0]);
    });

    it("ArrowDown on the line above the table opens the first header cell", () => {
      const text = `Intro\n\n${DOC}`;
      const main = page(text, { room, mode });
      main.focus();
      main.dispatch({ selection: { anchor: 6 } });
      keys(main, "ArrowDown");
      expect(place(main)).toEqual([0, 0]);
    });

    it("Enter goes down a column, also from a cell that reads like a list", () => {
      const text = "| - a | b |\n| - | - |\n| 1 | 2 |\n";
      const main = page(text, { room, mode });
      const cell = open(main, 0, 0);
      expect(keys(cell, "Enter")).toBe(true);
      expect(place(main)).toEqual([1, 0]);
      expect(bytes(main.state)).toBe(text);
    });

    it("a setting flip switches the open cell in place", () => {
      const main = page(DOC, { room, mode });
      const editor = activeCell(main) ?? null;
      expect(editor).toBeNull();
      const cell = open(main, 0, 1, B_FROM + 3);
      const mounted = activeCell(main);
      const at = caret(cell);
      setShowMarks(main, mode === "hidden");
      expect(activeCell(main)).toBe(mounted);
      expect(caret(cell)).toBe(at);
      expect(shown(cell)).toBe(mode === "hidden" ? "**b**" : "b");
      setShowMarks(main, mode !== "hidden");
      expect(activeCell(main)).toBe(mounted);
      expect(caret(cell)).toBe(at);
      expect(shown(cell)).toBe(mode === "hidden" ? "b" : "**b**");
    });

    it("a copy of the bold letter carries HTML, and pastes back as its Markdown", () => {
      const main = page(DOC, { room, mode });
      const cell = open(main, 0, 1);
      cell.focus();
      cell.dispatch({
        selection: { anchor: B_FROM + 2, head: B_FROM + 3 },
        userEvent: "select.pointer",
      });
      const bold = copy(cell);
      expect(bold.data.get("text/html")).toContain("<strong>b</strong>");
      expect(bold.data.get("text/html")).toContain("data-crystalline-copy");
      cell.dispatch({
        selection: { anchor: B_FROM, head: B_TO },
        userEvent: "select.pointer",
      });
      const whole = copy(cell);
      expect(whole.data.get("text/plain")).toBe("**b**");
      expect(whole.data.get("text/html")).toContain("<strong>b</strong>");
      const target = open(main, 1, 1);
      target.focus();
      paste(target, whole);
      expect(line(main, 3)).toBe("| 1 | 2**b** |");
    });

    it("Shift+F10 on a link opens its popover, elsewhere the Cell actions", () => {
      const text = "| [t](https://x.y) | c |\n| - | - |\n";
      const heard: Page["heard"] = [];
      const main = page(text, { room, mode, heard });
      const onLink = open(main, 0, 0, text.indexOf("t]"));
      const event = keydown(onLink.contentDOM, "F10", {
        shiftKey: true,
        keyCode: 121,
      });
      expect(event.defaultPrevented).toBe(true);
      expect(heard.at(-1)?.edit?.kind).toBe("link");
      expect(heard.at(-1)?.view).toBe(onLink);
      expect(cellActions()).toBeNull();
      const plain = open(main, 0, 1);
      keydown(plain.contentDOM, "F10", { shiftKey: true, keyCode: 121 });
      expect(cellActions()).not.toBeNull();
      // Only the closes are told with main.
      expect(heard.filter((h) => h.edit).every((h) => h.view !== main)).toBe(
        true,
      );
      expect(bytes(main.state)).toBe(text);
    });
  },
);

describe.each(ROOMS)("element editors in a cell, $name", ({ room }) => {
  const LINK = "| [t](https://x.y) | c |\n| - | - |\n";

  function address(next: string) {
    return (_state: unknown, edit: ElementEdit) =>
      edit.kind === "link" && edit.address
        ? [{ from: edit.address.from, to: edit.address.to, insert: next }]
        : null;
  }

  it("a press on a link's words asks from the cell, and the commit lands there", () => {
    const heard: Page["heard"] = [];
    const main = page(LINK, { room, heard });
    const cell = open(main, 0, 1);
    typeText(cell, "z");
    const linkCell = open(main, 0, 0);
    const words = linkCell.contentDOM.querySelector(".cm-link-text");
    if (!words) throw new Error("no link words");
    press(words);
    expect(heard.at(-1)?.edit?.kind).toBe("link");
    expect(heard.at(-1)?.view).toBe(linkCell);
    expect(commitElementEdit(main, address("https://z.y"))).toBe(true);
    expect(line(main, 1)).toBe("| [t](https://z.y) | cz |");
    expect(document.activeElement).toBe(linkCell.contentDOM);
    expect(activeCell(main)?.view).toBe(linkCell);
    undoOnce(main);
    expect(line(main, 1)).toBe("| [t](https://x.y) | cz |");
  });

  it("typing then committing in the same cell is two undo steps", () => {
    const heard: Page["heard"] = [];
    const main = page(LINK, { room, heard });
    // After the link's words the caret rests past its address, so the
    // letter goes after the link.
    const cell = open(main, 0, 0, LINK.indexOf("t]") + 1);
    typeText(cell, "q");
    expect(line(main, 1)).toBe("| [t](https://x.y)q | c |");
    cell.dispatch({ selection: { anchor: LINK.indexOf("t]") } });
    expect(keys(cell, "Mod-k")).toBe(true);
    expect(heard.at(-1)?.edit?.kind).toBe("link");
    expect(heard.at(-1)?.view).toBe(cell);
    expect(commitElementEdit(main, address("https://z.y"))).toBe(true);
    expect(line(main, 1)).toBe("| [t](https://z.y)q | c |");
    expect(document.activeElement).toBe(cell.contentDOM);
    undoOnce(main);
    expect(line(main, 1)).toBe("| [t](https://x.y)q | c |");
    undoOnce(main);
    expect(bytes(main.state)).toBe(LINK);
  });

  it("the Link button while a cell is open asks from the cell", () => {
    const heard: Page["heard"] = [];
    const main = page(LINK, { room, heard });
    const cell = open(main, 0, 1);
    const target = verbTarget(main);
    expect(target).toBe(cell);
    if (target) linkVerb(target);
    expect(heard.at(-1)?.edit?.kind).toBe("new-link");
    expect(heard.at(-1)?.view).toBe(cell);
    expect(bytes(main.state)).toBe(LINK);
  });

  it("a press on a wikilink chip or a status pill asks from the cell", () => {
    const text = "| [[tide-tables]] | :status[Done]{green} |\n| - | - |\n";
    const heard: Page["heard"] = [];
    const main = page(text, { room, heard });
    const chipCell = open(main, 0, 0);
    const chip = chipCell.contentDOM.querySelector(".cm-wikilink");
    if (!chip) throw new Error("no chip");
    press(chip);
    expect(heard.at(-1)?.edit?.kind).toBe("wikilink");
    expect(heard.at(-1)?.view).toBe(chipCell);
    const pillCell = open(main, 0, 1);
    const pill = pillCell.contentDOM.querySelector(".status-label");
    if (!pill) throw new Error("no pill");
    press(pill);
    expect(heard.at(-1)?.edit?.kind).toBe("status");
    expect(heard.at(-1)?.view).toBe(pillCell);
    expect(bytes(main.state)).toBe(text);
  });

  it("writes nothing once the page stops taking writes", () => {
    const heard: Page["heard"] = [];
    const main = page(LINK, { room, heard });
    const cell = open(main, 0, 0, LINK.indexOf("t]"));
    keys(cell, "Mod-k");
    expect(heard.at(-1)?.view).toBe(cell);
    // A read-only page closes the cell; the request closes with it.
    main.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    expect(commitElementEdit(main, address("https://z.y"))).toBe(false);
    expect(bytes(main.state)).toBe(LINK);
  });
});

describe.each(MODES)("footnotes in a drawn table, %s mode", (mode) => {
  const text = "| a[^n] | c |\n| - | - |\n\n[^n]: N\n";

  it("the open cell draws the number, and its click and Enter do nothing", () => {
    const main = page(text, { mode });
    const cell = open(main, 0, 1);
    const refCell = open(main, 0, 0, text.indexOf("a") + 1);
    expect(cell).not.toBe(refCell);
    const refs = [...refCell.contentDOM.querySelectorAll(".cm-footnote-ref")];
    if (mode === "hidden") {
      expect(refs.map((el) => el.textContent)).toEqual(["1"]);
    }
    const button = refCell.contentDOM.querySelector<HTMLElement>(
      ".cm-footnote-button",
    );
    const at = caret(refCell);
    if (button) {
      button.click();
      keydown(button, "Enter");
    }
    expect(caret(refCell)).toBe(at);
    expect(place(main)).toEqual([0, 0]);
    expect(bytes(main.state)).toBe(text);
  });

  it("the closed grid draws the reference as its literal label", () => {
    const main = page(text, { mode });
    const el = main.dom.querySelector<HTMLElement>(
      '[role="grid"] [aria-rowindex="1"][aria-colindex="1"]',
    );
    expect(el?.textContent).toBe("a[^n]");
  });
});

describe.each(MODES)("a Reflection cell, %s mode", (mode) => {
  it("never shows an HTML comment", () => {
    const text = "| a <!-- note --> b | c |\n| - | - |\n";
    const main = page(text, { mode, adapter: "reflection" });
    const cell = open(main, 0, 0);
    expect(shown(cell)).not.toContain("<!--");
    expect(shown(cell)).not.toContain("note");
    expect(bytes(main.state)).toBe(text);
  });
});
