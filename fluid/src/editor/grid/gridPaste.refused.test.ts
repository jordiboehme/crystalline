/**
 * A block paste whose grown table stops parsing (final review finding 9):
 * the error stays inside the paste handler, and the paste is typed into the
 * open cell as one value instead of being lost. The table verb is made to
 * break the table with a module mock (it writes over the delimiter line),
 * because no real verb does.
 */

import type { Extension } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { bytes, cleanupCells, soloMain } from "../../test/cellKit";
import { fencePreviews } from "../fencePreviews";
import type { SpanChange, TableModel } from "../tableModel";
import { hostOpenCell } from "./gridWidget";
import { gridExtension } from "./index";
import { setOpenCell } from "./openCell";
import { gridTables } from "./structure";
import { parsedState } from "../../test/parse";

vi.mock("../tableModel", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../tableModel")>();
  return {
    ...actual,
    // The delimiter line written over: the scratch table no longer parses.
    addColumnAfter: (model: TableModel): SpanChange[] => {
      const delimiter = model.lines[1];
      if (!delimiter) return [];
      return [
        {
          from: delimiter.start,
          to: delimiter.start + delimiter.text.length,
          insert: "not a delimiter",
        },
      ];
    },
  };
});

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

function layers(): Extension[] {
  return [gridExtension(), fencePreviews(false)];
}

function open(main: EditorView, row: number, column: number): EditorView {
  const cell = gridTables(parsedState(main.state))[0]?.rows[row]?.[column];
  if (!cell) throw new Error("no cell");
  main.dispatch({
    effects: setOpenCell.of({
      anchor: cell.content.to,
      head: cell.content.to,
      focused: true,
    }),
  });
  const editor = hostOpenCell(main, "now");
  if (!editor) throw new Error("not hosted");
  return editor.view;
}

function paste(view: EditorView, plain: string): void {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    value: {
      getData: (type: string) => (type === "text/plain" ? plain : ""),
      files: [],
      items: [],
      types: [],
    },
  });
  view.contentDOM.dispatchEvent(event);
}

describe("a block that cannot be filled", () => {
  test("is typed into the cell as one value", () => {
    const main = soloMain(DOC, layers());
    // Three columns wide: the table has to grow, through the broken verb.
    const view = open(main, 1, 1);
    paste(view, "x\ty\tz");
    expect(bytes(main.state)).toBe("| a | b |\n| - | - |\n| 1 | 2x\ty\tz |\n");
  });
});
