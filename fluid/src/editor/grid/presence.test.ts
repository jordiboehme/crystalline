/**
 * Presence inside the grid: a peer's caret in a static cell draws as a
 * marker in that cell's element, straight from the awareness, and the local
 * caret in an open cell is published to the room while the cell has the
 * focus - and taken back when it leaves.
 */

import { syntaxTree } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { Awareness } from "y-protocols/awareness";
import * as Y from "yjs";

import { cleanupCells, press, roomMain, typeIn } from "../../test/cellKit";
import { parsedState } from "../../test/parse";
import { PIPELESS_FIXTURES } from "../../test/pipelessTables";
import { fencePreviews } from "../fencePreviews";
import { activeCell } from "./cellEditor";
import { gridExtension } from "./index";
import { cellAt, gridTableAt, gridTables } from "./structure";

const DOC = "| a | b |\n| - | - |\n| 1 | 2 |\n\nafter\n";

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

function cellEl(main: EditorView, row: number, column: number): HTMLElement {
  const el = main.dom.querySelector<HTMLElement>(
    `table[role="grid"] [aria-rowindex="${String(row + 1)}"][aria-colindex="${String(column + 1)}"]`,
  );
  if (!el) throw new Error(`no cell ${String(row)}/${String(column)}`);
  return el;
}

function mouseDown(el: HTMLElement): void {
  el.dispatchEvent(
    new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }),
  );
}

function hosted(main: EditorView) {
  const cell = activeCell(main);
  if (!cell) throw new Error("no cell editor mounted");
  return cell;
}

/** Puts the remote peer's caret at `pos` of its own text. */
function peerCursor(
  remoteAwareness: Awareness,
  rtext: Y.Text,
  pos: number | null,
): void {
  remoteAwareness.setLocalStateField(
    "cursor",
    pos === null
      ? null
      : {
          anchor: Y.createRelativePositionFromTypeIndex(rtext, pos),
          head: Y.createRelativePositionFromTypeIndex(rtext, pos),
        },
  );
}

/** Where the remote peer sees the local caret's head, or null. */
function seenHead(
  remoteAwareness: Awareness,
  local: Y.Doc,
  remote: Y.Doc,
): number | null {
  const state = remoteAwareness.getStates().get(local.clientID) as
    { cursor?: { head?: unknown } | null } | undefined;
  const head = state?.cursor?.head;
  if (head == null) return null;
  const abs = Y.createAbsolutePositionFromRelativePosition(
    Y.createRelativePositionFromJSON(head),
    remote,
  );
  return abs?.index ?? null;
}

/** "row/column" of the first table's cell holding `pos`, or null. */
function cellName(main: EditorView, pos: number | null): string | null {
  if (pos === null) return null;
  // The whole parse: after a cell edit, main's own tree can stop short of
  // the table on a loaded machine (the grid no longer parses it untimed).
  const table = gridTableAt(
    main.state,
    pos,
    syntaxTree(parsedState(main.state)),
  );
  const cell = table ? cellAt(table, pos) : null;
  return cell ? `${String(cell.row)}/${String(cell.column)}` : null;
}

function markers(el: HTMLElement): HTMLElement[] {
  return [...el.querySelectorAll<HTMLElement>(".cm-grid-presence")];
}

const flush = () => new Promise<void>((resolve) => queueMicrotask(resolve));

