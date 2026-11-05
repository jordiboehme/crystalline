/**
 * Test helpers for the table grid's cell editor: a solo main view, a two-peer
 * room built the way the engram editor builds one, and a key press shaped
 * like a browser keydown.
 *
 * The room is LF space: the shared text holds `\n`, the buffer pins
 * `EditorState.lineSeparator.of("\n")`, Y.UndoManager is the only undo stack
 * (`history: false`) and the paste guard `normalizePastedEndings` is on.
 */

import type { Extension } from "@codemirror/state";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap, runScopeHandlers } from "@codemirror/view";
import { yCollab, yUndoManagerKeymap } from "y-codemirror.next";
import {
  Awareness,
  applyAwarenessUpdate,
  encodeAwarenessUpdate,
} from "y-protocols/awareness";
import * as Y from "yjs";

import { normalizePastedEndings } from "../collab/pastedEndings";
import { roomUndo } from "../editor/roomUndo";
import type { CellEditor } from "../editor/grid/cellEditor";
import { cellForwarding, mountCellEditor } from "../editor/grid/cellEditor";
import { openCellRange, setOpenCell } from "../editor/grid/openCell";
import type { GridCell } from "../editor/grid/structure";
import { gridTables } from "../editor/grid/structure";
import { baseExtensions, lineSeparatorFor } from "../editor/setup";
import { cellMarksExtensions } from "./cellMarksMode";
import { parsedState, parsedView } from "./parse";

const cleanups: (() => void)[] = [];

/** Runs every teardown the helpers below registered; call in `afterEach`. */
export function cleanupCells(): void {
  while (cleanups.length > 0) cleanups.pop()?.();
}

/** A main view outside any room, with its own CodeMirror history. */
export function soloMain(text: string, extra: Extension[] = []): EditorView {
  const main = new EditorView({
    state: parsedState(
      EditorState.create({
        doc: text,
        extensions: [
          ...lineSeparatorFor(text),
          ...baseExtensions(false),
          ...extra,
          ...cellMarksExtensions(),
        ],
      }),
    ),
    parent: document.body,
  });
  cleanups.push(() => main.destroy());
  return main;
}

/**
 * Two synced peers; the local main view is in the room. `hold()` keeps what
 * either peer writes from the other until `release()`, for a test of two edits
 * made before a sync.
 */
export function roomMain(text: string, extra: Extension[] = [cellForwarding]) {
  const local = new Y.Doc();
  const remote = new Y.Doc();
  let held: { to: Y.Doc; update: Uint8Array }[] | null = null;
  const send = (to: Y.Doc, update: Uint8Array) => {
    if (held) held.push({ to, update });
    else Y.applyUpdate(to, update, "remote");
  };
  const hold = () => {
    held ??= [];
  };
  const release = () => {
    const waiting = held ?? [];
    held = null;
    for (const { to, update } of waiting) Y.applyUpdate(to, update, "remote");
  };
  local.on("update", (update: Uint8Array, origin: unknown) => {
    if (origin !== "remote") send(remote, update);
  });
  remote.on("update", (update: Uint8Array, origin: unknown) => {
    if (origin !== "remote") send(local, update);
  });
  const ytext = local.getText("content");
  const rtext = remote.getText("content");
  rtext.insert(0, text); // the room's text arrives from the other peer
  const undoManager = new Y.UndoManager(ytext);
  const awareness = new Awareness(local);
  const remoteAwareness = new Awareness(remote);
  // Awareness travels both ways, each peer sending only its own client, so
  // a remote cursor reaches the local main view as it would from a server.
  const relay = (from: Awareness, to: Awareness) => {
    from.on(
      "update",
      (
        { added, updated, removed }: Record<string, number[]>,
        origin: unknown,
      ) => {
        if (origin === "remote") return;
        const changed = [
          ...(added ?? []),
          ...(updated ?? []),
          ...(removed ?? []),
        ].filter((id) => id === from.clientID);
        if (changed.length === 0) return;
        applyAwarenessUpdate(
          to,
          encodeAwarenessUpdate(from, changed),
          "remote",
        );
      },
    );
  };
  relay(awareness, remoteAwareness);
  relay(remoteAwareness, awareness);
  const main = new EditorView({
    state: parsedState(
      EditorState.create({
        doc: ytext.toJSON(),
        extensions: [
          EditorState.lineSeparator.of("\n"),
          ...baseExtensions(false, { history: false }),
          yCollab(ytext, awareness, { undoManager }),
          roomUndo.of(undoManager),
          keymap.of(yUndoManagerKeymap),
          normalizePastedEndings,
          ...extra,
          ...cellMarksExtensions(),
        ],
      }),
    ),
    parent: document.body,
  });
  cleanups.push(() => {
    main.destroy();
    awareness.destroy();
    remoteAwareness.destroy();
  });
  return {
    main,
    ytext,
    rtext,
    awareness,
    remoteAwareness,
    local,
    remote,
    undoManager,
    hold,
    release,
  };
}

