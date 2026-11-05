/**
 * The lazy chunk's entry. The loader imports this module dynamically, so
 * everything the layer needs and the eager bundle does not lands in its own
 * chunk: the parser, the verdict check and the field.
 * `drawnDiagrams` tells the read-only mermaid preview to step aside for the
 * fences the chunk draws; `diagramLoaded` tells the rest that it is in.
 */

import type { Extension } from "@codemirror/state";

import { diagramLoaded, drawnDiagrams } from "./draws";
import {
  diagramField,
  diagramTheme,
  pointerWatcher,
  verdictWatcher,
} from "./field";

export function diagramExtension(): Extension {
  return [
    diagramLoaded.of(true),
    diagramField,
    drawnDiagrams.compute([diagramField], (state) =>
      state.field(diagramField).fences.map((fence) => fence.from),
    ),
    verdictWatcher,
    pointerWatcher,
    diagramTheme,
  ];
}