describe("peers' carets in static cells", () => {
  test("a remote cursor in cell 2 draws a marker there from the awareness alone", () => {
    const { main, rtext, remoteAwareness } = roomMain(DOC, layers());
    remoteAwareness.setLocalStateField("user", {
      name: "Grace",
      color: "#ff0000",
    });
    // No main view dispatch at all: y-codemirror's own redraw is swallowed.
    const dispatch = vi.spyOn(main, "dispatch").mockImplementation(() => {});
    peerCursor(remoteAwareness, rtext, DOC.indexOf("2") + 1);
    expect(dispatch).toHaveBeenCalled(); // the swallowed y-codemirror redraw
    const [marker, ...more] = markers(cellEl(main, 1, 1));
    expect(more).toEqual([]);
    expect(marker?.textContent).toContain("Grace");
    expect(marker?.style.borderColor).toBe("rgb(255, 0, 0)");
    // Copying the cell's text never takes the peer's name with it.
    expect(marker?.style.userSelect).toBe("none");
    expect(markers(cellEl(main, 1, 0))).toEqual([]);
    dispatch.mockRestore();
  });

  test("a moving remote cursor touches only the cells it left and entered", () => {
    const { main, rtext, remoteAwareness } = roomMain(DOC, layers());
    peerCursor(remoteAwareness, rtext, DOC.indexOf("1"));
    expect(markers(cellEl(main, 1, 0))).toHaveLength(1);
    const header = cellEl(main, 0, 0);
    const seen: MutationRecord[] = [];
    const observer = new MutationObserver((records) => seen.push(...records));
    observer.observe(header, { childList: true, subtree: true });
    peerCursor(remoteAwareness, rtext, DOC.indexOf("2"));
    observer.disconnect();
    expect(seen.concat(observer.takeRecords())).toEqual([]);
    expect(markers(cellEl(main, 1, 0))).toEqual([]);
    expect(markers(cellEl(main, 1, 1))).toHaveLength(1);
  });

  test("the marker survives a redraw of its cell's text", () => {
    const { main, rtext, remoteAwareness } = roomMain(DOC, layers());
    peerCursor(remoteAwareness, rtext, DOC.indexOf("2") + 1);
    rtext.insert(DOC.indexOf("2") + 1, "x");
    const el = cellEl(main, 1, 1);
    expect(el.textContent).toContain("2x");
    expect(markers(el)).toHaveLength(1);
  });

  test("a main update that moves nothing in the grid does not look for cells", () => {
    const { main, rtext, remoteAwareness } = roomMain(DOC, layers());
    peerCursor(remoteAwareness, rtext, DOC.indexOf("2"));
    expect(markers(cellEl(main, 1, 1))).toHaveLength(1);
    const lookups = vi.spyOn(main.contentDOM, "querySelectorAll");
    main.dispatch({ selection: { anchor: DOC.indexOf("after") } });
    main.dispatch({ selection: { anchor: DOC.indexOf("after") + 2 } });
    const gridLookups = lookups.mock.calls.filter(
      ([sel]) => sel === ".cm-grid",
    );
    lookups.mockRestore();
    expect(gridLookups).toEqual([]);
    expect(markers(cellEl(main, 1, 1))).toHaveLength(1);
  });

  test("a remote cursor outside every table draws no marker", () => {
    const { main, rtext, remoteAwareness } = roomMain(DOC, layers());
    peerCursor(remoteAwareness, rtext, DOC.indexOf("after"));
    expect(main.dom.querySelectorAll(".cm-grid-presence")).toHaveLength(0);
  });

  test("a peer that leaves takes its marker with it", () => {
    const { main, rtext, remoteAwareness } = roomMain(DOC, layers());
    peerCursor(remoteAwareness, rtext, DOC.indexOf("2"));
    expect(markers(cellEl(main, 1, 1))).toHaveLength(1);
    peerCursor(remoteAwareness, rtext, null);
    expect(markers(cellEl(main, 1, 1))).toEqual([]);
  });

  test("in the cell hosting the editor the caret is drawn by the cell view", async () => {
    const { main, rtext, remoteAwareness } = roomMain(DOC, layers());
    peerCursor(remoteAwareness, rtext, DOC.indexOf("2"));
    mouseDown(cellEl(main, 1, 1));
    const cell = hosted(main);
    await flush();
    const el = cellEl(main, 1, 1);
    const drawn = markers(el);
    expect(drawn).toHaveLength(1);
    expect(cell.view.contentDOM.contains(drawn[0] ?? null)).toBe(true);
    // Once the cell closes, the static marker comes back.
    press(cell.view, "Escape");
    await flush();
    const back = markers(cellEl(main, 1, 1));
    expect(back).toHaveLength(1);
    expect(cell.view.dom.isConnected).toBe(false);
  });
});

