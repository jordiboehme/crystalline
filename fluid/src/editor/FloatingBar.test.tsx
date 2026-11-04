/**
 * The floating action bar: its text group on a selection, its text and table
 * groups in an open cell, and its keyboard (Alt+F10 in, arrows across,
 * Escape back with the selection as it was).
 */

import { undo } from "@codemirror/commands";
import type { Extension } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { Tooltips } from "../components/primitives";
import { bytes, cellOf, cleanupCells, soloMain } from "../test/cellKit";
import FloatingBar from "./FloatingBar";
import { fencePreviews } from "./fencePreviews";
import type { FloatingContext } from "./floatingContext";
import { floatingContextListener } from "./floatingContext";
import { activeCell } from "./grid/cellEditor";
import { tableAddRowBelow } from "./tableVerbs";
import { hostOpenCell } from "./grid/gridWidget";
import { gridExtension } from "./grid/index";
import { openCellRange, setOpenCell } from "./grid/openCell";
import { cleanupMarkViews, markView } from "../test/markKit";
import { imagePreviews } from "./imagePreviews";

const DOC =
  "Intro words\n\n| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |\n\nOutro\n";

let errors: MockInstance<typeof console.error>;

beforeEach(() => {
  errors = vi.spyOn(console, "error");
});

afterEach(() => {
  cleanupCells();
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

interface Harness {
  main: EditorView;
  context: () => FloatingContext;
  /** Renders the bar for the current context. */
  show: (wikilinks?: boolean) => void;
}

function harness(extra: Extension[] = []): Harness {
  let latest: FloatingContext = null;
  let rerender: ((ui: React.ReactElement) => void) | null = null;
  let wl = true;
  const ui = () => <FloatingBar main={main} context={latest} wikilinks={wl} />;
  const main = soloMain(DOC, [
    gridExtension(),
    fencePreviews(false),
    floatingContextListener((next) => {
      latest = next;
      if (rerender) {
        const draw = rerender;
        act(() => {
          draw(ui());
        });
      }
    }),
    ...extra,
  ]);
  return {
    main,
    context: () => latest,
    show: (wikilinks = true) => {
      wl = wikilinks;
      const result = render(ui(), { wrapper: Tooltips });
      rerender = result.rerender;
    },
  };
}

function bar(): HTMLElement {
  return screen.getByRole("toolbar", {
    name: /^(Quick format|Picture options)$/,
  });
}

function quick(name: string): HTMLElement {
  return within(bar()).getByRole("button", { name });
}

function openCell(main: EditorView, row: number, column: number): EditorView {
  const pos = cellOf(main, row, column).content.to;
  main.dispatch({
    effects: setOpenCell.of({ anchor: pos, head: pos, focused: true }),
  });
  const editor = hostOpenCell(main, "now");
  if (!editor) throw new Error("not hosted");
  return editor.view;
}

/** Gives `view` a layout: position `pos` sits at x = pos * 10 + `shift()`. */
function fakeLayout(view: EditorView, shift: () => number): void {
  view.coordsAtPos = (pos: number) => {
    const left = pos * 10 + shift();
    return { left, right: left, top: 100, bottom: 116 };
  };
}

function altF10(target: Element): void {
  fireEvent.keyDown(target, { key: "F10", altKey: true, keyCode: 121 });
}

describe("the floating bar on a text selection", () => {
  test("Bold wraps the selection in ** and keeps it selected", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    fireEvent.click(quick("Bold"));
    expect(bytes(h.main.state).startsWith("**Intro** words")).toBe(true);
    const sel = h.main.state.selection.main;
    expect(h.main.state.sliceDoc(sel.from, sel.to)).toBe("Intro");
  });

  test("offers only the text group, under the sticky bar's names", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    for (const name of [
      "Bold",
      "Italic",
      "Strikethrough",
      "Inline code",
      "Link",
      "Wiki link",
    ]) {
      expect(quick(name)).toBeInTheDocument();
    }
    expect(
      within(bar()).queryByRole("button", { name: "Insert row below" }),
    ).toBeNull();
  });

  test("a document without wiki links gets no Wiki link button", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show(false);
    expect(
      within(bar()).queryByRole("button", { name: "Wiki link" }),
    ).toBeNull();
  });

  test("the bar cancels its own mousedown", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    const event = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
    });
    quick("Italic").dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  });

  test("the bar hides while the pointer is down in the editor", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    fireEvent.pointerDown(h.main.dom, { isPrimary: true });
    expect(screen.queryByRole("toolbar", { name: "Quick format" })).toBeNull();
    fireEvent.pointerUp(document, { isPrimary: true });
    expect(bar()).toBeInTheDocument();
  });

  test("Escape in the text hides the bar until a new selection", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    fireEvent.keyDown(h.main.contentDOM, { key: "Escape" });
    expect(screen.queryByRole("toolbar", { name: "Quick format" })).toBeNull();
    // A press and release in the editor picks a selection again (jsdom draws
    // every selection at the same empty rect, so the press is what tells).
    // On the editor's frame: CodeMirror's own press handling, which would
    // move the selection, is not what is tested here.
    fireEvent.pointerDown(h.main.dom, { isPrimary: true });
    fireEvent.pointerUp(document, { isPrimary: true });
    expect(bar()).toBeInTheDocument();
  });

  test("after Escape, a rect that only moved does not bring the bar back", async () => {
    const h = harness();
    let shift = 0;
    fakeLayout(h.main, () => shift);
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    // Into the bar and back out: the selection stays, the bar goes.
    altF10(h.main.contentDOM);
    await vi.waitFor(() => {
      expect(quick("Bold")).toHaveFocus();
    });
    fireEvent.keyDown(quick("Bold"), { key: "Escape" });
    expect(screen.queryByRole("toolbar", { name: "Quick format" })).toBeNull();
    // A reflow (a peer's edit, a font load): the rect moves, nothing else.
    shift = 40;
    h.main.dispatch({
      changes: { from: h.main.state.doc.length, insert: "x" },
    });
    await vi.waitFor(() => {
      expect(h.context()?.rect.left).toBe(40);
    });
    expect(screen.queryByRole("toolbar", { name: "Quick format" })).toBeNull();
    // The person moving the selection is a new place again.
    h.main.dispatch({
      selection: { anchor: 0, head: 3 },
      userEvent: "select",
    });
    await vi.waitFor(() => {
      expect(bar()).toBeInTheDocument();
    });
  });

  test("a touch press hides the bar as a mouse press does", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    fireEvent.pointerDown(h.main.dom, {
      isPrimary: true,
      pointerType: "touch",
    });
    expect(screen.queryByRole("toolbar", { name: "Quick format" })).toBeNull();
    fireEvent.pointerUp(document, { isPrimary: true, pointerType: "touch" });
    expect(bar()).toBeInTheDocument();
  });

  test("a touch scroll that ends in a cancel shows the bar again", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    fireEvent.pointerDown(h.main.dom, {
      isPrimary: true,
      pointerType: "touch",
    });
    expect(screen.queryByRole("toolbar", { name: "Quick format" })).toBeNull();
    fireEvent.pointerCancel(document, {
      isPrimary: true,
      pointerType: "touch",
    });
    expect(bar()).toBeInTheDocument();
  });

  test("a selection that spans a table offers no bar", async () => {
    const h = harness();
    h.main.dispatch({
      selection: { anchor: 0, head: h.main.state.doc.length },
    });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(h.context()).toBeNull();
  });

  test("the bar hides when the focus leaves the editor", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    act(() => {
      outside.focus();
    });
    expect(screen.queryByRole("toolbar", { name: "Quick format" })).toBeNull();
    act(() => {
      h.main.focus();
    });
    expect(bar()).toBeInTheDocument();
    outside.remove();
  });

  test("Alt+F10 focuses the first button, arrows move, Escape goes back", async () => {
    const h = harness();
    h.main.focus();
    h.main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("text");
    });
    h.show();
    const before = h.main.state.selection;
    // The bar never takes focus on its own.
    expect(bar().contains(document.activeElement)).toBe(false);
    altF10(h.main.contentDOM);
    await vi.waitFor(() => {
      expect(quick("Bold")).toHaveFocus();
    });
    // One tab stop: the roving tabindex.
    expect(quick("Bold")).toHaveAttribute("tabindex", "0");
    expect(quick("Italic")).toHaveAttribute("tabindex", "-1");
    fireEvent.keyDown(quick("Bold"), { key: "ArrowRight" });
    expect(quick("Italic")).toHaveFocus();
    fireEvent.keyDown(quick("Italic"), { key: "ArrowLeft" });
    expect(quick("Bold")).toHaveFocus();
    fireEvent.keyDown(quick("Bold"), { key: "End" });
    expect(quick("Wiki link")).toHaveFocus();
    fireEvent.keyDown(quick("Wiki link"), { key: "Home" });
    expect(quick("Bold")).toHaveFocus();
    fireEvent.keyDown(quick("Bold"), { key: "Escape" });
    expect(h.main.hasFocus).toBe(true);
    expect(h.main.state.selection.eq(before)).toBe(true);
  });
});

