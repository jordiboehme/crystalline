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
import { floatingContextListener } from "./floatingContext";
import { hostOpenCell } from "./grid/gridWidget";
import { gridExtension } from "./grid/index";
import { closeCell, setOpenCell } from "./grid/openCell";

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