/** The cell at `row` (0 is the header) and `column` of the first table. */
export function cellOf(
  main: EditorView,
  row: number,
  column: number,
): GridCell {
  const table = gridTables(parsedState(main.state))[0];
  const cell = table?.rows[row]?.[column];
  if (!cell) throw new Error(`no cell ${row}/${column}`);
  return cell;
}

/**
 * Opens the cell holding `pos` and mounts a cell editor on it.
 *
 * Main's parse is finished first. An edit on main (a typed letter, an undo)
 * parses again within a 20 millisecond `Date.now()` budget, and on a loaded
 * machine that budget runs out before the table: the open cell then has no
 * range and the mount throws "no open cell". The grid never mounts on a short
 * tree: `host` in `gridWidget.ts` mounts nothing while the cell is not parsed,
 * and a landing asks for the parse first (`parseThrough` in `openCell.ts`).
 * A test asks for it too, as `parse.ts` explains.
 */
export function openAt(
  main: EditorView,
  pos: number,
  focused = true,
  extensions: Extension[] = [],
): CellEditor {
  parsedView(main);
  main.dispatch({
    effects: setOpenCell.of({ anchor: pos, head: pos, focused }),
  });
  const host = document.createElement("div");
  document.body.appendChild(host);
  const cell = mountCellEditor(main, host, extensions);
  if (focused) cell.view.focus();
  cleanups.push(() => {
    cell.destroy();
    host.remove();
  });
  return cell;
}

/** The open cell's content text, read from the main view's structure. */
export function openText(main: EditorView): string | null {
  const cell = openCellRange(main.state);
  if (!cell) return null;
  return main.state.doc.sliceString(cell.content.from, cell.content.to);
}

/** The whole document with the state's own line breaks. */
export function bytes(state: EditorState): string {
  return state.doc.sliceString(0, state.doc.length, state.lineBreak);
}

/** Types `text` at the cell view's caret as one local transaction. */
export function typeIn(cell: CellEditor, text: string): void {
  const { view } = cell;
  const range = view.state.selection.main;
  view.dispatch({
    changes: { from: range.from, to: range.to, insert: text },
    selection: { anchor: range.from + text.length },
    userEvent: "input.type",
  });
}

/**
 * Presses a key in a view through its keymap, shaped like a real browser
 * keydown: a shifted letter is the capital letter and keyCode names the
 * physical key. `mod` is Ctrl or Cmd by platform.
 */
export function press(
  view: EditorView,
  key: string,
  opts: { mod?: boolean; shift?: boolean } = {},
): boolean {
  const mac = /Mac/.test(navigator.platform);
  const letter = key.length === 1;
  const event = new KeyboardEvent("keydown", {
    key: letter && opts.shift ? key.toUpperCase() : key,
    keyCode: letter
      ? key.toUpperCase().charCodeAt(0)
      : key === "Enter"
        ? 13
        : key === "Backspace"
          ? 8
          : key === "Delete"
            ? 46
            : 0,
    shiftKey: opts.shift ?? false,
    metaKey: (opts.mod ?? false) && mac,
    ctrlKey: (opts.mod ?? false) && !mac,
    bubbles: true,
    cancelable: true,
  });
  return runScopeHandlers(view, event, "editor");
}
