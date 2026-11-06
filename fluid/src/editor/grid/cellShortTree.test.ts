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
import { CANNOT_LINK, elementEditCell } from "../elementEdit";
import { CELL_NOT_READ, cellParseBudget } from "./cellEditor";
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

/**
 * The pins of D13 and D17 on a fair clock, with no stub at work: a cell that
 * opened before its row was read, or whose reparse stopped short. A press over
 * marks is written as on a whole tree, or not at all, and never leaves a lone
 * mark. `cellMarks.test.ts` pins the refusal past the press's own budget.
 */
describe.each([
  { room: false, mode: "hidden" as const, crlf: false },
  { room: false, mode: "hidden" as const, crlf: true },
  { room: false, mode: "show" as const, crlf: false },
  { room: false, mode: "show" as const, crlf: true },
  { room: true, mode: "hidden" as const, crlf: false },
  { room: true, mode: "show" as const, crlf: false },
])(
  "over marks in a cell, $mode, room $room, crlf $crlf",
  ({ crlf, ...options }) => {
    const above = Array.from(
      { length: 4000 },
      (_, i) => `Paragraph ${String(i)} with **bold** text.`,
    ).join("\n\n");

    /** The row's text after `write`, in a cell opened whole or at budget 0. */
    function pressed(
      row: string,
      word: string,
      length: number,
      unread: boolean,
      write: (view: EditorView, from: number, to: number) => void,
    ) {
      const lf = `${above}\n\n| a | b |\n| - | - |\n${row}\n`;
      const text = crlf ? lf.replaceAll("\n", "\r\n") : lf;
      const main = markView(text, {
        ...options,
        ...(crlf ? { lineSeparator: "\r\n" as const } : {}),
      });
      if (unread) {
        main.dispatch({
          effects: StateEffect.appendConfig.of(cellParseBudget.of(0)),
        });
      }
      const cell = gridTables(parsedState(main.state))[0]?.rows[1]?.[1];
      if (!cell) throw new Error("no cell");
      const lineFrom = main.state.doc.lineAt(cell.content.from).from;
      const from = lineFrom + row.indexOf(word);
      const view = openAt(main, cell.content.to, false).view;
      if (unread) {
        expect(syntaxTree(view.state).length).toBeLessThan(cell.content.from);
      }
      write(view, from, from + length);
      const result = main.state.doc.lineAt(cell.content.from).text;
      cleanupCells();
      cleanupMarkViews();
      return result;
    }

    const select = (view: EditorView, from: number, to: number) => {
      view.dispatch({
        selection: EditorSelection.range(from, to),
        userEvent: "select.pointer",
      });
    };
    const cut = (view: EditorView, from: number, to: number) => {
      select(view, from, to);
      const event = new Event("cut", { bubbles: true, cancelable: true });
      Object.defineProperty(event, "clipboardData", {
        value: { setData: () => {}, getData: () => "", clearData: () => {} },
      });
      view.focus();
      view.contentDOM.dispatchEvent(event);
    };
    const type = (view: EditorView, from: number, to: number) => {
      select(view, from, to);
      typeText(view, "X");
    };
    const paste = (view: EditorView, from: number, to: number) => {
      select(view, from, to);
      view.dispatch({
        changes: { from, to, insert: "Y" },
        userEvent: "input.paste",
      });
    };

    it.each([
      ["the old D17 row", "| label | **a** b |", "a** b", 5],
      ["two pairs", "| x | a **bold** b `code` e |", "a **b", 5],
      [
        "a bold end",
        "| x | a **bold** :status[Done]{green} c |",
        "ld** :st",
        8,
      ],
      ["a link", "| x | [l](u) `c` |", "(u) `", 5],
    ])(
      "typed or pasted over %s reads as on a whole tree",
      (_n, row, word, n) => {
        for (const write of [type, paste]) {
          const whole = pressed(row, word, n, false, write);
          // Not at all is fair too: the press's own 150 ms can run out on a
          // machine under load, and then it refuses (D17).
          expect([whole, row]).toContain(pressed(row, word, n, true, write));
        }
      },
    );

    it("cut with a real event reads as on a whole tree, or not at all", () => {
      const row = "| label | **a** b |";
      const whole = pressed(row, "a** b", 5, false, cut);
      expect(whole).not.toBe(row);
      expect([whole, row]).toContain(pressed(row, "a** b", 5, true, cut));
    });

    it("never leaves the lone mark the old pin named, where marks are hidden", () => {
      const wanted =
        options.mode === "hidden" ? "| label | **X** |" : "| label | **X |";
      const row = pressed("| label | **a** b |", "a** b", 5, true, type);
      expect(["| label | **a** b |", wanted]).toContain(row);
    });
  },
);

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
      onElementEdit: () => {},
    });
    const cell = gridTables(parsedState(main.state))[0]?.rows[1]?.[1];
    if (!cell) throw new Error("no cell");
    const { from, to } = cell.content;
    const view = openAt(main, to, false, [elementEditCell(main)]).view;
    const said = hearing(view);
    return { main, view, said, from, to, text };
  }

  /** The tree `view` holds stops above its row, as a stall leaves it. */
  function shortened(view: EditorView, from: number) {
    expect(syntaxTree(view.state).length).toBeLessThan(from);
  }

  /** A letter written at `at` and taken back, the reparses cut short. */
  function writeAt(view: EditorView, at: number) {
    clock.starved = true;
    view.dispatch({ changes: { from: at, insert: "Z" } });
    view.dispatch({ changes: { from: at, to: at + 1 } });
    clock.starved = false;
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

  it("types over marks after a short reparse as on a whole tree", () => {
    const row = "| x | a **bold** b `code` e |";
    const press = (short: boolean) => {
      const { main, view, said, from } = cellOn(row);
      const at = from + "a ".length - 2;
      const to = from + "a **b".length;
      if (short) {
        writeAt(view, from + row.indexOf("e |") - 6);
        shortened(view, from);
      }
      view.dispatch({
        selection: EditorSelection.range(at, to),
        userEvent: "select.pointer",
      });
      typeText(view, "Y");
      const line = main.state.doc.lineAt(from).text;
      const words = [...said];
      cleanupCells();
      cleanupMarkViews();
      return { line, words };
    };
    expect(press(true)).toEqual(press(false));
  });

  it.each([{ crlf: false }, { crlf: true }])(
    "refuses a new link across marks after a short reparse, crlf $crlf",
    ({ crlf }) => {
      if (options.room && crlf) return;
      const { main, view, said, from, to } = cellOn(
        "| x | some **bold word** here |",
        crlf,
      );
      const line = main.state.doc.lineAt(from);
      const start = line.text.indexOf("word") + line.from;
      writeZ(view, to);
      shortened(view, from);
      view.dispatch({
        selection: EditorSelection.range(start, start + "word** h".length),
        userEvent: "select.pointer",
      });
      const before = bytes(main.state);
      keys(view, "Mod-k");
      expect(bytes(main.state)).toBe(before);
      expect(said).toEqual([CANNOT_LINK]);
    },
  );

  it("still refuses, and writes nothing, when the press's own parse runs out", () => {
    // A row the first parse has not reached: a cut reparse is too near its row
    // for a starved press to fall short, as the rest of the tree is reused.
    const above = Array.from(
      { length: 4000 },
      (_, i) => `Paragraph ${String(i)} with **bold** text.`,
    ).join("\n\n");
    const text = `${above}\n\n| a | b |\n| - | - |\n| x | plain words |\n`;
    const main = markView(text, options);
    main.dispatch({
      effects: StateEffect.appendConfig.of(cellParseBudget.of(0)),
    });
    const cell = gridTables(parsedState(main.state))[0]?.rows[1]?.[1];
    if (!cell) throw new Error("no cell");
    const { from, to } = cell.content;
    const view = openAt(main, to, false).view;
    const said = hearing(view);
    shortened(view, from);
    view.dispatch({
      selection: EditorSelection.range(from, from + 5),
      userEvent: "select.pointer",
    });
    clock.starved = true;
    keys(view, "Mod-b");
    setCaret(view, from + 1);
    keys(view, "Delete");
    clock.starved = false;
    expect(bytes(main.state)).toBe(text);
    expect(said).toEqual([CELL_NOT_READ, CELL_NOT_READ]);
  });
});
