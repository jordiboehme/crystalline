/**
 * Byte fidelity of the cell editor: opening a cell and typing one character
 * changes exactly that one character of the file, in LF and CRLF files, with
 * the main view's prose layers drawn in the cell; undo gives back the file
 * byte for byte. And the one teardown path: Raw and a buffer swap both close a
 * focused cell without losing what was typed.
 */

import { undo } from "@codemirror/commands";
import type { Extension } from "@codemirror/state";
import { Compartment, EditorState } from "@codemirror/state";
import { QueryClient } from "@tanstack/react-query";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import {
  bytes,
  cellOf,
  cleanupCells,
  openAt,
  soloMain,
  typeIn,
} from "../../test/cellKit";
import { parsedState } from "../../test/parse";
import { engramAdapter } from "../adapter";
import { baseExtensions, lineSeparatorFor } from "../setup";
import { previewConfig } from "../surface";
import { activeCell, cellForwarding } from "./cellEditor";
import { cellPreviewLayers } from "./draws";
import { closeCell, onCellClosed } from "./openCell";
import { gridTables } from "./structure";

const ENGRAMS = import.meta.glob<string>("../fixtures/**/*.md", {
  query: "?raw",
  import: "default",
  eager: true,
});

const SPECIAL = [
  "| Kind | Sample |",
  "| --- | --- |",
  "| pipe | a \\| b |",
  "| break | one<br>two |",
  "| link | see [[tide-tables]] |",
  "| label | :status[Done]{green} |",
  "| math | $x$ and $y^2$ |",
  "",
].join("\n");

const LF_FIXTURES: [string, string][] = [
  ...Object.entries(ENGRAMS),
  ["special", SPECIAL],
];

const FIXTURES: [string, string][] = [
  ...LF_FIXTURES,
  ...LF_FIXTURES.map(([name, text]): [string, string] => [
    `${name} (CRLF)`,
    text.replaceAll("\n", "\r\n"),
  ]),
];

const adapter = engramAdapter({
  domain: "harbor",
  client: new QueryClient(),
  vocab: () => null,
});

function layers(raw: boolean): Extension {
  return previewConfig(raw, false, [], adapter);
}

let errors: MockInstance<typeof console.error>;

beforeEach(() => {
  errors = vi.spyOn(console, "error");
});

afterEach(() => {
  cleanupCells();
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

test("the fixtures hold tables", () => {
  const counted = LF_FIXTURES.map(
    ([, text]) => gridTables(soloMain(text).state).length,
  );
  expect(counted.reduce((a, b) => a + b, 0)).toBeGreaterThanOrEqual(4);
});

describe.each(FIXTURES)("cell fidelity: %s", (_, text) => {
  const probe = parsedState(
    EditorState.create({
      doc: text,
      extensions: [...lineSeparatorFor(text), ...baseExtensions(false)],
    }),
  );
  const cells = gridTables(probe).flatMap((table, t) =>
    table.rows.flatMap((row) => row.map((cell) => [t, cell.row, cell.column])),
  );

  test.each(cells)(
    "table %i cell %i/%i takes one character and undoes to the bytes",
    (t, row, column) => {
      const main = soloMain(text, [layers(false), cellForwarding]);
      expect(main.state.facet(cellPreviewLayers).length).toBeGreaterThan(0);
      const before = main.state;
      const cell = gridTables(before)[t]?.rows[row]?.[column];
      if (!cell) throw new Error("cell vanished");
      const at = cell.content.to;
      const editor = openAt(main, at, false);
      typeIn(editor, "Z");
      const lb = before.lineBreak;
      const len = before.doc.length;
      const want =
        before.doc.sliceString(0, at, lb) +
        "Z" +
        before.doc.sliceString(at, len, lb);
      expect(bytes(main.state)).toBe(want);
      expect(editor.view.state.doc.eq(main.state.doc)).toBe(true);
      closeCell(main, "user", "none");
      expect(activeCell(main)).toBeNull();
      expect(undo(main)).toBe(true);
      expect(bytes(main.state)).toBe(text);
    },
  );
});

describe("one teardown path", () => {
  const DOC = "Intro\n\n| a | b |\n| - | - |\n| 1 | 2 |\n";

  function focusedCell() {
    const preview = new Compartment();
    const main = soloMain(DOC, [preview.of([layers(false), cellForwarding])]);
    const editor = openAt(main, cellOf(main, 1, 0).content.to, true);
    typeIn(editor, "x");
    const heard = vi.fn();
    onCellClosed(main, heard);
    return { main, editor, preview, heard };
  }

  test("Raw on while a cell holds focus keeps the text and focuses main", async () => {
    const { main, editor, preview, heard } = focusedCell();
    expect(document.activeElement).toBe(editor.view.contentDOM);
    main.dispatch({ effects: preview.reconfigure(layers(true)) });
    await Promise.resolve();
    expect(bytes(main.state)).toBe(
      "Intro\n\n| a | b |\n| - | - |\n| 1x | 2 |\n",
    );
    expect(activeCell(main)).toBeNull();
    expect(editor.view.dom.isConnected).toBe(false);
    expect(document.activeElement).toBe(main.contentDOM);
    expect(heard).toHaveBeenCalledTimes(1);
    expect(heard).toHaveBeenCalledWith("raw");
  });

  test("setState while a cell holds focus keeps the text and focuses main", async () => {
    const { main, editor, preview, heard } = focusedCell();
    main.setState(
      parsedState(
        EditorState.create({
          doc: main.state.doc,
          extensions: [
            ...baseExtensions(false),
            preview.of([layers(false), cellForwarding]),
          ],
        }),
      ),
    );
    await Promise.resolve();
    expect(bytes(main.state)).toBe(
      "Intro\n\n| a | b |\n| - | - |\n| 1x | 2 |\n",
    );
    expect(activeCell(main)).toBeNull();
    expect(editor.view.dom.isConnected).toBe(false);
    expect(document.activeElement).toBe(main.contentDOM);
    expect(heard).toHaveBeenCalledTimes(1);
    expect(heard).toHaveBeenCalledWith("swap");
  });
});