describe("the floating bar in an open cell", () => {
  test("Bold wraps the cell selection inside the cell only", async () => {
    const h = harness();
    const cell = openCell(h.main, 2, 1);
    const { from, to } = cellOf(h.main, 2, 1).content;
    cell.dispatch({ selection: { anchor: from, head: to } });
    await vi.waitFor(() => {
      expect(h.context()).toMatchObject({ kind: "cell", selection: true });
    });
    h.show();
    fireEvent.click(quick("Bold"));
    expect(bytes(h.main.state)).toBe(DOC.replace("| 3 | 4 |", "| 3 | **4** |"));
    expect(activeCell(h.main)?.view.hasFocus).toBe(true);
  });

  test("Insert row below adds the row below the open cell's row", async () => {
    const h = harness();
    openCell(h.main, 1, 0);
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("cell");
    });
    h.show();
    fireEvent.click(quick("Insert row below"));
    const plain = soloMain(DOC);
    expect(tableAddRowBelow(plain, cellOf(plain, 1, 0).content.from)).toBe(
      true,
    );
    expect(bytes(h.main.state)).toBe(bytes(plain.state));
    // The new row's cell is open, and one undo takes the row away again.
    expect(openCellRange(h.main.state)).toMatchObject({ row: 2, column: 0 });
    undo(h.main);
    expect(bytes(h.main.state)).toBe(DOC);
  });

  test("the cell group adds the table verbs and More table actions", async () => {
    const h = harness();
    openCell(h.main, 1, 0);
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("cell");
    });
    h.show();
    for (const name of [
      "Bold",
      "Insert row below",
      "Insert column right",
      "Delete row",
      "Delete column",
      "Align",
      "More table actions",
    ]) {
      expect(quick(name)).toBeInTheDocument();
    }
    fireEvent.click(quick("More table actions"));
    expect(
      document.querySelector('[role="menu"][aria-label="Cell actions"]'),
    ).not.toBeNull();
  });

  test("Delete row is off on the header row", async () => {
    const h = harness();
    openCell(h.main, 0, 0);
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("cell");
    });
    h.show();
    expect(quick("Delete row")).toBeDisabled();
  });

  test("Alt+F10 from the cell and Escape back keep the cell's selection", async () => {
    const h = harness();
    const cell = openCell(h.main, 1, 1);
    const { from, to } = cellOf(h.main, 1, 1).content;
    cell.dispatch({ selection: { anchor: from, head: to } });
    await vi.waitFor(() => {
      expect(h.context()).toMatchObject({ kind: "cell", selection: true });
    });
    h.show();
    altF10(cell.contentDOM);
    await vi.waitFor(() => {
      expect(quick("Bold")).toHaveFocus();
    });
    fireEvent.keyDown(quick("Bold"), { key: "ArrowRight" });
    expect(quick("Italic")).toHaveFocus();
    fireEvent.keyDown(quick("Italic"), { key: "Escape" });
    const view = activeCell(h.main)?.view;
    expect(view?.hasFocus).toBe(true);
    expect(view?.state.selection.main).toMatchObject({
      anchor: from,
      head: to,
    });
  });

  test("typing after Escape back into the cell keeps the bar hidden", async () => {
    const h = harness();
    const cell = openCell(h.main, 1, 1);
    fakeLayout(cell, () => 0);
    cell.dispatch({ selection: { anchor: cell.state.selection.main.head } });
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("cell");
    });
    h.show();
    altF10(cell.contentDOM);
    await vi.waitFor(() => {
      expect(quick("Bold")).toHaveFocus();
    });
    fireEvent.keyDown(quick("Bold"), { key: "Escape" });
    expect(cell.hasFocus).toBe(true);
    const before = h.context()?.rect.left;
    const head = cell.state.selection.main.head;
    cell.dispatch({
      changes: { from: head, insert: "z" },
      selection: { anchor: head + 1 },
      userEvent: "input.type",
    });
    await vi.waitFor(() => {
      expect(h.context()?.rect.left).not.toBe(before);
    });
    expect(screen.queryByRole("toolbar", { name: "Quick format" })).toBeNull();
  });

  test("a bar reached by Alt+F10 does not keep a closed cell to return to", async () => {
    const h = harness();
    const cell = openCell(h.main, 1, 0);
    await vi.waitFor(() => {
      expect(h.context()?.kind).toBe("cell");
    });
    h.show();
    altF10(cell.contentDOM);
    await vi.waitFor(() => {
      expect(quick("Bold")).toHaveFocus();
    });
    // The verb lands in the new row's cell; the old cell view is gone.
    fireEvent.click(quick("Insert row below"));
    const landed = activeCell(h.main)?.view;
    expect(landed).toBeDefined();
    expect(landed).not.toBe(cell);
    // Back in the bar by Tab rather than Alt+F10, then Escape.
    act(() => {
      quick("Bold").focus();
    });
    fireEvent.keyDown(quick("Bold"), { key: "Escape" });
    expect(landed?.hasFocus).toBe(true);
  });
});

