/**
 * The lazy chunk's entry. The loader imports this module dynamically, so
 * everything the grid needs and the eager bundle does not lands in its own
 * chunk: the grid widget and its cell drawing, the cell editor, the
 * forwarding between it and the main view, and the grid's keyboard.
 * `gridLoaded` tells the read-only table preview to step aside for the
 * tables the grid now draws.
 */

import type { Extension } from "@codemirror/state";

import { cellForwarding } from "./cellEditor";
import { gridLoaded } from "./draws";
import { mainEntryKeys } from "./gridKeys";
import { gridView } from "./gridWidget";

export function gridExtension(): Extension {
  return [gridLoaded.of(true), cellForwarding, gridView, mainEntryKeys];
}
