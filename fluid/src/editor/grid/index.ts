/**
 * The lazy chunk's entry. The loader imports this module dynamically, so
 * everything the grid needs and the eager bundle does not lands in its own
 * chunk: the grid widget and its cell drawing, the cell editor, the
 * forwarding between it and the main view, the grid's keyboard, presence
 * (peers' carets in the cells, the local caret out of an open cell) and
 * search and replace reaching into the cells.
 * `gridLoaded` tells the read-only table preview to step aside for the
 * tables the grid now draws.
 */

import type { Extension } from "@codemirror/state";

import { cellForwarding } from "./cellEditor";
import { gridLoaded } from "./draws";
import { mainEntryKeys } from "./gridKeys";
import { gridSearch } from "./gridSearch";
import { gridView } from "./gridWidget";
import { gridPresence } from "./presence";

export function gridExtension(): Extension {
  return [
    gridLoaded.of(true),
    cellForwarding,
    gridView,
    mainEntryKeys,
    gridPresence,
    gridSearch,
  ];
}