describe("the floating bar on a picture", () => {
  afterEach(cleanupMarkViews);

  /** A view with the picture layer, its picture selected, and the bar drawn. */
  async function onPicture(doc: string, ref: string): Promise<EditorView> {
    let latest: FloatingContext = null;
    const v = markView(doc, {
      images: imagePreviews("eng"),
      onFloatingContext: (next) => {
        latest = next;
      },
    });
    const from = doc.indexOf(ref);
    v.focus();
    v.dispatch({ selection: { anchor: from, head: from + ref.length } });
    await vi.waitFor(() => {
      expect(latest?.kind).toBe("image");
    });
    render(<FloatingBar main={v} context={latest} wikilinks />, {
      wrapper: Tooltips,
    });
    return v;
  }

  const picture = () =>
    screen.getByRole("toolbar", { name: "Picture options" });

  test("is named Picture options and offers no text group", async () => {
    await onPicture("![C](assets/c.png)\n", "![C](assets/c.png)");
    expect(screen.queryByRole("toolbar", { name: "Quick format" })).toBeNull();
    expect(
      within(picture()).queryByRole("button", { name: "Bold" }),
    ).toBeNull();
  });

  test("Alt+F10 focuses Left with text wrap, the one tab stop", async () => {
    const v = await onPicture("![C](assets/c.png)\n", "![C](assets/c.png)");
    altF10(v.contentDOM);
    const first = within(picture()).getByRole("button", {
      name: "Left with text wrap",
    });
    await vi.waitFor(() => {
      expect(first).toHaveFocus();
    });
    expect(first).toHaveAttribute("tabindex", "0");
    expect(
      within(picture()).getByRole("textbox", { name: "Alt text" }),
    ).toHaveAttribute("tabindex", "-1");
    fireEvent.keyDown(first, { key: "End" });
    expect(
      within(picture()).getByRole("button", { name: "Remove picture" }),
    ).toHaveFocus();
    fireEvent.keyDown(document.activeElement ?? first, { key: "Escape" });
    expect(v.hasFocus).toBe(true);
    const sel = v.state.selection.main;
    expect(v.state.sliceDoc(sel.from, sel.to)).toBe("![C](assets/c.png)");
  });

  test("Alt+F10 on an external picture focuses the alt field", async () => {
    const ref = "![M](https://example.com/m.png)";
    const v = await onPicture(`${ref}\n`, ref);
    altF10(v.contentDOM);
    await vi.waitFor(() => {
      expect(
        within(picture()).getByRole("textbox", { name: "Alt text" }),
      ).toHaveFocus();
    });
  });
});

