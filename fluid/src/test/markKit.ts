/**
 * Test helpers for the hidden-mark editing: a view built through
 * `surfaceExtensions` (the way both screens build theirs), a caret, keys shaped
 * like a browser keydown, typing that goes through the input handlers as the
 * DOM would, and undo that matches the surface (CodeMirror solo, the room's
 * `Y.UndoManager` in a room).
 */

import { undo, undoDepth } from "@codemirror/commands";
import { Compartment, EditorSelection, EditorState } from "@codemirror/state";
import { EditorView, keymap, runScopeHandlers } from "@codemirror/view";
import { QueryClient } from "@tanstack/react-query";
import { yCollab, yUndoManagerKeymap } from "y-codemirror.next";
import { Awareness } from "y-protocols/awareness";
import * as Y from "yjs";

import type { DocumentAdapter } from "../editor/adapter";
import { engramAdapter, reflectionAdapter } from "../editor/adapter";
import type { ElementEdit } from "../editor/elementEdit";
import { marksOnEditLine } from "../editor/marks/mode";
import { roomUndo } from "../editor/roomUndo";
import { previewConfig, surfaceExtensions } from "../editor/surface";
import { parsedView } from "./parse";

export { bytes } from "./cellKit";

export interface MarkViewOptions {
  /** "hidden" keeps marks hidden on the caret line too; "show" reveals them there. */
  mode?: "hidden" | "show";
  /** Build the view as a room peer, with the room's undo stack. */
  room?: boolean;
  adapter?: "engram" | "reflection";
  /** The source view: no preview layers at all. */
  raw?: boolean;
  /** Told the formats the format buttons show as pressed. */
  onFormatState?: (formats: ReadonlySet<string>) => void;
  /** Told the element edit a click, Mod-k or Shift+F10 asked for. */
  onElementEdit?: (edit: ElementEdit | null, view: EditorView) => void;
  /** The room's shared text, for a second peer (`markPeers`). */
  ytext?: Y.Text;
  /**
   * The buffer's line separator. Left out, it is LF, so a CRLF file keeps its
   * `\r` in each line; a solo screen picks it from the file (`separatorOf`).
   */
  lineSeparator?: "\n" | "\r\n";
  /**
   * Whether the view's parse is finished before it is handed back (the
   * default). False keeps whatever the first parse of the state reached,
   * for a test of what a press does on a short tree.
   */
  parsed?: boolean;
}

const views = new Set<EditorView>();
const undoManagers = new WeakMap<EditorView, Y.UndoManager>();
const roomTexts = new WeakMap<EditorView, Y.Text>();
const markCompartments = new WeakMap<EditorView, Compartment>();
const previews = new WeakMap<
  EditorView,
  { compartment: Compartment; adapter: DocumentAdapter }
>();

/** Destroys every view `markView` built; call in `afterEach`. */
export function cleanupMarkViews(): void {
  for (const view of views) {
    view.destroy();
  }
  views.clear();
}

