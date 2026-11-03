/**
 * Test helpers for the hidden-mark editing: a view built through
 * `surfaceExtensions` (the way both screens build theirs), a caret, keys shaped
 * like a browser keydown, typing that goes through the input handlers as the
 * DOM would, and undo that matches the surface (CodeMirror solo, the room's
 * `Y.UndoManager` in a room).
 */

import { undo } from "@codemirror/commands";
import { Compartment, EditorSelection, EditorState } from "@codemirror/state";
import { EditorView, keymap, runScopeHandlers } from "@codemirror/view";
import { QueryClient } from "@tanstack/react-query";
import { yCollab, yUndoManagerKeymap } from "y-codemirror.next";
import { Awareness } from "y-protocols/awareness";
import * as Y from "yjs";

import { engramAdapter, reflectionAdapter } from "../editor/adapter";
import { marksOnEditLine } from "../editor/marks/mode";
import { roomUndo } from "../editor/roomUndo";
import { surfaceExtensions } from "../editor/surface";
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
}

const views = new Set<EditorView>();
const undoManagers = new WeakMap<EditorView, Y.UndoManager>();
const roomTexts = new WeakMap<EditorView, Y.Text>();
const markCompartments = new WeakMap<EditorView, Compartment>();

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
  let doc = text;
  let roomText: Y.Text | null = null;
  if (options.room) {
    const ydoc = new Y.Doc();
    const ytext = ydoc.getText("content");
    roomText = ytext;
    ytext.insert(0, text);
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
        EditorState.lineSeparator.of("\n"),
        ...surfaceExtensions({
          adapter,
          dark: false,
          history: !options.room,
          screen,
          preview: new Compartment(),
          raw: options.raw ?? false,
          images: [],
          uploads: [],
          onTableContext: () => {},
          onImageContext: () => {},
          onElementContext: () => {},
          onFloatingContext: () => {},
          marks,
          showMarks: options.mode === "show",
        }),
      ],
    }),
    parent: document.body,
  });
  views.add(view);
  markCompartments.set(view, marks);
  if (undoManager) {
    undoManagers.set(view, undoManager);
  }
  if (roomText) {
    roomTexts.set(view, roomText);
  }
  return parsedView(view);
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

/** One undo step: CodeMirror's solo, the room's manager in a room. */
export function undoOnce(view: EditorView): void {
  const manager = undoManagers.get(view);
  if (manager) {
    manager.undo();
  } else {
    undo(view);
  }
}
