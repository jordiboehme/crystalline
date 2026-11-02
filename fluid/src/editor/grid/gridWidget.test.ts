/**
 * The grid widget: a table drawn as a `role="grid"` block in place of its
 * pipe source, patched in place while it keeps its shape, and hosting the
 * cell editor in the cell the open-cell intent names - also after a shape
 * change and after the table was out of view.
 */

import type { Extension } from "@codemirror/state";
import { Compartment, EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import {
  bytes,
  cleanupCells,
  openText,
  roomMain,
  soloMain,
  typeIn,
} from "../../test/cellKit";
import { parsedState } from "../../test/parse";
import { fencePreviews } from "../fencePreviews";
import { baseExtensions } from "../setup";
import { activeCell } from "./cellEditor";
import { GridWidget, gridField } from "./gridWidget";
import { gridExtension } from "./index";
import type { CloseReason } from "./openCell";
import {
  closeCell,
  onCellClosed,
  openCellField,
  openCellRange,
} from "./openCell";

const DOC = "| a | b |\n| - | - |\n| 1 | 2 |\n";

let errors: MockInstance<typeof console.error>;

beforeEach(() => {
  errors = vi.spyOn(console, "error");
});

afterEach(() => {
  cleanupCells();
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

/** The grid and the read-only previews, as the preview compartment has them. */
function layers(): Extension[] {
  return [gridExtension(), fencePreviews(false)];
}

function grid(main: EditorView): HTMLTableElement {
  const table = main.dom.querySelector<HTMLTableElement>('table[role="grid"]');
  if (!table) throw new Error("no grid");
  return table;
}

/** The cell element at a grid row (0 is the header) and column. */
function cellEl(main: EditorView, row: number, column: number): HTMLElement {
  const el = grid(main).querySelector<HTMLElement>(
    `[aria-rowindex="${String(row + 1)}"][aria-colindex="${String(column + 1)}"]`,
  );
  if (!el) throw new Error(`no cell ${String(row)}/${String(column)}`);
  return el;
}

function mouseDown(el: HTMLElement, init: MouseEventInit = {}): MouseEvent {
  const event = new MouseEvent("mousedown", {
    bubbles: true,
    cancelable: true,
    button: 0,
    ...init,
  });
  el.dispatchEvent(event);
  return event;
}

/**
 * Opens a cell with the mouse and puts the caret at its end. jsdom has no
 * layout, so where `posAtCoords` puts the caret there means nothing.
 */
function openByMouse(main: EditorView, row: number, column: number) {
  mouseDown(cellEl(main, row, column));
  const cell = hosted(main);
  const range = openCellRange(main.state);
  if (!range) throw new Error("no open cell");
  cell.view.dispatch({ selection: { anchor: range.content.to } });
  return cell;
}

function hosted(main: EditorView) {
  const cell = activeCell(main);
  if (!cell) throw new Error("no cell editor mounted");
  return cell;
}

function hasFocus(view: EditorView): boolean {
  return view.dom.contains(document.activeElement);
}

const flush = () => new Promise<void>((resolve) => queueMicrotask(resolve));

/** The one grid widget the field draws for the first table. */
function widgetOf(main: EditorView): GridWidget {
  let found: GridWidget | null = null;
  main.state.field(gridField).between(0, main.state.doc.length, (_f, _t, d) => {
    const widget = (d.spec as { widget?: unknown }).widget;
    if (!found && widget instanceof GridWidget) found = widget;
  });
  if (!found) throw new Error("no grid widget");
  return found;
}

describe("drawing", () => {
  test("a table draws as a grid with header and data cells", () => {
    const main = soloMain(DOC, layers());
    const table = grid(main);
    expect(table.getAttribute("aria-label")).toBe("Table");
    const headers = [...table.querySelectorAll("th")];
    const cells = [...table.querySelectorAll("td")];
    expect(headers.map((el) => el.textContent)).toEqual(["a", "b"]);
    expect(cells.map((el) => el.textContent)).toEqual(["1", "2"]);
    expect(
      headers.every((el) => el.getAttribute("role") === "columnheader"),
    ).toBe(true);
    expect(cells.every((el) => el.getAttribute("role") === "gridcell")).toBe(
      true,
    );
    expect(cellEl(main, 1, 1).textContent).toBe("2");
    // The pipe source is replaced, and the read-only preview is not drawn.
    expect(main.contentDOM.textContent).not.toContain("| a |");
    expect(main.dom.querySelector(".cm-table-preview")).toBeNull();
  });

  test("the delimiter row sets each column's alignment", () => {
    const main = soloMain(
      "| l | c | r | n |\n| :- | :-: | -: | - |\n| 1 | 2 | 3 | 4 |\n",
      layers(),
    );
    const aligns = [0, 1, 2, 3].map((c) => cellEl(main, 1, c).style.textAlign);
    expect(aligns).toEqual(["left", "center", "right", ""]);
    expect(cellEl(main, 0, 1).style.textAlign).toBe("center");
  });

  test("one cell is in the tab order, the grid itself is focusable", () => {
    const main = soloMain(DOC, layers());
    const table = grid(main);
    expect(table.getAttribute("tabindex")).toBe("-1");
    const order = [...table.querySelectorAll("th, td")].map((el) =>
      el.getAttribute("tabindex"),
    );
    expect(order).toEqual(["0", "-1", "-1", "-1"]);
  });

  test("cell content is drawn as Markdown, never as HTML", () => {
    const main = soloMain(
      "| a | b |\n| - | - |\n| **x** \\| y | <script>z</script> |\n",
      layers(),
    );
    expect(cellEl(main, 1, 0).querySelector("strong")?.textContent).toBe("x");
    expect(cellEl(main, 1, 0).textContent).toBe("x | y");
    expect(cellEl(main, 1, 1).querySelector("script")).toBeNull();
    expect(cellEl(main, 1, 1).textContent).toBe("<script>z</script>");
  });

  test("Raw takes the grid away and gives it back", () => {
    const grid = new Compartment();
    const main = soloMain(DOC, [grid.of(layers())]);
    expect(main.dom.querySelector('[role="grid"]')).not.toBeNull();
    main.dispatch({ effects: grid.reconfigure([]) });
    expect(main.dom.querySelector('[role="grid"]')).toBeNull();
    expect(main.contentDOM.textContent).toContain("| a | b |");
    main.dispatch({ effects: grid.reconfigure(layers()) });
    expect(main.dom.querySelector('[role="grid"]')).not.toBeNull();
  });

  test("a table inside a blockquote keeps its text and its preview", () => {
    const main = soloMain("> | a |\n> | - |\n> | 1 |\n", layers());
    expect(main.dom.querySelector('[role="grid"]')).toBeNull();
    expect(main.dom.querySelector(".cm-table-preview")).not.toBeNull();
    expect(main.contentDOM.textContent).toContain("> | a |");
  });

  test("an edit above the table keeps the grid element", () => {
    const main = soloMain(`Intro\n\n${DOC}`, layers());
    const before = grid(main);
    main.dispatch({ changes: { from: 0, insert: "More " } });
    expect(grid(main)).toBe(before);
    expect(cellEl(main, 1, 0).textContent).toBe("1");
  });

  test("eq holds for the same shape and texts, not for another text", () => {
    const one = soloMain(DOC, layers());
    const two = soloMain(`Intro\n\n${DOC}`, layers());
    const three = soloMain(DOC.replace("2", "3"), layers());
    expect(widgetOf(one).eq(widgetOf(two))).toBe(true);
    expect(widgetOf(one).eq(widgetOf(three))).toBe(false);
  });
});

describe("opening a cell with the mouse", () => {
  test("a mousedown opens the cell under it, focused, in that cell", () => {
    const main = soloMain(DOC, layers());
    const target = cellEl(main, 1, 1);
    const event = mouseDown(target);
    expect(event.defaultPrevented).toBe(true);
    const cell = hosted(main);
    expect(target.contains(cell.view.dom)).toBe(true);
    expect(hasFocus(cell.view)).toBe(true);
    expect(main.state.field(openCellField)?.focused).toBe(true);
    expect(openText(main)).toBe("2");
  });

  test("a mousedown on the hosting cell is left to the cell view", () => {
    const main = soloMain(DOC, layers());
    mouseDown(cellEl(main, 1, 0));
    const cell = hosted(main);
    mouseDown(cell.view.contentDOM);
    expect(hosted(main)).toBe(cell);
    expect(cellEl(main, 1, 0).contains(cell.view.dom)).toBe(true);
  });

  test("moving to another cell draws the first one's text again", () => {
    const main = soloMain("| a | b |\n| - | - |\n| **1** | 2 |\n", layers());
    mouseDown(cellEl(main, 1, 0));
    mouseDown(cellEl(main, 1, 1));
    const first = cellEl(main, 1, 0);
    expect(first.querySelector(".cm-editor")).toBeNull();
    expect(first.querySelector("strong")?.textContent).toBe("1");
    expect(cellEl(main, 1, 1).contains(hosted(main).view.dom)).toBe(true);
  });

  test("moving to a cell of another table draws the first one's text again", () => {
    const main = soloMain(`${DOC}\nText\n\n| c |\n| - |\n| 3 |\n`, layers());
    mouseDown(cellEl(main, 1, 0));
    const second =
      main.dom.querySelectorAll<HTMLElement>('table[role="grid"]')[1];
    const target = second?.querySelector<HTMLElement>(
      '[aria-rowindex="2"][aria-colindex="1"]',
    );
    if (!target) throw new Error("no second table");
    mouseDown(target);
    expect(target.contains(hosted(main).view.dom)).toBe(true);
    expect(openText(main)).toBe("3");
    const first = cellEl(main, 1, 0);
    expect(first.querySelector(".cm-editor")).toBeNull();
    expect(first.textContent).toBe("1");
  });

  test("closing the cell draws its text again", () => {
    const main = soloMain(DOC, layers());
    typeIn(openByMouse(main, 1, 0), "x");
    closeCell(main, "user", "grid");
    const el = cellEl(main, 1, 0);
    expect(el.querySelector(".cm-editor")).toBeNull();
    expect(el.textContent).toBe("1x");
    expect(document.activeElement).toBe(grid(main));
  });

  test("a click on a link in a static cell opens the cell, not the link", () => {
    const main = soloMain(
      "| a | b |\n| - | - |\n| [x](https://example.com) | 2 |\n",
      layers(),
    );
    const anchor = cellEl(main, 1, 0).querySelector("a");
    if (!anchor) throw new Error("no anchor");
    mouseDown(anchor);
    expect(openText(main)).toBe("[x](https://example.com)");
  });

  test("a Mod-click on a link leaves the cell closed", () => {
    const main = soloMain(
      "| a | b |\n| - | - |\n| [x](https://example.com) | 2 |\n",
      layers(),
    );
    const anchor = cellEl(main, 1, 0).querySelector("a");
    if (!anchor) throw new Error("no anchor");
    const event = mouseDown(anchor, { metaKey: true, ctrlKey: true });
    expect(event.defaultPrevented).toBe(false);
    expect(main.state.field(openCellField)).toBeNull();
    expect(activeCell(main)).toBeNull();
  });

  test("the cell is re-read from the current state, not from the draw", () => {
    const main = soloMain(`Intro\n\n${DOC}`, layers());
    main.dispatch({ changes: { from: 0, insert: "Longer " } });
    const cell = openByMouse(main, 1, 1);
    expect(openText(main)).toBe("2");
    typeIn(cell, "y");
    expect(bytes(main.state)).toBe(`Longer Intro\n\n${DOC.replace("2", "2y")}`);
  });
});

describe("in-place patching and remounting", () => {
  test("a remote edit in another cell patches that cell and leaves the host alone", () => {
    const { main, rtext } = roomMain(DOC, layers());
    mouseDown(cellEl(main, 1, 0));
    const cell = hosted(main);
    const host = cellEl(main, 1, 0);
    const other = cellEl(main, 1, 1);
    rtext.insert(DOC.indexOf("2") + 1, "x");
    expect(cellEl(main, 1, 1)).toBe(other);
    expect(other.textContent).toBe("2x");
    expect(cellEl(main, 1, 0)).toBe(host);
    expect(hosted(main)).toBe(cell);
    expect(host.contains(cell.view.dom)).toBe(true);
    expect(hasFocus(cell.view)).toBe(true);
  });

  test("a remote added column remounts the editor in the same cell with focus and caret", async () => {
    const { main, rtext, remote } = roomMain(DOC, layers());
    const first = openByMouse(main, 1, 0);
    typeIn(first, "23");
    first.view.dispatch({
      selection: { anchor: first.view.state.selection.main.head - 1 },
    });
    const offset =
      first.view.state.selection.main.head -
      (openCellRange(main.state)?.content.from ?? 0);
    const text = rtext.toJSON();
    const ends = [...text.matchAll(/\|\n/g)].map((m) => m.index + 1).reverse();
    remote.transact(() => {
      ends.forEach((at, i) => {
        rtext.insert(at, i === ends.length - 2 ? " - |" : " c |");
      });
    });
    expect(rtext.toJSON()).toBe(
      "| a | b | c |\n| - | - | - |\n| 123 | 2 | c |\n",
    );
    const second = hosted(main);
    expect(second).not.toBe(first);
    expect(cellEl(main, 1, 0).contains(second.view.dom)).toBe(true);
    expect(cellEl(main, 1, 2).textContent).toBe("c");
    expect(
      second.view.state.selection.main.head -
        (openCellRange(main.state)?.content.from ?? 0),
    ).toBe(offset);
    await flush();
    expect(hasFocus(second.view)).toBe(true);
    expect(openText(main)).toBe("123");
  });

  test("a table scrolled out and back remounts the open cell with focus", async () => {
    const main = soloMain(DOC, layers());
    const first = openByMouse(main, 1, 1);
    first.view.dispatch({
      selection: { anchor: first.view.state.selection.main.head - 1 },
    });
    const caret = first.view.state.selection.main.head;
    const widget = widgetOf(main);
    const dom = grid(main).closest<HTMLElement>(".cm-grid");
    if (!dom) throw new Error("no grid root");
    // What virtualisation does: the widget's DOM is destroyed, later a new
    // one is built. The old DOM stays where it is, so main's own DOM
    // observer has nothing to repair; the new one is attached beside it.
    widget.destroy(dom);
    expect(activeCell(main)).toBeNull();
    expect(main.state.field(openCellField)).not.toBeNull();
    const again = widget.toDOM(main);
    document.body.appendChild(again);
    const second = hosted(main);
    expect(again.contains(second.view.dom)).toBe(true);
    expect(second.view.state.selection.main.head).toBe(caret);
    await flush();
    expect(hasFocus(second.view)).toBe(true);
    again.remove();
  });

  test("a table scrolled back without focus remounts without taking focus", async () => {
    const main = soloMain(DOC, layers());
    mouseDown(cellEl(main, 1, 1));
    main.focus();
    // CodeMirror reports a focus change a few milliseconds after the event.
    await vi.waitFor(() => {
      expect(main.state.field(openCellField)?.focused).toBe(false);
    });
    const widget = widgetOf(main);
    const dom = grid(main).closest<HTMLElement>(".cm-grid");
    if (!dom) throw new Error("no grid root");
    widget.destroy(dom);
    const again = widget.toDOM(main);
    document.body.appendChild(again);
    await flush();
    expect(hasFocus(hosted(main).view)).toBe(false);
    again.remove();
  });

  test("while composing, a same-shape remote change waits and never touches the host", () => {
    const { main, rtext } = roomMain(DOC, layers());
    mouseDown(cellEl(main, 1, 0));
    const cell = hosted(main);
    const host = cellEl(main, 1, 0);
    const other = cellEl(main, 1, 1);
    const hostChildren = [...host.childNodes];
    // jsdom has no IME; `composing` is what CodeMirror reports mid-composition.
    Object.defineProperty(cell.view, "composing", {
      get: () => true,
      configurable: true,
    });
    expect(cell.view.composing).toBe(true);
    const seen: MutationRecord[] = [];
    const observer = new MutationObserver((records) => seen.push(...records));
    observer.observe(host, { childList: true, attributes: true });
    rtext.insert(DOC.indexOf("2") + 1, "x");
    expect(other.textContent).toBe("2");
    expect([...host.childNodes]).toEqual(hostChildren);
    expect(observer.takeRecords().concat(seen)).toEqual([]);
    observer.disconnect();
    // The composition ends: the waiting patch lands.
    delete (cell.view as unknown as { composing?: boolean }).composing;
    cell.view.contentDOM.dispatchEvent(new CompositionEvent("compositionend"));
    expect(other.textContent).toBe("2x");
    expect(host.contains(cell.view.dom)).toBe(true);
    expect(hosted(main)).toBe(cell);
  });
});

describe("a patch waiting for a composition", () => {
  test("lands when the editor goes away before the composition ends", () => {
    const { main, rtext } = roomMain(DOC, layers());
    mouseDown(cellEl(main, 1, 0));
    const cell = hosted(main);
    Object.defineProperty(cell.view, "composing", {
      get: () => true,
      configurable: true,
    });
    rtext.insert(DOC.indexOf("2") + 1, "x");
    expect(cellEl(main, 1, 1).textContent).toBe("2");
    closeCell(main, "user", "none");
    expect(cellEl(main, 1, 1).textContent).toBe("2x");
    expect(cellEl(main, 1, 0).textContent).toBe("1");
  });
});

describe("teardown with the grid hosting the cell", () => {
  test("Raw while a grid cell has focus closes it as raw and focuses main", async () => {
    const grid = new Compartment();
    const main = soloMain(DOC, [grid.of(layers())]);
    typeIn(openByMouse(main, 1, 0), "x");
    const reasons: CloseReason[] = [];
    onCellClosed(main, (reason) => reasons.push(reason));
    main.dispatch({ effects: grid.reconfigure([]) });
    await flush();
    expect(reasons).toEqual(["raw"]);
    expect(activeCell(main)).toBeNull();
    expect(bytes(main.state)).toBe(DOC.replace("1", "1x"));
    expect(document.activeElement).toBe(main.contentDOM);
  });

  test("setState while a grid cell has focus closes it as swap", async () => {
    const main = soloMain(DOC, layers());
    mouseDown(cellEl(main, 1, 0));
    const reasons: CloseReason[] = [];
    onCellClosed(main, (reason) => reasons.push(reason));
    main.setState(
      parsedState(
        EditorState.create({
          doc: "| p | q |\n| - | - |\n| 3 | 4 |\n",
          extensions: [...baseExtensions(false), ...layers()],
        }),
      ),
    );
    await flush();
    expect(reasons).toEqual(["swap"]);
    expect(activeCell(main)).toBeNull();
    expect(cellEl(main, 1, 0).textContent).toBe("3");
    expect(document.activeElement).toBe(main.contentDOM);
  });

  test("the main view's destroy closes the cell as destroy", () => {
    const main = soloMain(DOC, layers());
    mouseDown(cellEl(main, 1, 0));
    const reasons: CloseReason[] = [];
    onCellClosed(main, (reason) => reasons.push(reason));
    main.destroy();
    expect(reasons).toEqual(["destroy"]);
  });
});
