/**
 * Byte fidelity of the cell editor: opening a cell and typing one character
 * changes exactly that one character of the file, inside the cell, in LF and
 * CRLF files, with the main view's prose layers and the hidden-mark layer
 * drawn in the cell, with marks hidden and with marks shown on the edited
 * line; undo gives back the file byte for byte. And the one teardown path: Raw and a buffer swap both close a
 * focused cell without losing what was typed.
 */

import { undo } from "@codemirror/commands";
import { ensureSyntaxTree } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import {
  Compartment,
  EditorSelection,
  EditorState,
  StateEffect,
} from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { QueryClient } from "@tanstack/react-query";
import type { MockInstance } from "vitest";
import {
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  test,
  vi,
} from "vitest";

import {
  bytes,
  cellOf,
  cleanupCells,
  openAt,
  soloMain,
  typeIn,
} from "../../test/cellKit";
import type { MarkViewOptions } from "../../test/markKit";
import {
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  setShowMarks,
  showsMarks,
  typeText,
  undoOnce,
} from "../../test/markKit";
import { parsedState } from "../../test/parse";
import { engramAdapter } from "../adapter";
import { baseExtensions, lineSeparatorFor } from "../setup";
import { canonical } from "../marks/caret";
import type { DeleteOutcome } from "../marks/deleteCheck";
import { deletionOutcome } from "../marks/deleteCheck";
import { runsOnLine } from "../marks/hiddenMarks";
import { hiddenMarkLayerOn, marksOnEditLine } from "../marks/mode";
import { previewConfig } from "../surface";
import { activeCell, cellForwarding } from "./cellEditor";
import { cellPreviewLayers } from "./draws";
import { closeCell, onCellClosed, openCellField } from "./openCell";
import { prefetchGrid } from "./loader";
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
  cleanupMarkViews();
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

test("the fixtures hold tables", () => {
  const counted = LF_FIXTURES.map(
    ([, text]) => gridTables(soloMain(text).state).length,
  );
  expect(counted.reduce((a, b) => a + b, 0)).toBeGreaterThanOrEqual(4);
});

const MODES = [
  { mode: "hidden", show: false },
  { mode: "show", show: true },
];

describe.each(
  FIXTURES.flatMap(([name, text]) =>
    MODES.map(({ mode, show }) => ({ name, text, mode, show })),
  ),
)("cell fidelity: $name, $mode mode", ({ text, show }) => {
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
      const main = soloMain(text, [
        layers(false),
        marksOnEditLine.of(show),
        cellForwarding,
      ]);
      expect(main.state.facet(cellPreviewLayers).length).toBeGreaterThan(0);
      const before = main.state;
      const cell = gridTables(before)[t]?.rows[row]?.[column];
      if (!cell) throw new Error("cell vanished");
      const at = cell.content.to;
      const editor = openAt(main, at, false);
      expect(editor.view.state.facet(hiddenMarkLayerOn)).toBe(true);
      expect(editor.view.state.facet(marksOnEditLine)).toBe(show);
      // Typed as the DOM types (C22), at the caret the cell shows: its
      // canonical place, which the format rules type at (F6).
      const head = editor.view.state.selection.main.head;
      expect(head).toBe(canonical(editor.view.state, at));
      expect(head).toBeGreaterThanOrEqual(cell.content.from);
      expect(head).toBeLessThanOrEqual(cell.content.to);
      typeText(editor.view, "Z");
      const lb = before.lineBreak;
      const len = before.doc.length;
      const want =
        before.doc.sliceString(0, head, lb) +
        "Z" +
        before.doc.sliceString(head, len, lb);
      expect(bytes(main.state)).toBe(want);
      expect(editor.view.state.doc.eq(main.state.doc)).toBe(true);
      closeCell(main, "user", "none");
      expect(activeCell(main)).toBeNull();
      expect(undo(main)).toBe(true);
      expect(bytes(main.state)).toBe(text);
    },
  );
});

