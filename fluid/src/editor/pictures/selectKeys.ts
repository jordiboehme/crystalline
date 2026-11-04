/**
 * The keys around a picture (rulings 3, 4, 5 and 8), in the lazy tools chunk.
 *
 * - ArrowRight from an empty caret at a picture's start selects it, and with
 *   the picture selected puts the caret at its end; ArrowLeft mirrors it.
 *   Shift with an arrow is the editor's: the atomic range takes the picture
 *   whole.
 * - Backspace from its end and Delete from its start select it; either key
 *   on a selected picture removes it (`removePicture`), with its line when
 *   it stands alone on one.
 * - Enter on a selected picture is Escape then Enter: the caret after it,
 *   then the editor's own Enter (a list item continues); Escape alone puts
 *   the caret after it. Typing over it replaces it, as over any
 *   selection.
 * - A cut of a selected picture writes its reference text, the source
 *   bytes, as `text/plain` and removes it the way the keys do. Part 2's copy
 *   layer adds its marked HTML beside it (P9), which a paste in Fluid reads
 *   as "take the Markdown", so cut and paste moves a picture byte for byte.
 *   A copy is the editor's own: the same text, nothing removed.
 *
 * "At a picture's start" is judged from the drawn caret: a run of hidden
 * marks between the caret and the picture is stepped over first, as the
 * hidden-mark layer's arrows step. That layer leaves exactly those steps to
 * these keys (`pictureKeysOn`, `pictureForKeys`, P3). None of them acts on a
 * picture in a link or a table cell, nor without the picture layer (Raw):
 * there the keys are what they were.
 */

import type { EditorState, Extension } from "@codemirror/state";
import { EditorSelection, Prec } from "@codemirror/state";
import type { Command } from "@codemirror/view";
import { EditorView, keymap, runScopeHandlers } from "@codemirror/view";

import { writable } from "../elementEdit";
import { runAt } from "../marks/hiddenMarks";
import { pictureForKeys, pictureKeysOn, picturesDrawn } from "../marks/mode";
import type { PictureRef } from "./refs";
import { selectedPicture } from "./refs";
import { removePicture } from "./verbs";

/** The picture the one selection range covers exactly, when the keys own it. */
function chosen(state: EditorState): PictureRef | null {
  if (!state.facet(picturesDrawn) || state.selection.ranges.length > 1) {
    return null;
  }
  const ref = selectedPicture(state);
  return ref === null || ref.inLink || ref.inCell ? null : ref;
}

/**
 * The picture one step from an empty caret toward `forward`, past a run of
 * hidden marks on that side, or null.
 */
function arriving(state: EditorState, forward: boolean): PictureRef | null {
  const { selection } = state;
  if (selection.ranges.length > 1 || !selection.main.empty) {
    return null;
  }
  const head = selection.main.head;
  const run = runAt(state, head);
  const edge = run ? (forward ? run.to : run.from) : head;
  return pictureForKeys(state, edge, forward);
}

/** Selects `ref`, its head on the far side of the way the key went. */
function selectPicture(
  view: EditorView,
  ref: PictureRef,
  forward: boolean,
): void {
  view.dispatch({
    selection: forward
      ? EditorSelection.single(ref.from, ref.to)
      : EditorSelection.single(ref.to, ref.from),
    userEvent: "select",
    scrollIntoView: true,
  });
}

/** Puts an empty caret at `pos`. */
function caretAt(view: EditorView, pos: number): void {
  view.dispatch({
    selection: EditorSelection.cursor(pos),
    userEvent: "select",
    scrollIntoView: true,
  });
}

function arrow(forward: boolean): Command {
  return (view) => {
    const ref = chosen(view.state);
    if (ref !== null) {
      caretAt(view, forward ? ref.to : ref.from);
      return true;
    }
    const next = arriving(view.state, forward);
    if (next === null) {
      return false;
    }
    selectPicture(view, next, forward);
    return true;
  };
}

function deletion(forward: boolean): Command {
  return (view) => {
    const ref = chosen(view.state);
    if (ref !== null) {
      // One in from its start, so a picture that ends where this one begins
      // is not the one removed. A refusal (also on a page that is not
      // writable, `removePicture`'s own check) says why on its own.
      removePicture(view, ref.from + 1);
      return true;
    }
    const next = arriving(view.state, forward);
    if (next === null) {
      return false;
    }
    selectPicture(view, next, forward);
    return true;
  };
}

/**
 * Enter on a selected picture: Escape, then Enter (P30). The caret goes
 * after the picture and the Enter key runs again from there through every
 * keymap, so a list item or a quote continues as it does from a caret, and
 * the picture is kept.
 */
const enter: Command = (view) => {
  const { state } = view;
  const ref = chosen(state);
  if (ref === null || !writable(state)) {
    return false;
  }
  caretAt(view, ref.to);
  runScopeHandlers(
    view,
    new KeyboardEvent("keydown", {
      key: "Enter",
      keyCode: 13,
      bubbles: true,
      cancelable: true,
    }),
    "editor",
  );
  return true;
};

/** Escape on a selected picture: the caret after it. */
const escape: Command = (view) => {
  const ref = chosen(view.state);
  if (ref === null) {
    return false;
  }
  caretAt(view, ref.to);
  return true;
};

/**
 * A cut of a selected picture. At the default precedence and after Part 2's
 * copy layer in the configuration, so that layer has read the selection for
 * its HTML before this removes it; CodeMirror prevents the event for a
 * handler that answers true, which is what lets that layer add the HTML
 * (P9). The removal is `removePicture`, judged
 * as any delete (D12, P7). On a page that is not writable it is the
 * editor's: a copy and nothing removed.
 */
const cut = EditorView.domEventHandlers({
  cut(event, view) {
    const { state } = view;
    const ref = chosen(state);
    const data = event.clipboardData;
    if (ref === null || data === null || !writable(state)) {
      return false;
    }
    data.clearData();
    data.setData("text/plain", state.sliceDoc(ref.from, ref.to));
    removePicture(view, ref.from + 1);
    return true;
  },
});

/** The keys around a picture and its cut, for the main view. */
export const selectKeys: Extension = [
  pictureKeysOn.of(true),
  Prec.high(
    keymap.of([
      { key: "ArrowLeft", run: arrow(false) },
      { key: "ArrowRight", run: arrow(true) },
      { key: "Backspace", run: deletion(false) },
      { key: "Delete", run: deletion(true) },
      { key: "Enter", run: enter },
      { key: "Escape", run: escape },
    ]),
  ),
  cut,
];