/** A view on `text` built through `surfaceExtensions`, with a finished parse. */
export function markView(
  text: string,
  options: MarkViewOptions = {},
): EditorView {
  const adapter =
    options.adapter === "reflection"
      ? reflectionAdapter()
      : engramAdapter({
          domain: "eng",
          client: new QueryClient(),
          vocab: () => null,
        });
  let screen: ReturnType<typeof yCollab>[] = [];
  let undoManager: Y.UndoManager | null = null;
  const marks = new Compartment();
  const preview = new Compartment();
  let doc = text;
  let roomText: Y.Text | null = null;
  if (options.room || options.ytext) {
    let ytext = options.ytext;
    if (!ytext) {
      ytext = new Y.Doc().getText("content");
      ytext.insert(0, text);
    }
    const ydoc = ytext.doc;
    if (!ydoc) {
      throw new Error("a room text belongs to a Y.Doc");
    }
    roomText = ytext;
    undoManager = new Y.UndoManager(ytext);
    doc = ytext.toJSON();
    screen = [
      yCollab(ytext, new Awareness(ydoc), { undoManager }),
      roomUndo.of(undoManager),
      keymap.of(yUndoManagerKeymap),
    ];
  }
  const view = new EditorView({
    state: EditorState.create({
      doc,
      extensions: [
        EditorState.lineSeparator.of(options.lineSeparator ?? "\n"),
        ...surfaceExtensions({
          adapter,
          dark: false,
          history: !options.room,
          screen,
          preview,
          raw: options.raw ?? false,
          images: [],
          uploads: [],
          onTableContext: () => {},
          onImageContext: () => {},
          onElementContext: () => {},
          onFloatingContext: () => {},
          ...(options.onFormatState
            ? { onFormatState: options.onFormatState }
            : {}),
          ...(options.onElementEdit
            ? { onElementEdit: options.onElementEdit }
            : {}),
          marks,
          showMarks: options.mode === "show",
        }),
      ],
    }),
    parent: document.body,
  });
  views.add(view);
  markCompartments.set(view, marks);
  previews.set(view, { compartment: preview, adapter });
  if (undoManager) {
    undoManagers.set(view, undoManager);
  }
  if (roomText) {
    roomTexts.set(view, roomText);
  }
  return options.parsed === false ? view : parsedView(view);
}

/**
 * A peer's insert into the room `view` is in, arriving through the binding
 * the way a remote change does (no selection, the binding's own annotation).
 */
export function remoteInsert(view: EditorView, at: number, text: string): void {
  const ytext = roomTexts.get(view);
  if (!ytext) {
    throw new Error("remoteInsert takes a room view built by markView");
  }
  ytext.doc?.transact(() => {
    ytext.insert(at, text);
  }, "peer");
}

/**
 * Two peers of one room on `text`, each a view built by `markView`. What one
 * peer writes reaches the other only on `sync()`, so a test can let both
 * type before either sees the other.
 */
export function markPeers(
  text: string,
  options: Omit<MarkViewOptions, "room" | "ytext"> = {},
): { local: EditorView; peer: EditorView; sync: () => void } {
  const localDoc = new Y.Doc();
  const peerDoc = new Y.Doc();
  localDoc.getText("content").insert(0, text);
  Y.applyUpdate(peerDoc, Y.encodeStateAsUpdate(localDoc));
  const queue: { to: Y.Doc; update: Uint8Array }[] = [];
  localDoc.on("update", (update: Uint8Array, origin: unknown) => {
    if (origin !== "sync") queue.push({ to: peerDoc, update });
  });
  peerDoc.on("update", (update: Uint8Array, origin: unknown) => {
    if (origin !== "sync") queue.push({ to: localDoc, update });
  });
  const local = markView(text, {
    ...options,
    ytext: localDoc.getText("content"),
  });
  const peer = markView(text, {
    ...options,
    ytext: peerDoc.getText("content"),
  });
  return {
    local,
    peer,
    sync: () => {
      while (queue.length > 0) {
        const next = queue.shift();
        if (next) Y.applyUpdate(next.to, next.update, "sync");
      }
    },
  };
}

/** Whether the setting says to show marks on the edited line. */
export function showsMarks(view: EditorView): boolean {
  return view.state.facet(marksOnEditLine);
}

/**
 * Flips the setting the way a screen does: a reconfigure of the marks
 * compartment, with no selection in the transaction.
 */
export function setShowMarks(view: EditorView, show: boolean): void {
  const marks = markCompartments.get(view);
  if (!marks) {
    throw new Error("setShowMarks takes a view built by markView");
  }
  view.dispatch({ effects: marks.reconfigure(marksOnEditLine.of(show)) });
}

