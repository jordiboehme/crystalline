/**
 * What the floating action bar is told: a text selection outside a table,
 * the caret or a selection in the open cell, or nothing. The context's kind
 * never depends on layout, so jsdom (which has none) still decides it; the
 * rectangle falls back to an empty one here.
 */

import type { Extension } from "@codemirror/state";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { cellOf, cleanupCells, soloMain } from "../test/cellKit";
import { fencePreviews } from "./fencePreviews";
import type { FloatingContext } from "./floatingContext";
import { floatingContextListener, watchDocChanges } from "./floatingContext";
import { hostOpenCell } from "./grid/gridWidget";
import { gridExtension } from "./grid/index";
import { closeCell, setOpenCell } from "./grid/openCell";
import { imagePreviews } from "./imagePreviews";

const DOC = "Intro words\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\nOutro\n";

let errors: MockInstance<typeof console.error>;

beforeEach(() => {
  errors = vi.spyOn(console, "error");
});

afterEach(() => {
  cleanupCells();
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

/** A main view with the grid and the listener; `seen` holds every report. */
function setup(doc = DOC, extra: Extension[] = []) {
  const seen: FloatingContext[] = [];
  const main = soloMain(doc, [
    gridExtension(),
    fencePreviews(false),
    floatingContextListener((context) => {
      seen.push(context);
    }),
    ...extra,
  ]);
  const last = () => seen.at(-1);
  return { main, seen, last };
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

describe("floatingContextListener", () => {
  test("a non-empty selection outside a table reports text", async () => {
    const { main, last } = setup();
    main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("text");
    });
    expect(last()).toMatchObject({ kind: "text" });
    const context = last();
    expect(context && context.rect).toBeInstanceOf(DOMRect);
  });

  test("an empty selection reports null", async () => {
    const { main, last, seen } = setup();
    main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("text");
    });
    main.dispatch({ selection: { anchor: 3 } });
    await vi.waitFor(() => {
      expect(last()).toBeNull();
    });
    // The first look always reports, then only changes do.
    expect(seen.filter((c) => c === null).length).toBeGreaterThanOrEqual(1);
  });

  test("an open cell reports cell, with whether it holds a selection", async () => {
    const { main, last } = setup();
    const cell = openCell(main, 1, 0);
    await vi.waitFor(() => {
      expect(last()).toMatchObject({ kind: "cell", selection: false });
    });
    const { from, to } = cellOf(main, 1, 0).content;
    cell.dispatch({ selection: { anchor: from, head: to } });
    await vi.waitFor(() => {
      expect(last()).toMatchObject({ kind: "cell", selection: true });
    });
  });

  test("an open cell wins over a stale main selection", async () => {
    const { main, last } = setup();
    main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("text");
    });
    openCell(main, 1, 1);
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("cell");
    });
  });

  test("closing the cell reports null", async () => {
    const { main, last } = setup();
    openCell(main, 1, 0);
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("cell");
    });
    closeCell(main, "user", "main");
    await vi.waitFor(() => {
      expect(last()).toBeNull();
    });
  });

  test("a selection inside a table's source reports null", async () => {
    // A table inside a quote is not drawn: its text is selectable in main.
    const quoted = "Intro\n\n> | a | b |\n> | - | - |\n> | 1 | 2 |\n";
    const { main, last } = setup(quoted);
    const at = quoted.indexOf("| 1");
    main.dispatch({ selection: { anchor: at, head: at + 3 } });
    await vi.waitFor(() => {
      expect(last()).toBeNull();
    });
  });

  test("a read-only page reports null for a selection", async () => {
    const { main, last } = setup(DOC, [EditorState.readOnly.of(true)]);
    main.dispatch({ selection: { anchor: 0, head: 5 } });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(last()).toBeNull();
  });

  test("a page that turns read-only drops the context", async () => {
    const lock = new Compartment();
    const { main, last } = setup(DOC, [lock.of([])]);
    main.dispatch({ selection: { anchor: 0, head: 5 } });
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("text");
    });
    // The way an ended room turns the page off: a reconfigure, with the
    // selection left where it was.
    main.dispatch({
      effects: lock.reconfigure([
        EditorView.editable.of(false),
        EditorState.readOnly.of(true),
      ]),
    });
    await vi.waitFor(() => {
      expect(last()).toBeNull();
    });
  });
});

describe("the cell context's rect (finding 3)", () => {
  function tableBox(main: EditorView, rect: DOMRect): void {
    const table = main.dom.querySelector("table");
    if (!table) throw new Error("no table");
    table.getBoundingClientRect = () => rect;
  }

  test("is the table's box, with the caret's beside it", async () => {
    const { main, last } = setup();
    tableBox(main, new DOMRect(10, 200, 300, 120));
    openCell(main, 1, 0);
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("cell");
    });
    const context = last();
    if (context?.kind !== "cell") throw new Error("no cell context");
    expect([context.rect.top, context.rect.bottom]).toEqual([200, 320]);
    expect(context.caret).toBeInstanceOf(DOMRect);
  });

  test("is the caret's for a table taller than the viewport", async () => {
    const { main, last } = setup();
    tableBox(main, new DOMRect(10, -50, 300, window.innerHeight + 100));
    openCell(main, 1, 0);
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("cell");
    });
    const context = last();
    if (context?.kind !== "cell") throw new Error("no cell context");
    expect(context.rect.top).toBe(context.caret.top);
  });
});

