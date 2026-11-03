/**
 * Runs Part 1's grid suites with the hidden-mark layer in the cell (Task 12,
 * fix round 1, F5). A wrapper test file sets the mode before its tests run
 * and imports the suite; `soloMain` and `roomMain` then give every main view
 * the layer for its cells (`cellPreviewLayers`) and the marks setting, the
 * way `previewConfig` does on the real surface. Main itself keeps no layer, as
 * the suites were written for.
 */

import type { Extension } from "@codemirror/state";
import { beforeAll } from "vitest";

import { cellPreviewLayers } from "../editor/grid/draws";
import { hiddenMarkLayer } from "../editor/marks";
import { marksOnEditLine } from "../editor/marks/mode";

export type CellMarksMode = "off" | "hidden" | "show";

let mode: CellMarksMode = "off";

/** The mode the views built from here on use. */
export function cellMarksMode(): CellMarksMode {
  return mode;
}

/** Sets the mode for every test of the calling file. */
export function runWithCellMarks(next: CellMarksMode): void {
  beforeAll(() => {
    mode = next;
  });
}

/** What a main view adds for the current mode: nothing when off. */
export function cellMarksExtensions(): Extension[] {
  if (mode === "off") return [];
  return [
    cellPreviewLayers.of([hiddenMarkLayer()]),
    marksOnEditLine.of(mode === "show"),
  ];
}
