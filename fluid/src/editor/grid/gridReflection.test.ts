/**
 * A table right under a Reflection section's hidden id line, drawn as the
 * grid on the real surface: the id line stays hidden and byte-identical
 * through a cell edit, and Backspace at a cell's start changes nothing outside
 * the cell. The spike's probe 4, kept.
 */

import type { EditorView } from "@codemirror/view";
import { Compartment, EditorState } from "@codemirror/state";
import { EditorView as View } from "@codemirror/view";
import type { MockInstance } from "vitest";
import { afterEach, beforeAll, beforeEach, expect, test, vi } from "vitest";

import { bytes, press, typeIn } from "../../test/cellKit";
import { parsedState } from "../../test/parse";
import { reflectionAdapter } from "../adapter";
import { hidesIdLines } from "../hiddenIdLines";
import { surfaceExtensions } from "../surface";
import { activeCell } from "./cellEditor";
import { prefetchGrid } from "./loader";
import { openCellRange } from "./openCell";

const ID_LINE = "<!-- r:aaaa1111 -->";
const DOC = `## One\n${ID_LINE}\n| a | b |\n| - | - |\n| 1 | 2 |\n`;

let errors: MockInstance<typeof console.error>;
let main: EditorView | null = null;

beforeAll(async () => {
  // The chunk is in before the state is built, as on a second visit.
  await prefetchGrid();
});

beforeEach(() => {
  errors = vi.spyOn(console, "error");
});

afterEach(() => {
  main?.destroy();
  main = null;
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

function reflection(): EditorView {
  main = new View({
    state: parsedState(
      EditorState.create({
        doc: DOC,
        extensions: [
          EditorState.lineSeparator.of("\n"),
          ...surfaceExtensions({
            adapter: reflectionAdapter(),
            dark: false,
            history: true,
            screen: [],
            preview: new Compartment(),
            raw: false,
            images: [],
            uploads: [],
            onTableContext: () => {},
            onImageContext: () => {},
            onElementContext: () => {},
          }),
        ],
      }),
    ),
    parent: document.body,
  });
  return main;
}

function open(view: EditorView, row: number, column: number) {
  const el = view.dom.querySelector<HTMLElement>(
    `[role="grid"] [aria-rowindex="${String(row + 1)}"][aria-colindex="${String(column + 1)}"]`,
  );
  if (!el) throw new Error("no grid cell");
  el.dispatchEvent(
    new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }),
  );
  const cell = activeCell(view);
  const range = openCellRange(view.state);
  if (!cell || !range) throw new Error("no cell editor");
  // jsdom has no layout: put the caret at the end rather than under the
  // pointer.
  cell.view.dispatch({ selection: { anchor: range.content.to } });
  return cell;
}

/** The document outside the open cell's content, before and after it. */
function outside(view: EditorView): [string, string] {
  const cell = openCellRange(view.state);
  if (!cell) throw new Error("no open cell");
  const text = bytes(view.state);
  return [text.slice(0, cell.content.from), text.slice(cell.content.to)];
}

test("a table under a hidden id line is a grid and the id line stays hidden", () => {
  const view = reflection();
  expect(hidesIdLines(view.state)).toBe(true);
  expect(view.dom.querySelector('[role="grid"]')).not.toBeNull();
  expect(view.contentDOM.textContent).not.toContain("r:aaaa1111");
  expect(view.state.doc.line(2).text).toBe(ID_LINE);
});

test("a cell edit leaves the id line byte-identical", () => {
  const view = reflection();
  const cell = open(view, 1, 0);
  typeIn(cell, "x");
  expect(bytes(view.state)).toBe(DOC.replace("| 1 |", "| 1x |"));
  expect(view.state.doc.line(2).text).toBe(ID_LINE);
  expect(hidesIdLines(view.state)).toBe(true);
});

test("Backspace at a data cell's start changes nothing outside the cell", () => {
  const view = reflection();
  const cell = open(view, 1, 0);
  const range = openCellRange(view.state);
  if (!range) throw new Error("no open cell");
  cell.view.dispatch({ selection: { anchor: range.content.from } });
  const before = bytes(view.state);
  expect(press(cell.view, "Backspace")).toBe(true);
  press(cell.view, "Backspace");
  expect(bytes(view.state)).toBe(before);
});

test("Backspace in the header cell under the id line keeps the id line", () => {
  const view = reflection();
  const cell = open(view, 0, 0);
  expect(
    view.state.doc.lineAt(openCellRange(view.state)?.content.from ?? 0).number,
  ).toBe(3);
  typeIn(cell, "x");
  const kept = outside(view);
  // Two presses empty the cell, the rest would reach the hidden heading.
  for (let i = 0; i < 5; i += 1) press(cell.view, "Backspace");
  expect(openCellRange(view.state)?.text).toBe("");
  expect(outside(view)).toEqual(kept);
  expect(view.state.doc.line(1).text).toBe("## One");
  expect(view.state.doc.line(2).text).toBe(ID_LINE);
  expect(hidesIdLines(view.state)).toBe(true);
});
