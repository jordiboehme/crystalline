/**
 * A press in a cell right after a reparse that stopped above its row.
 *
 * A document change parses for 20 ms of `Date.now()` (`LanguageState.apply`)
 * and keeps what it finished. A pause of the page in that time, a slow phone
 * or a long engram leave the view's tree short of the row, until the
 * background parse catches up. A press must not read that as "the row is not
 * read": it has a parse budget of its own (D35), and only a press whose
 * budget ran out says the cell is loading.
 *
 * The clock is stubbed, so all of this holds at any load: while `starved`,
 * each look at it finds a second gone, and a parse takes one step and stops.
 * This relies on @codemirror/language 6.13.1 reading `Date.now` for its parse
 * budget (D26), as `cellFormatGate.test.ts` does.
 */

import { syntaxTree } from "@codemirror/language";
import { EditorSelection, StateEffect } from "@codemirror/state";
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
  setCaret,
  typeText,
} from "../../test/markKit";
import { parsedState } from "../../test/parse";
import { CELL_NOT_READ } from "./cellEditor";
import { prefetchGrid } from "./loader";
import { gridTables } from "./structure";

const clock = { starved: false };

beforeAll(async () => {
  await prefetchGrid();
});

beforeEach(() => {
  const now = Date.now.bind(Date);
  let ahead = 0;
  vi.spyOn(Date, "now").mockImplementation(() => {
    if (clock.starved) ahead += 1_000;
    return now() + ahead;
  });
});

afterEach(() => {
  clock.starved = false;
  vi.restoreAllMocks();
  cleanupCells();
  cleanupMarkViews();
});

const ABOVE = Array.from(
  { length: 40 },
  (_, i) => `Paragraph ${String(i)} with **bold** text.`,
).join("\n\n");

/** Every announcement a view makes from now on. */
function hearing(view: EditorView): string[] {
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
  return said;
}

/** The next write in `view` has its reparse cut short, and only that. */
function cutNextWrite(view: EditorView): void {
  const dispatch = view.dispatch.bind(view);
  view.dispatch = ((...specs: Parameters<typeof dispatch>) => {
    view.dispatch = dispatch;
    clock.starved = true;
    try {
      dispatch(...specs);
    } finally {
      clock.starved = false;
    }
  }) as typeof view.dispatch;
}

describe.each([
  { room: false, mode: "hidden" as const },
  { room: false, mode: "show" as const },
  { room: true, mode: "hidden" as const },
  { room: true, mode: "show" as const },
])("a press in a cell after a short reparse, $mode, room $room", (options) => {
  function cellOn(row: string, crlf = false) {
    const lf = `${ABOVE}\n\n| a | b |\n| - | - |\n${row}\n\nThe end.\n`;
    const text = crlf ? lf.replaceAll("\n", "\r\n") : lf;
    const main = markView(text, {
      ...options,
      ...(crlf ? { lineSeparator: "\r\n" as const } : {}),
    });
    const cell = gridTables(parsedState(main.state))[0]?.rows[1]?.[1];
    if (!cell) throw new Error("no cell");
    const { from, to } = cell.content;
    const view = openAt(main, to, false).view;
    const said = hearing(view);
    return { main, view, said, from, to, text };
  }

  /** The tree `view` holds stops above its row, as a stall leaves it. */
  function shortened(view: EditorView, from: number) {
    expect(syntaxTree(view.state).length).toBeLessThan(from);
  }

  /** A letter written at `to`, its reparse cut short. */
  function writeZ(view: EditorView, to: number) {
    clock.starved = true;
    view.dispatch({
      changes: { from: to, insert: "Z" },
      userEvent: "input.type",
    });
    clock.starved = false;
  }

  it.each([{ crlf: false }, { crlf: true }])(
    "takes the bold off again when the first press's reparse stopped short, crlf $crlf",
    ({ crlf }) => {
      if (options.room && crlf) return;
      const { main, view, said, from, to, text } = cellOn(
        "| wikilink | [[tide-tables]] |",
        crlf,
      );
      view.dispatch({
        selection: EditorSelection.range(from, to),
        userEvent: "select.pointer",
      });
      cutNextWrite(view);
      // The press itself is timed fairly: only its write is cut short.
      keys(view, "Mod-b");
      expect(main.state.doc.lineAt(from).text).toBe(
        "| wikilink | **[[tide-tables]]** |",
      );
      shortened(view, from);
      keys(view, "Mod-b");
      expect(said).toEqual([]);
      expect(bytes(main.state)).toBe(text);
    },
  );

  it("deletes one letter a key after a short reparse", () => {
    for (const [key, at, want] of [
      ["Delete", 1, "| x | pain words |"],
      ["Backspace", 2, "| x | pain words |"],
    ] as const) {
      const { main, view, said, from, to, text } = cellOn(
        "| x | plain words |",
      );
      writeZ(view, to);
      shortened(view, from);
      setCaret(view, from + at);
      keys(view, key);
      expect(said).toEqual([]);
      expect(main.state.doc.lineAt(from).text).toBe(
        want.replace("words", "wordsZ"),
      );
      expect(bytes(main.state).length).toBe(text.length);
      cleanupCells();
      cleanupMarkViews();
    }
  });

  it("types over a selection after a short reparse", () => {
    const { main, view, said, from, to } = cellOn("| x | plain words |");
    writeZ(view, to);
    shortened(view, from);
    view.dispatch({
      selection: EditorSelection.range(from, from + 5),
      userEvent: "select.pointer",
    });
    typeText(view, "Y");
    expect(said).toEqual([]);
    expect(main.state.doc.lineAt(from).text).toBe("| x | Y wordsZ |");
  });

  it("takes a label whole after a short reparse, as it does on a whole tree", () => {
    const { main, view, said, from, to } = cellOn(
      "| x | a :status[Done]{green} |",
    );
    writeZ(view, to);
    shortened(view, from);
    setCaret(view, from + 2);
    keys(view, "Delete");
    expect(said).toEqual([]);
    // Where the marks show, the label shows as written, its letters to take.
    expect(main.state.doc.lineAt(from).text).toBe(
      options.mode === "hidden"
        ? "| x | a Z |"
        : "| x | a status[Done]{green}Z |",
    );
  });

  it("still refuses, and writes nothing, when the press's own parse runs out", () => {
    const { main, view, said, from, to } = cellOn("| x | plain words |");
    writeZ(view, to);
    shortened(view, from);
    const before = bytes(main.state);
    view.dispatch({
      selection: EditorSelection.range(from, from + 5),
      userEvent: "select.pointer",
    });
    clock.starved = true;
    keys(view, "Mod-b");
    setCaret(view, from + 1);
    keys(view, "Delete");
    clock.starved = false;
    expect(bytes(main.state)).toBe(before);
    expect(said).toEqual([CELL_NOT_READ, CELL_NOT_READ]);
  });
});