describe("placement by an open cell (finding 3)", () => {
  const box = (top: number, bottom: number, left = 20) =>
    new DOMRect(left, top, 200, bottom - top);

  /** Renders the bar for `context`, with a layout jsdom does not have. */
  function placed(context: FloatingContext): HTMLElement {
    const main = soloMain(DOC, [gridExtension(), fencePreviews(false)]);
    main.focus();
    const stubs = [
      vi
        .spyOn(HTMLElement.prototype, "offsetParent", "get")
        .mockReturnValue(document.body),
      vi
        .spyOn(HTMLElement.prototype, "offsetHeight", "get")
        .mockReturnValue(30),
      vi
        .spyOn(HTMLElement.prototype, "offsetWidth", "get")
        .mockReturnValue(100),
      vi
        .spyOn(document.body, "getBoundingClientRect")
        .mockReturnValue(new DOMRect(0, 0, 1000, 800)),
    ];
    try {
      render(<FloatingBar main={main} context={context} wikilinks />, {
        wrapper: Tooltips,
      });
    } finally {
      for (const stub of stubs) stub.mockRestore();
    }
    return bar();
  }

  test("stands above the table, not above the caret's row", () => {
    const el = placed({
      kind: "cell",
      rect: box(300, 400),
      caret: box(350, 366, 80),
      selection: false,
    });
    expect(el.style.top).toBe("264px");
    expect(el.style.left).toBe("20px");
  });

  test("goes below the table with no room above it", () => {
    const el = placed({
      kind: "cell",
      rect: box(10, 400),
      caret: box(350, 366, 80),
      selection: false,
    });
    expect(el.style.top).toBe("406px");
  });

  test("stands above a picture, and below it with no room above", () => {
    const image = (rect: DOMRect): FloatingContext => ({
      kind: "image",
      rect,
      from: 0,
      to: 0,
      formattable: true,
    });
    expect(placed(image(box(300, 400))).style.top).toBe("264px");
    cleanup();
    expect(placed(image(box(10, 400))).style.top).toBe("406px");
  });

  test("goes by the caret when the table leaves no room on either side", () => {
    const el = placed({
      kind: "cell",
      rect: box(10, 760),
      caret: box(350, 366, 80),
      selection: false,
    });
    expect(el.style.top).toBe("314px");
    expect(el.style.left).toBe("80px");
  });
});