/**
 * The hidden-mark fidelity properties (`marks/fidelity.test.ts`) in a cell,
 * on the page main view the screens build (`markView`), in both modes, LF and
 * CRLF solo and LF in a room:
 *
 * 1. Moving the cell's caret to every stop, selecting the cell's text and
 *    flipping the setting twice dispatch no change.
 * 2. At every run boundary and the cell's end, one typed letter adds exactly
 *    that letter, and one undo gives back the bytes.
 * 3. Backspace and Delete there never change the mark pairs of the row,
 *    except by removing a pair they emptied (in show mode a key that takes a
 *    mark character is not judged: the marks are text there).
 * 4. Mod-b on the cell's text undoes to the bytes; on a cell with no bold,
 *    Mod-b twice gives the bytes back (or the first one is refused).
 *
 * The one byte Part 1 allows outside the typed letter is R24's edge pipe
 * (`withEdges`), written when a bare edge cell is emptied; no property here
 * types into an emptied cell, and a removal is judged by its pairs.
 */
/**
 * The most of the judged Backspace and Delete presses in cells that may be
 * refused (D5), as in main (`marks/fidelity.test.ts`).
 */
const MAX_REFUSED = 0.05;

/** The fewest presses the cell runs must judge: a silent refusal of all fails. */
const CELL_FLOOR = 100;

