/**
 * The undo stack an editor runs on, and the shifted-Z redo key.
 *
 * A buffer carries exactly one undo stack (see `baseExtensions`): CodeMirror's
 * history outside a room, the room's Y.UndoManager in one. `stackUndo` runs
 * whichever is there.
 *
 * Why the redo key needs its own binding: CodeMirror's keymap runner looks up
 * a keydown by its unshifted name first when `event.key` is a lower-case
 * letter. Ctrl+Shift+Z with Caps Lock on (or any input that sends key "z"
 * with shiftKey) is therefore offered to `Mod-z` before `Mod-Shift-z`. The
 * room's undo command always reports handled (`undo() || true` in
 * y-codemirror.next) and CodeMirror's history undo reports handled whenever
 * its stack is not empty, so the press undoes and redo never runs. On a Mac
 * w3c-keyname reads the key from keyCode for Cmd+Shift, so it is "Z" there,
 * but the rule below holds on every platform. Also, CodeMirror's own history
 * binds Ctrl-Shift-z for Linux only, so on Windows a solo buffer had no
 * Ctrl+Shift+Z redo at all.
 */

import {
  historyField,
  redo as historyRedo,
  undo as historyUndo,
} from "@codemirror/commands";
import type { Extension } from "@codemirror/state";
import { Prec } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { keymap } from "@codemirror/view";
import { ySyncFacet, yUndoManagerKeymap } from "y-codemirror.next";

/** Whether `view` carries an undo stack: CodeMirror's history or a room's. */
export function hasUndoStack(view: EditorView): boolean {
  return (
    view.state.field(historyField, false) !== undefined ||
    view.state.facet(ySyncFacet) !== undefined
  );
}

/**
 * Runs one undo or redo on `view`'s stack: CodeMirror's history solo,
 * Y.UndoManager in a room. Returns whether a command ran.
 */
export function stackUndo(view: EditorView, which: "undo" | "redo"): boolean {
  if (view.state.field(historyField, false) !== undefined) {
    return (which === "undo" ? historyUndo : historyRedo)(view);
  }
  if (view.state.facet(ySyncFacet) === undefined) return false;
  const key = which === "undo" ? "Mod-z" : "Mod-Shift-z";
  const binding = yUndoManagerKeymap.find((b) => b.key === key);
  try {
    return binding?.run?.(view) ?? false;
  } catch {
    // A room built without an undo manager has no stack to run.
    return false;
  }
}

/**
 * Whether `event` is the redo chord Mod+Shift+Z (Cmd on a Mac, Ctrl
 * elsewhere), whatever case `event.key` has.
 *
 * Nothing while an input method composes (a dead key or an IME), and no Alt:
 * Ctrl+Alt is AltGr on Windows. The letter is read from `event.key` when it is
 * a Latin letter, so a layout that puts another letter on the Z position
 * (QWERTZ, AZERTY) keeps its own meaning for that key. For any other key (a
 * Cyrillic layout, say) the physical key decides, as CodeMirror's runner
 * falls back to keyCode too.
 */
export function isShiftedZ(event: KeyboardEvent): boolean {
  if (event.isComposing || event.keyCode === 229) return false;
  const mac =
    typeof navigator !== "undefined" &&
    /Mac|iPhone|iPad|iPod/.test(navigator.platform);
  const mod = mac
    ? event.metaKey && !event.ctrlKey
    : event.ctrlKey && !event.metaKey;
  if (!mod || !event.shiftKey || event.altKey) return false;
  if (/^[a-z]$/i.test(event.key)) return event.key.toLowerCase() === "z";
  return event.code === "KeyZ" || event.keyCode === 90;
}

/**
 * Sends Mod+Shift+Z to `redo` before any other binding sees it.
 *
 * An `any` binding in a highest-precedence keymap runs first for every key,
 * and it gets the event, so it reads `shiftKey` itself instead of trusting
 * the name the runner looks up first. `redo` answers whether the key is
 * taken; when it is, `Mod-z` never runs for this press.
 */
export function shiftedZRedo(redo: (view: EditorView) => boolean): Extension {
  return Prec.highest(
    keymap.of([
      { any: (view, event) => (isShiftedZ(event) ? redo(view) : false) },
    ]),
  );
}

/**
 * The shifted-Z redo of a main editor, on its own stack. The key is taken
 * whenever the buffer has a stack, also when there is nothing to redo:
 * letting it through would reach `Mod-z` and undo.
 */
export const mainShiftedZRedo: Extension = shiftedZRedo((view) => {
  if (!hasUndoStack(view)) return false;
  stackUndo(view, "redo");
  return true;
});
