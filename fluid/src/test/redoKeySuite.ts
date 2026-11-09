/**
 * Mod+Shift+Z redoes in every editor that binds undo and redo, whatever case
 * `event.key` has (`undoStack.ts`). A wrapper test file names the platform
 * before anything loads CodeMirror (which reads `navigator.platform` once),
 * then calls `redoKeySuite`.
 *
 * Every press is a keydown with key "z" and shiftKey, the shape Caps Lock
 * gives on Linux and Windows. The kits' own `keys` and `press` capitalise a
 * shifted letter, so they would hide the bug; the presses here are built by
 * hand and dispatched on the content DOM, the path a browser takes.
 */

import { isolateHistory } from "@codemirror/commands";
import type { EditorView } from "@codemirror/view";
import { runScopeHandlers } from "@codemirror/view";
import { afterEach, describe, expect, test } from "vitest";

import { cellForwarding } from "../editor/grid/cellEditor";
import { roomUndo } from "../editor/roomUndo";
import {
  bytes,
  cellOf,
  cleanupCells,
  openAt,
  roomMain,
  soloMain,
} from "./cellKit";
import { cleanupMarkViews, markView } from "./markKit";

const DOC = "| a | b |\n| - | - |\n| 1 | 2 |\n";

/** The platform's Mod: Cmd on a Mac, Ctrl elsewhere. */
function modOf(): KeyboardEventInit {
  return /Mac/.test(navigator.platform) ? { metaKey: true } : { ctrlKey: true };
}

/** A keydown for the Z key on `view`, with Mod held, through the DOM. */
function pressZ(view: EditorView, shift: boolean): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key: "z",
    code: "KeyZ",
    keyCode: 90,
    shiftKey: shift,
    ...modOf(),
    bubbles: true,
    cancelable: true,
  });
  view.contentDOM.dispatchEvent(event);
  return event;
}

/**
 * Inserts `text` at `at` as its own undo step: isolated in CodeMirror's
 * history, and with the room manager's capture window closed in a room.
 */
function step(view: EditorView, at: number, text: string): void {
  view.state.facet(roomUndo)?.stopCapturing();
  view.dispatch({
    changes: { from: at, insert: text },
    selection: { anchor: at + text.length },
    userEvent: "input.type",
    annotations: isolateHistory.of("full"),
  });
  view.state.facet(roomUndo)?.stopCapturing();
}

/**
 * Two steps on `edit` ("A", then "B" after it), one Mod+Z, then Mod+Shift+Z
 * with key "z": the Mod+Z takes "B" back, the shifted press puts it back.
 * `main` holds the document; `edit` is where the keys go.
 */
function undoThenRedo(main: EditorView, edit: EditorView, at: number): void {
  const before = bytes(main.state);
  const withA = before.slice(0, at) + "A" + before.slice(at);
  const withAB = before.slice(0, at) + "AB" + before.slice(at);
  step(edit, at, "A");
  step(edit, at + 1, "B");
  expect(bytes(main.state)).toBe(withAB);

  const undo = pressZ(edit, false);
  expect(undo.defaultPrevented).toBe(true);
  expect(bytes(main.state)).toBe(withA);

  const redo = pressZ(edit, true);
  expect(redo.defaultPrevented).toBe(true);
  expect(bytes(main.state)).toBe(withAB);
}

/** With nothing to redo, the shifted press changes nothing (it never undoes). */
function nothingToRedo(main: EditorView, edit: EditorView, at: number): void {
  const before = bytes(main.state);
  const withAB = before.slice(0, at) + "AB" + before.slice(at);
  step(edit, at, "A");
  step(edit, at + 1, "B");
  const redo = pressZ(edit, true);
  expect(redo.defaultPrevented).toBe(true);
  expect(bytes(main.state)).toBe(withAB);
}

/** The end of the first data cell's content, in main's coordinates. */
function cellEnd(main: EditorView): number {
  return cellOf(main, 1, 0).content.to;
}

export function redoKeySuite(): void {
  afterEach(() => {
    cleanupMarkViews();
    cleanupCells();
  });

  describe("Mod+Shift+Z with key z redoes", () => {
    test("solo, built as a screen builds it", () => {
      const view = markView("one\n");
      undoThenRedo(view, view, 0);
    });

    test("in a room, built as a screen builds it", () => {
      const view = markView("one\n", { room: true });
      undoThenRedo(view, view, 0);
    });

    test("solo main beside a table", () => {
      const main = soloMain(DOC, [cellForwarding]);
      undoThenRedo(main, main, 0);
    });

    test("in a room, main beside a table, and the peer sees it", () => {
      const { main, rtext } = roomMain(DOC);
      undoThenRedo(main, main, 0);
      expect(rtext.toJSON()).toBe(bytes(main.state));
    });

    test("in a table cell, solo", () => {
      const main = soloMain(DOC, [cellForwarding]);
      const cell = openAt(main, cellEnd(main));
      undoThenRedo(main, cell.view, cellEnd(main));
    });

    test("in a table cell, in a room", () => {
      const { main, rtext } = roomMain(DOC);
      const cell = openAt(main, cellEnd(main));
      undoThenRedo(main, cell.view, cellEnd(main));
      expect(rtext.toJSON()).toBe(bytes(main.state));
    });

    test("through the keymap alone, the way the kits press keys", () => {
      const view = markView("one\n", { room: true });
      step(view, 0, "A");
      step(view, 1, "B");
      pressZ(view, false);
      const event = new KeyboardEvent("keydown", {
        key: "z",
        keyCode: 90,
        shiftKey: true,
        ...modOf(),
      });
      expect(runScopeHandlers(view, event, "editor")).toBe(true);
      expect(view.state.doc.toString()).toBe("ABone\n");
    });
  });

  describe("Mod+Shift+Z with nothing to redo never undoes", () => {
    test("solo", () => {
      const view = markView("one\n");
      nothingToRedo(view, view, 0);
    });

    test("in a room", () => {
      const view = markView("one\n", { room: true });
      nothingToRedo(view, view, 0);
    });

    test("in a table cell, solo", () => {
      const main = soloMain(DOC, [cellForwarding]);
      const cell = openAt(main, cellEnd(main));
      nothingToRedo(main, cell.view, cellEnd(main));
    });

    test("in a table cell, in a room", () => {
      const { main } = roomMain(DOC);
      const cell = openAt(main, cellEnd(main));
      nothingToRedo(main, cell.view, cellEnd(main));
    });
  });
}