// R23: carets in a table without outer pipes land in the right cell, at
// both ends of each cell's text, the bare line edges included.
describe.each(PIPELESS_FIXTURES)("presence without outer pipes: $name", (f) => {
  test("a peer's caret at either end of each cell is drawn in that cell", () => {
    const { main, rtext, remoteAwareness } = roomMain(f.text, layers());
    const table = gridTables(main.state)[0];
    if (!table) throw new Error("no table");
    for (const row of table.rows) {
      for (const cell of row) {
        for (const pos of [cell.content.from, cell.content.to]) {
          peerCursor(remoteAwareness, rtext, pos);
          const holding = table.rows.flatMap((r, ri) =>
            r
              .map((_, ci) => [ri, ci])
              .filter(
                ([ri, ci]) =>
                  markers(cellEl(main, ri ?? 0, ci ?? 0)).length > 0,
              ),
          );
          expect(
            holding,
            `${String(cell.row)}/${String(cell.column)} at ${String(pos)}`,
          ).toEqual([[cell.row, cell.column]]);
        }
      }
    }
    peerCursor(remoteAwareness, rtext, null);
  });

  test("the local caret in an open edge cell is published in that cell", async () => {
    const { main, remoteAwareness, local, remote } = roomMain(f.text, layers());
    for (const [row, column] of [
      [2, 0],
      [2, 1],
    ]) {
      mouseDown(cellEl(main, row ?? 0, column ?? 0));
      const cell = hosted(main);
      await flush();
      expect(cellName(main, seenHead(remoteAwareness, local, remote))).toBe(
        `${String(row)}/${String(column)}`,
      );
      typeIn(cell, "x");
      await flush();
      expect(cellName(main, seenHead(remoteAwareness, local, remote))).toBe(
        `${String(row)}/${String(column)}`,
      );
    }
  });
});

describe("the local caret in an open cell", () => {
  test("the focused cell publishes its caret, and typing moves it", async () => {
    const { main, remoteAwareness, local, remote } = roomMain(DOC, layers());
    mouseDown(cellEl(main, 1, 1));
    const cell = hosted(main);
    expect(cellName(main, seenHead(remoteAwareness, local, remote))).toBe(
      "1/1",
    );
    typeIn(cell, "xy");
    await flush();
    expect(seenHead(remoteAwareness, local, remote)).toBe(
      cell.view.state.selection.main.head,
    );
  });

  test("after Tab out of a cell, the peer sees no cursor at the old cell", async () => {
    const { main, remoteAwareness, local, remote } = roomMain(DOC, layers());
    mouseDown(cellEl(main, 1, 0));
    expect(cellName(main, seenHead(remoteAwareness, local, remote))).toBe(
      "1/0",
    );
    press(hosted(main).view, "Tab");
    await flush();
    expect(cellName(main, seenHead(remoteAwareness, local, remote))).toBe(
      "1/1",
    );
  });

  test("blur takes the published caret back", async () => {
    const { main, remoteAwareness, local, remote } = roomMain(DOC, layers());
    mouseDown(cellEl(main, 1, 1));
    expect(seenHead(remoteAwareness, local, remote)).not.toBeNull();
    const input = document.createElement("input");
    document.body.appendChild(input);
    input.focus();
    await flush();
    expect(seenHead(remoteAwareness, local, remote)).toBeNull();
    // The cell is still open, only unfocused.
    expect(activeCell(main)).not.toBeNull();
    input.remove();
  });

  test("Escape to grid navigation leaves no caret for the peer", async () => {
    const { main, remoteAwareness, local, remote } = roomMain(DOC, layers());
    mouseDown(cellEl(main, 1, 1));
    press(hosted(main).view, "Escape");
    await flush();
    expect(seenHead(remoteAwareness, local, remote)).toBeNull();
  });
});