describe("the picture context", () => {
  const ATTACHED = "![Chart](assets/c.png)";
  const EXTERNAL = "![Map](https://example.com/m.png)";
  const PICTURES = `Words ${ATTACHED} and ${EXTERNAL}.\n\n[${"![In](assets/i.png)"}](https://x.y)\n`;

  function pictures(doc = PICTURES) {
    return setup(doc, [imagePreviews("eng")]);
  }

  function select(main: EditorView, from: number, to: number): void {
    main.dispatch({ selection: { anchor: from, head: to } });
  }

  test("a selected attachment reports image, formattable", async () => {
    const { main, last } = pictures();
    const from = PICTURES.indexOf(ATTACHED);
    select(main, from, from + ATTACHED.length);
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("image");
    });
    expect(last()).toMatchObject({
      kind: "image",
      from,
      to: from + ATTACHED.length,
      formattable: true,
    });
  });

  test("a selected external picture reports image, not formattable", async () => {
    const { main, last } = pictures();
    const from = PICTURES.indexOf(EXTERNAL);
    select(main, from, from + EXTERNAL.length);
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("image");
    });
    expect(last()).toMatchObject({ kind: "image", formattable: false });
  });

  test("a selection one character longer than the picture reports text", async () => {
    const { main, last } = pictures();
    const from = PICTURES.indexOf(ATTACHED);
    select(main, from, from + ATTACHED.length + 1);
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("text");
    });
  });

  test("a picture inside a link reports text", async () => {
    const { main, last } = pictures();
    const from = PICTURES.indexOf("![In]");
    select(main, from, from + "![In](assets/i.png)".length);
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("text");
    });
  });

  test("collapsing the selection reports null", async () => {
    const { main, last } = pictures();
    const from = PICTURES.indexOf(ATTACHED);
    select(main, from, from + ATTACHED.length);
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("image");
    });
    main.dispatch({ selection: { anchor: from } });
    await vi.waitFor(() => {
      expect(last()).toBeNull();
    });
  });

  test("with no pictures drawn, a selected reference is text", async () => {
    const { main, last } = setup(PICTURES);
    const from = PICTURES.indexOf(ATTACHED);
    select(main, from, from + ATTACHED.length);
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("text");
    });
  });

  test("a change that keeps the rect reports the picture's new place", async () => {
    const { main, last } = pictures();
    const from = PICTURES.indexOf(ATTACHED);
    select(main, from, from + ATTACHED.length);
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("image");
    });
    // jsdom has no layout: every rect is the empty one, so only the
    // positions tell the two contexts apart (D8).
    main.dispatch({ changes: { from: 0, insert: "More " } });
    await vi.waitFor(() => {
      expect(last()).toMatchObject({ kind: "image", from: from + 5 });
    });
  });

  test("the rect is the drawn image's box", async () => {
    const { main, last } = pictures();
    const from = PICTURES.indexOf(ATTACHED);
    select(main, from, from + ATTACHED.length);
    await vi.waitFor(() => {
      expect(
        main.contentDOM.querySelector(".cm-picture-selected img"),
      ).not.toBeNull();
    });
    const image = main.contentDOM.querySelector(".cm-picture-selected img");
    if (!(image instanceof HTMLElement)) throw new Error("no drawn image");
    image.getBoundingClientRect = () => new DOMRect(40, 300, 200, 100);
    main.dispatch({ selection: { anchor: 0 } });
    select(main, from, from + ATTACHED.length);
    await vi.waitFor(() => {
      expect(last()?.kind).toBe("image");
    });
    const context = last();
    if (context?.kind !== "image") throw new Error("no image context");
    expect([context.rect.left, context.rect.top, context.rect.bottom]).toEqual([
      40, 300, 400,
    ]);
  });
});

describe("the picture context, round 1", () => {
  test("a picture in a table cell gets no picture context", async () => {
    const doc = "| a | b |\n| - | - |\n| ![C](assets/c.png) | 2 |\n";
    const { main, last, seen } = setup(doc, [imagePreviews("eng")]);
    const from = doc.indexOf("![C]");
    main.dispatch({
      selection: { anchor: from, head: from + "![C](assets/c.png)".length },
    });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(seen.some((context) => context?.kind === "image")).toBe(false);
    expect(last()).toBeNull();
  });

  test("a picture that stops being an attachment reports formattable false", async () => {
    const attached = "![C](assets/abcdefghijklm.png)";
    const external = "![C](https://e.org/abcdef.png)";
    expect(external.length).toBe(attached.length);
    const doc = `${attached}\n`;
    const { main, last } = setup(doc, [imagePreviews("eng")]);
    main.dispatch({ selection: { anchor: 0, head: attached.length } });
    await vi.waitFor(() => {
      expect(last()).toMatchObject({ kind: "image", formattable: true });
    });
    main.dispatch({
      changes: {
        from: 5,
        to: attached.length - 1,
        insert: external.slice(5, -1),
      },
    });
    await vi.waitFor(() => {
      expect(last()).toMatchObject({ kind: "image", formattable: false });
    });
  });
});

describe("watchDocChanges", () => {
  test("hands every change of main to a watcher until it stops", () => {
    const { main } = setup();
    const seen: number[] = [];
    const stop = watchDocChanges(main, (changes) => {
      seen.push(changes === null ? -1 : changes.mapPos(0, 1));
    });
    main.dispatch({ changes: { from: 0, insert: "ab" } });
    main.dispatch({ selection: { anchor: 1 } });
    stop();
    main.dispatch({ changes: { from: 0, insert: "c" } });
    expect(seen).toEqual([2]);
  });

  test("tells a watcher when the state is replaced", () => {
    const { main } = setup();
    const seen: unknown[] = [];
    watchDocChanges(main, (changes) => {
      seen.push(changes);
    });
    main.setState(main.state);
    expect(seen).toEqual([null]);
  });
});
