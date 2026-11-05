/**
 * What the eager side of the table grid shares with the lazy chunk: the test
 * for which tables the grid draws, and the facets the chunk, the preview
 * layers and the picture drop landing talk through. Eager, so it imports
 * only types from the syntax tree and the grid's structure, and the table
 * parser - never `./index`.
 */

import type { EditorState, Extension } from "@codemirror/state";
import { Facet } from "@codemirror/state";
import type { SyntaxNodeRef } from "@lezer/common";

import { parseTable } from "../tableModel";
import type { GridTable } from "./structure";

/** The slice of a document `gridDraws` reads the table's text through. */
export interface TextSource {
  sliceString(from: number, to: number): string;
}

/**
 * Whether the grid is loaded and drawing. Provided by the lazy chunk's
 * `gridExtension()`, read by `fencePreviews` to decide whether the read-only
 * preview is still needed: while the chunk loads, and for good when its import
 * fails, every table keeps the preview.
 */
export const gridLoaded = Facet.define<boolean, boolean>({
  combine: (values) => values.some(Boolean),
});

/**
 * Whether the grid chunk could not be loaded and the loader gave up (three
 * failed imports). The read-only table preview then says how to get the
 * grid back. Provided by the loader's compartment, read by `fencePreviews`.
 */
export const gridUnavailable = Facet.define<boolean, boolean>({
  combine: (values) => values.some(Boolean),
});

/**
 * The prose layers a cell editor draws with, and the hidden-mark layer after
 * them: the same ones the main view gets, provided by `previewConfig` so the
 * two never drift apart.
 */
export const cellPreviewLayers = Facet.define<Extension[], Extension[]>({
  combine: (values) => values.flat(),
});

/**
 * True for a `Table` outside any blockquote or list item whose text the table
 * parser accepts. Those are the tables the grid draws; the rest stay source
 * with the read-only preview.
 */
export function gridDraws(node: SyntaxNodeRef, doc: TextSource): boolean {
  if (node.name !== "Table") return false;
  for (let up = node.node.parent; up; up = up.parent) {
    if (up.name === "Blockquote" || up.name === "ListItem") return false;
  }
  return parseTable(doc.sliceString(node.from, node.to)) !== null;
}

/**
 * The tables the grid draws in a state. Past where the parse reaches, the
 * grid keeps the tables it drew, read again at their mapped places (D58):
 * the open cell is read from those until the tree reaches them, and a
 * picture dropped over one lands below it rather than inside its source. The
 * grid provides it; it lives here, on the eager side, so neither the open
 * cell nor the drop landing imports the grid, and the landing pulls nothing
 * of the lazy chunk into the eager bundle.
 */
export const drawnGridTables =
  Facet.define<(state: EditorState) => readonly GridTable[]>();
