/**
 * A format command in a table cell whose row is not read yet says the cell
 * is loading (D13), and says so before the command reads its own short
 * tree. Under load `toggleFormat`'s own parse of the result could fall
 * short first and refuse with "This text cannot be formatted here.". Here
 * every parse is starved on purpose while the keys are pressed (each look
 * at the clock finds its time budget spent), so the order is pinned at any
 * load.
 */

import { syntaxTree } from "@codemirror/language";
import { StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import {
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";

import { cleanupCells, openAt } from "../../test/cellKit";
import {
  bytes,
  cleanupMarkViews,
  keys,
  markView,
  typeText,
} from "../../test/markKit";
import { CELL_NOT_READ, cellParseBudget } from "./cellEditor";
import { prefetchGrid } from "./loader";
import { gridTables } from "./structure";

/**
 * While set, the clock jumps a second at every look, as on a machine too
 * busy to give a parse any time: a parse takes one step and stops. The
 * clock never goes back, so nothing that times itself sees time run
 * backwards afterwards. This relies on @codemirror/language 6.13.1 reading
 * `Date.now` for its parse budget (D26): if a later version times its
 * parse another way, this test no longer starves it.
 */
const parsing = { starved: false };

beforeAll(async () => {
  await prefetchGrid();
});

beforeEach(() => {
  const now = Date.now.bind(Date);
  let ahead = 0;
  vi.spyOn(Date, "now").mockImplementation(() => {
    if (parsing.starved) ahead += 1_000;
    return now() + ahead;
  });
});

afterEach(() => {
  parsing.starved = false;
  vi.restoreAllMocks();
  cleanupCells();
  cleanupMarkViews();
});

const ROOMS = [
  { name: "solo", room: false },
  { name: "in a room", room: true },
];

describe.each(ROOMS)(
  "a format command in an unread cell, $name",
  ({ room }) => {
    const above = Array.from(
      { length: 4000 },
      (_, i) => `Paragraph ${String(i)} with **bold** text.`,
    ).join("\n\n");
    const text = `${above}\n\n| a | b |\n| - | - |\n| label | plain words |\n`;

    it("says the cell is loading, before its own parse can refuse", () => {
      const main = markView(text, { mode: "hidden", room });
      main.dispatch({
        effects: StateEffect.appendConfig.of(cellParseBudget.of(0)),
      });
      const cell = gridTables(main.state)[0]?.rows[1]?.[1];
      if (!cell) throw new Error("no cell");
      const view = openAt(main, cell.content.from, false).view;
      expect(syntaxTree(view.state).length).toBeLessThan(cell.content.from);
      const said: string[] = [];
      view.dispatch({
        effects: StateEffect.appendConfig.of(
          EditorView.updateListener.of((update) => {
            for (const tr of update.transactions) {
              for (const effect of tr.effects) {
                if (effect.is(EditorView.announce)) said.push(effect.value);
              }
            }
          }),
        ),
      });
      view.dispatch({
        selection: { anchor: cell.content.from, head: cell.content.to },
      });
      parsing.starved = true;
      keys(view, "Mod-b");
      keys(view, "Mod-i");
      parsing.starved = false;
      expect(bytes(main.state)).toBe(text);
      // At a caret: no format waits, so the letter is plain.
      view.dispatch({ selection: { anchor: cell.content.to } });
      parsing.starved = true;
      keys(view, "Mod-b");
      parsing.starved = false;
      typeText(view, "x");
      expect(main.state.doc.lineAt(cell.content.from).text).toBe(
        "| label | plain wordsx |",
      );
      expect(said).toEqual([CELL_NOT_READ, CELL_NOT_READ, CELL_NOT_READ]);
    });
  },
);
