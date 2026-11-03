/**
 * The lazy chunk's entry. The loader imports this module dynamically, so
 * everything the grid needs and the eager bundle does not lands in its own
 * chunk: the grid widget and its cell drawing, the cell editor, the
 * forwarding between it and the main view, the grid's keyboard, presence
 * (peers' carets in the cells, the local caret out of an open cell) and
 * search and replace reaching into the cells.
 * `gridLoaded` tells the read-only table preview to step aside for the
 * tables the grid now draws, and `mainSelectionParked` tells the live
 * preview that main's selection is stale while a cell is open.
 */

import type { Extension } from "@codemirror/state";
import { StateEffect, StateField } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

import { cellForwarding } from "./cellEditor";
import { mainSelectionParked } from "../parkedSelection";
import { gridLoaded } from "./draws";
import { mainEntryKeys } from "./gridKeys";
import { gridSearch } from "./gridSearch";
import { gridView } from "./gridWidget";
import { openCellField } from "./openCell";
import { gridPresence } from "./presence";

/** Main's focus, as the state sees it: set on every focus change. */
const setMainFocus = StateEffect.define<boolean>();

const mainFocus = StateField.define<boolean>({
  create: () => false,
  update: (value, tr) => {
    for (const effect of tr.effects) {
      if (effect.is(setMainFocus)) return effect.value;
    }
    return value;
  },
});

/**
 * Main's selection is parked while a cell is open and main itself does not
 * hold the focus. With main focused (Shift+Tab back in while the cell is
 * still mounted) its selection is the live one, and its line shows its
 * marks again.
 */
const parking: Extension = [
  mainFocus,
  EditorView.focusChangeEffect.of((_state, focusing) =>
    setMainFocus.of(focusing),
  ),
  mainSelectionParked.compute(
    [openCellField, mainFocus],
    (state) =>
      state.field(openCellField, false) != null &&
      !state.field(mainFocus, false),
  ),
];

export function gridExtension(): Extension {
  return [
    gridLoaded.of(true),
    cellForwarding,
    parking,
    gridView,
    mainEntryKeys,
    gridPresence,
    gridSearch,
  ];
}