describe("the format buttons over hidden marks", () => {
  afterEach(cleanupMarkViews);

  test("Bold formats the visible characters of the selection", () => {
    const v = markView("ab **cd** ef");
    v.focus();
    v.dispatch({ selection: { anchor: 1, head: 6 } });
    render(
      <FloatingBar
        main={v}
        context={{ kind: "text", rect: new DOMRect() }}
        wikilinks
        formats={new Set(["strong"])}
      />,
      { wrapper: Tooltips },
    );
    fireEvent.click(quick("Bold"));
    expect(bytes(v.state)).toBe("a**b cd** ef");
  });

  test("the four format buttons say which formats the selection has", () => {
    const v = markView("**hello**");
    v.focus();
    v.dispatch({ selection: { anchor: 2, head: 7 } });
    render(
      <FloatingBar
        main={v}
        context={{ kind: "text", rect: new DOMRect() }}
        wikilinks
        formats={new Set(["strong"])}
      />,
      { wrapper: Tooltips },
    );
    expect(quick("Bold")).toHaveAttribute("aria-pressed", "true");
    expect(quick("Italic")).toHaveAttribute("aria-pressed", "false");
    expect(quick("Strikethrough")).toHaveAttribute("aria-pressed", "false");
    expect(quick("Inline code")).toHaveAttribute("aria-pressed", "false");
    expect(quick("Link")).not.toHaveAttribute("aria-pressed");
  });

  test("with no formats told, the format buttons are not toggles", () => {
    const v = markView("**hello**");
    v.focus();
    v.dispatch({ selection: { anchor: 2, head: 7 } });
    render(
      <FloatingBar
        main={v}
        context={{ kind: "text", rect: new DOMRect() }}
        wikilinks
      />,
      { wrapper: Tooltips },
    );
    expect(quick("Bold")).not.toHaveAttribute("aria-pressed");
  });
});