/** Flips Raw the way the engram editor does: a reconfigure of the preview. */
export function setRaw(view: EditorView, raw: boolean): void {
  const preview = previews.get(view);
  if (!preview) {
    throw new Error("setRaw takes a view built by markView");
  }
  view.dispatch({
    effects: preview.compartment.reconfigure(
      previewConfig(raw, false, [], preview.adapter),
    ),
  });
}

/** The caret (the main selection's head). */
export function caret(view: EditorView): number {
  return view.state.selection.main.head;
}

/** Puts an empty caret at `pos` the way a click does, by default. */
export function setCaret(
  view: EditorView,
  pos: number,
  userEvent = "select.pointer",
): void {
  view.dispatch({ selection: EditorSelection.cursor(pos), userEvent });
}

const MODIFIERS = new Map([
  ["Mod", "mod"],
  ["Shift", "shift"],
  ["Alt", "alt"],
  ["Ctrl", "ctrl"],
]);

/**
 * Presses each named key in order (`"ArrowRight"`, `"Shift-ArrowLeft"`,
 * `"Mod-b"`) as a browser-shaped keydown through the view's keymaps. A shifted
 * letter is the capital letter and keyCode names the physical key. Returns
 * whether the last key was handled.
 */
export function keys(view: EditorView, ...names: string[]): boolean {
  const mac = /Mac/.test(navigator.platform);
  let handled = false;
  for (const name of names) {
    const parts = name.split("-");
    // A trailing "-" is the minus key itself: "Mod--" splits to [..., "", ""].
    let key = parts.pop() ?? "";
    if (key === "" && parts.length > 0) {
      parts.pop();
      key = "-";
    }
    const held = new Set(parts.map((part) => MODIFIERS.get(part) ?? part));
    const letter = key.length === 1;
    const event = new KeyboardEvent("keydown", {
      key: letter && held.has("shift") ? key.toUpperCase() : key,
      keyCode: letter
        ? key.toUpperCase().charCodeAt(0)
        : (KEY_CODES.get(key) ?? 0),
      shiftKey: held.has("shift"),
      altKey: held.has("alt"),
      metaKey: held.has("mod") && mac,
      ctrlKey: (held.has("mod") && !mac) || held.has("ctrl"),
      bubbles: true,
      cancelable: true,
    });
    handled = runScopeHandlers(view, event, "editor");
  }
  return handled;
}

const KEY_CODES = new Map([
  ["Enter", 13],
  ["Backspace", 8],
  ["Delete", 46],
  ["ArrowLeft", 37],
  ["ArrowUp", 38],
  ["ArrowRight", 39],
  ["ArrowDown", 40],
  ["Home", 36],
  ["End", 35],
  ["Tab", 9],
]);

/**
 * Types `text` one character at a time as the DOM would: each character is
 * offered to the input handlers (`EditorView.inputHandler`), and when none
 * takes it the default `input.type` insert is dispatched.
 */
export function typeText(view: EditorView, text: string): void {
  for (const char of text) {
    const { from, to } = view.state.selection.main;
    const insert = () =>
      view.state.update({
        changes: { from, to, insert: char },
        selection: EditorSelection.cursor(from + char.length),
        userEvent: "input.type",
        scrollIntoView: true,
      });
    let handled = false;
    for (const handler of view.state.facet(EditorView.inputHandler)) {
      if (handler(view, from, to, char, insert)) {
        handled = true;
        break;
      }
    }
    if (!handled) {
      view.dispatch(insert());
    }
  }
}

/**
 * How many undo steps the view's stack holds: CodeMirror's history solo, the
 * room's manager in a room.
 */
export function undoSteps(view: EditorView): number {
  const manager = undoManagers.get(view);
  return manager ? manager.undoStack.length : undoDepth(view.state);
}

/** One undo step: CodeMirror's solo, the room's manager in a room. */
export function undoOnce(view: EditorView): void {
  const manager = undoManagers.get(view);
  if (manager) {
    manager.undo();
  } else {
    undo(view);
  }
}
