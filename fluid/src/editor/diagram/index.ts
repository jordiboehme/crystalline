/**
 * The lazy chunk's entry. The loader imports this module dynamically, so
 * everything the layer needs and the eager bundle does not lands in its own
 * chunk: the parser, the verdict check and the field.
 * `drawnDiagrams` tells the read-only mermaid preview to step aside for the
 * fences the chunk draws; `diagramLoaded` tells the rest that it is in.
 */

import type { Extension } from "@codemirror/state";

import { diagramLoaded, drawnDiagrams } from "./draws";
import { diagramField, pointerWatcher, verdictWatcher } from "./field";
import { diagramIntent, intentWatcher } from "./intent";
import { diagramTheme } from "./widget";

export function diagramExtension(): Extension {
  return [
    diagramLoaded.of(true),
    diagramField,
    // After the field: it reads the fences the field draws.
    diagramIntent,
    drawnDiagrams.compute([diagramField], (state) =>
      state.field(diagramField).fences.map((fence) => fence.from),
    ),
    verdictWatcher,
    pointerWatcher,
    intentWatcher,
    diagramTheme,
  ];
}
