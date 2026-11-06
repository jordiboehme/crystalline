/**
 * The lazy chunk's entry. The loader imports this module dynamically, so
 * everything the layer needs and the eager bundle does not lands in its own
 * chunk: the parser, the verdict check and the field.
 * `drawnDiagrams` tells the read-only mermaid preview to step aside for the
 * fences the chunk draws; `diagramLoaded` tells the rest that it is in.
 */

import type { Extension } from "@codemirror/state";

import { mainSelectionParked } from "../parkedSelection";
import {
  diagramLoaded,
  drawnDiagrams,
  drawnFenceLines,
  enterDrawnFence,
} from "./draws";
import { diagramField, pointerWatcher, verdictWatcher } from "./field";
import { flowActions } from "./flowchart/actions";
import {
  diagramEntryKeys,
  diagramFocus,
  enterCanvas,
  fenceMarksGuard,
} from "./focus";
import { diagramIntent, intentWatcher } from "./intent";
import { labelEditorPlugin } from "./labelEditor";
import { diagramTheme, flipBackHold } from "./widget";

export function diagramExtension(): Extension {
  return [
    diagramLoaded.of(true),
    diagramField,
    // After the field: it reads the fences the field draws.
    diagramIntent,
    drawnDiagrams.compute([diagramField], (state) =>
      state.field(diagramField).fences.map((fence) => fence.from),
    ),
    drawnFenceLines.compute([diagramField], (state) =>
      state.field(diagramField).fences.map((fence) => ({
        from: state.doc.lineAt(fence.from).from,
        to: state.doc.lineAt(fence.to).to,
      })),
    ),
    enterDrawnFence.of(enterCanvas),
    // No bar writes into a hidden fence while the keyboard is on the drawing.
    mainSelectionParked.compute([diagramIntent], (state) => {
      const intent = state.field(diagramIntent);
      return intent !== null && intent.released !== true;
    }),
    fenceMarksGuard,
    diagramEntryKeys,
    diagramFocus,
    verdictWatcher,
    pointerWatcher,
    intentWatcher,
    labelEditorPlugin,
    flowActions,
    flipBackHold,
    diagramTheme,
  ];
}