describe("hidden-mark properties in a cell", () => {
  beforeAll(async () => {
    await prefetchGrid();
  });

  const PAIRS = new Set([
    "Emphasis",
    "StrongEmphasis",
    "Strikethrough",
    "InlineCode",
    "Link",
  ]);

  /** The pair nodes on line `n`, by name, with their visible characters. */
  function pairsOn(state: EditorState, n: number) {
    const tree = ensureSyntaxTree(state, state.doc.length, 10_000);
    if (!tree) throw new Error("the parse did not finish");
    const line = state.doc.line(n);
    const pairs: { name: string; visible: number[] }[] = [];
    tree.iterate({
      from: line.from,
      to: line.to,
      enter: (node) => {
        if (!PAIRS.has(node.name)) return;
        // Lezer reads any bracketed text as a Link; the page needs an address.
        if (node.name === "Link" && !node.node.getChild("URL")) return;
        const first = node.node.firstChild;
        const last = node.node.lastChild;
        const marks: [number, number][] = [];
        for (let c = node.node.firstChild; c; c = c.nextSibling) {
          if (/Mark$|^URL$|^LinkTitle$|^LinkLabel$/.test(c.name)) {
            marks.push([c.from, c.to]);
          }
        }
        const visible: number[] = [];
        for (let p = first?.to ?? node.from; p < (last?.from ?? node.to); p++) {
          if (!marks.some(([a, b]) => a <= p && p < b)) visible.push(p);
        }
        pairs.push({ name: node.name, visible });
      },
    });
    return pairs;
  }

  function oneLetterAdded(before: string, after: string): boolean {
    if (after.length !== before.length + 1) return false;
    let i = 0;
    while (i < before.length && before[i] === after[i]) i++;
    return after.slice(0, i) + after.slice(i + 1) === before;
  }

  /** The positions of `a` a change to `b` removed, as one stretch. */
  function removedFrom(a: string, b: string): Set<number> {
    let head = 0;
    while (head < a.length && a[head] === b[head]) head++;
    let tail = 0;
    while (
      tail < a.length - head &&
      tail < b.length - head &&
      a[a.length - 1 - tail] === b[b.length - 1 - tail]
    ) {
      tail++;
    }
    const out = new Set<number>();
    for (let p = head; p < a.length - tail; p++) out.add(p);
    return out;
  }

  const RUNS = [
    ...FIXTURES.flatMap(([name, text]) =>
      MODES.map(({ mode }) => ({ name, text, mode, room: false })),
    ),
    ...LF_FIXTURES.flatMap(([name, text]) =>
      MODES.map(({ mode }) => ({ name, text, mode, room: true })),
    ),
  ];

  type Outcome = DeleteOutcome | "exact";
  interface CellRun {
    failures: string[];
    checks: number;
    cells: number;
    deletes: Record<Outcome, number>;
  }
  const kept = new Map<string, CellRun>();

  /** One run, computed once: its test and the tally both read it. */
  function cellRunOf(run: (typeof RUNS)[number]): CellRun {
    const key = `${run.name}|${run.mode}|${String(run.room)}`;
    let result = kept.get(key);
    if (!result) {
      result = computeRun(run);
      kept.set(key, result);
      cleanupCells();
      cleanupMarkViews();
    }
    return result;
  }

  function computeRun({ text, mode, room }: (typeof RUNS)[number]): CellRun {
    const deletes: Record<Outcome, number> = {
      exact: 0,
      moved: 0,
      merged: 0,
      refused: 0,
    };
    /** The outcome the last deletion check named (a box the listener fills). */
    const named: { outcome: DeleteOutcome | null } = { outcome: null };
    const heard = new WeakSet<EditorView>();
    /** Hears the outcome a cell view's deletion check names. */
    const hear = (view: EditorView): EditorView => {
      if (!heard.has(view)) {
        heard.add(view);
        view.dispatch({
          effects: StateEffect.appendConfig.of(
            EditorView.updateListener.of((update) => {
              for (const tr of update.transactions) {
                for (const effect of tr.effects) {
                  if (effect.is(deletionOutcome)) named.outcome = effect.value;
                }
              }
            }),
          ),
        });
      }
      return view;
    };
    const options: MarkViewOptions = {
      mode: mode as "hidden" | "show",
      room,
      ...(!room && text.includes("\r\n")
        ? { lineSeparator: "\r\n" as const }
        : {}),
    };
    // The run boundaries, read where every mark is hidden.
    const probe = markView(text, { ...options, mode: "hidden", room: false });
    const main = markView(text, options);
    let changed = 0;
    main.dispatch({
      effects: StateEffect.appendConfig.of(
        EditorView.updateListener.of((update) => {
          if (update.docChanged) changed += 1;
        }),
      ),
    });
    const show = mode === "show";
    const failures: string[] = [];
    let checks = 0;
    const now = () => bytes(main.state);
    const cells = gridTables(main.state).flatMap((table) => table.rows.flat());
    const done = (): CellRun => ({
      failures,
      checks,
      cells: cells.length,
      deletes,
    });
    for (const cell of cells) {
      const { from, to } = cell.content;
      const n = main.state.doc.lineAt(from).number;
      const runs = runsOnLine(probe.state, n).filter(
        (run) => run.from >= from && run.to <= to,
      );
      const marks = runs.flatMap((run) =>
        run.marks.map((m): [number, number] => [m.from, m.to]),
      );
      const stops = [
        ...new Set([from, to, ...runs.flatMap((r) => [r.from, r.to])]),
      ].sort((a, b) => a - b);
      const label = `line ${n} cell ${cell.row}/${cell.column} ${JSON.stringify(probe.state.doc.line(n).text)}`;
      const at = (pos: number) => `${label} at ${pos - from}`;

      // Property 1.
      const before = changed;
      let view = openAt(main, to, false).view;
      for (const stop of stops) {
        setCaret(view, stop);
        setCaret(view, stop, "select");
        checks += 2;
      }
      keys(view, "Home", "End");
      view.dispatch({
        selection: EditorSelection.range(from, to),
        userEvent: "select.pointer",
      });
      const was = showsMarks(main);
      setShowMarks(main, !was);
      setShowMarks(main, was);
      checks += 2;
      if (changed !== before || now() !== text) {
        failures.push(`moving in ${label} changed the bytes`);
      }

      /** The open cell's view, mounted again only when it was closed. */
      const cellView = (pos: number) => {
        const open = activeCell(main);
        return hear(open ? open.view : openAt(main, pos, false).view);
      };
      for (const stop of stops) {
        // Property 2.
        view = cellView(stop);
        setCaret(view, stop);
        const head = view.state.selection.main.head;
        typeText(view, "Z");
        checks += 1;
        if (!oneLetterAdded(text, now())) {
          failures.push(
            `typing ${at(head)} wrote ${JSON.stringify(main.state.doc.lineAt(from).text)}`,
          );
        }
        undoOnce(main);
        if (now() !== text) {
          failures.push(`undo after typing ${at(head)}`);
          return done();
        }
        // Property 3.
        for (const key of ["Backspace", "Delete"]) {
          view = cellView(stop);
          setCaret(view, stop);
          const start = main.state;
          named.outcome = null;
          keys(view, key);
          const outcome = named.outcome;
          if (outcome === "refused") {
            deletes.refused += 1;
            if (now() !== text) {
              failures.push(`a refused ${key} ${at(stop)} wrote`);
              return done();
            }
            continue;
          }
          if (now() === text) continue;
          deletes[outcome ?? "exact"] += 1;
          const deleted = removedFrom(
            start.doc.toString(),
            main.state.doc.toString(),
          );
          const touches = [...deleted].some((p) =>
            marks.some(([a, b]) => a <= p && p < b),
          );
          if (!(show && touches)) {
            checks += 1;
            const old = pairsOn(start, n);
            const after = pairsOn(main.state, n);
            for (const name of PAIRS) {
              const x = old.filter((p) => p.name === name).length;
              const y = after.filter((p) => p.name === name).length;
              const emptied = old.filter(
                (p) =>
                  p.name === name &&
                  p.visible.length > 0 &&
                  p.visible.every((q) => deleted.has(q)),
              ).length;
              // A merge writes two pairs of one kind as one.
              const merged = outcome === "merged" ? 1 : 0;
              if (y > x || x - y > emptied + merged) {
                failures.push(
                  `${key} ${at(stop)}: ${name} ${x} -> ${y}, wrote ${JSON.stringify(main.state.doc.lineAt(from).text)}`,
                );
              }
            }
          }
          undoOnce(main);
          if (now() !== text) {
            failures.push(`undo after ${key} ${at(stop)}`);
            return done();
          }
        }
      }

      // Property 4.
      if (from === to) continue;
      const bold = () => {
        const cellView = openAt(main, to, false).view;
        cellView.dispatch({
          selection: EditorSelection.range(from, to),
          userEvent: "select.pointer",
        });
        keys(cellView, "Mod-b");
        return cellView;
      };
      const plain = !pairsOn(main.state, n).some(
        (p) => p.name === "StrongEmphasis",
      );
      const once = bold();
      checks += 1;
      if (now() !== text) {
        if (plain) {
          keys(once, "Mod-b");
          checks += 1;
          if (now() !== text) {
            failures.push(
              `Mod-b twice in ${label} gave ${JSON.stringify(main.state.doc.lineAt(from).text)}`,
            );
            undoOnce(main);
          }
        }
        if (now() !== text) undoOnce(main);
        if (now() !== text) {
          failures.push(`Mod-b then undo in ${label}`);
          return done();
        }
      }
      closeCell(main, "user", "none");
    }
    return done();
  }

  test.each(RUNS)(
    "$name, $mode mode, room $room",
    { timeout: 120_000 },
    (run) => {
      const result = cellRunOf(run);
      expect(result.cells).toBeGreaterThan(0);
      expect(result.failures).toEqual([]);
      expect(result.checks).toBeGreaterThan(result.cells);
    },
  );

  test(
    "tallies the deletes in cells, and refuses few",
    { timeout: 120_000 },
    () => {
      const all = RUNS.map((run) => cellRunOf(run).deletes);
      const sum = (key: Outcome) => all.reduce((n, each) => n + each[key], 0);
      const judged =
        sum("exact") + sum("moved") + sum("merged") + sum("refused");
      console.info(
        `cell deletes: ${sum("exact")} exact, ${sum("moved")} moved, ${sum("merged")} merged, ${sum("refused")} refused`,
      );
      expect(judged).toBeGreaterThan(CELL_FLOOR);
      expect(sum("refused") / judged).toBeLessThanOrEqual(MAX_REFUSED);
    },
  );

  describe.each(["hidden", "show"] as const)("pins, %s mode", (mode) => {
    /** One press in a cell of a one-row table: the outcome and the row. */
    function press(row: string, at: number, key: string) {
      const text = `| a | b |\n| - | - |\n${row}\n`;
      const main = markView(text, { mode });
      const cell = gridTables(main.state)[0]?.rows[1]?.[1];
      if (!cell) throw new Error("no cell");
      const pos = text.indexOf(row) + at;
      const view = openAt(main, pos, false).view;
      let outcome: DeleteOutcome | null = null;
      view.dispatch({
        effects: StateEffect.appendConfig.of(
          EditorView.updateListener.of((update) => {
            for (const tr of update.transactions) {
              for (const effect of tr.effects) {
                if (effect.is(deletionOutcome)) outcome = effect.value;
              }
            }
          }),
        ),
      });
      setCaret(view, pos);
      keys(view, key);
      return {
        outcome: outcome as DeleteOutcome | null,
        row: main.state.doc.lineAt(cell.content.from).text,
        view,
      };
    }

    test("(a) moves a mark across the space Delete leaves", () => {
      const { outcome, row, view } = press("| x | **A b** |", 8, "Delete");
      expect(outcome).toBe("moved");
      expect(row).toBe("| x |  **b** |");
      // The caret stays before the space it showed before: Z shows "Z b".
      typeText(view, "Z");
      expect(view.state.doc.lineAt(view.state.selection.main.head).text).toBe(
        "| x | Z **b** |",
      );
    });

    test("(b) merges two bold words whose space Delete takes", () => {
      // After the closing marks: in both modes the caret stands before the space.
      const at = 11;
      const { outcome, row } = press("| x | **a** **b** |", at, "Delete");
      expect(outcome).toBe("merged");
      expect(row).toBe("| x | **ab** |");
    });

    test("(c) refuses the space after a closer that follows punctuation", () => {
      const at = 12;
      const { outcome, row } = press("| x | **a.** b |", at, "Delete");
      expect(outcome).toBe("refused");
      expect(row).toBe("| x | **a.** b |");
    });
  });
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
    const head = main.state.field(openCellField)?.head;
    main.dispatch({ effects: preview.reconfigure(layers(true)) });
    await Promise.resolve();
    // In Raw the table is text: main's caret takes over at the cell's head.
    expect(main.state.selection.main.head).toBe(head);
    expect(bytes(main.state)).toBe(
      "Intro\n\n| a | b |\n| - | - |\n| 1x | 2 |\n",
    );
    expect(activeCell(main)).toBeNull();
    expect(editor.view.dom.isConnected).toBe(false);
    expect(document.activeElement).toBe(main.contentDOM);
    expect(heard).toHaveBeenCalledTimes(1);
    expect(heard).toHaveBeenCalledWith("raw");
  });

  test("setState with a new document keeps the new state's selection", async () => {
    const { main, preview, heard } = focusedCell();
    const next = "Other\n\n| x | y |\n| - | - |\n| 7 | 8 |\n";
    main.setState(
      parsedState(
        EditorState.create({
          doc: next,
          selection: { anchor: 3 },
          extensions: [
            ...baseExtensions(false),
            preview.of([layers(false), cellForwarding]),
          ],
        }),
      ),
    );
    await Promise.resolve();
    expect(heard).toHaveBeenCalledWith("swap");
    expect(bytes(main.state)).toBe(next);
    expect(main.state.selection.main.anchor).toBe(3);
    expect(main.state.selection.main.head).toBe(3);
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
