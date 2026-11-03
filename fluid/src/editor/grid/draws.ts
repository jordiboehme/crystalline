/**
 * What the eager side of the table grid shares with the lazy chunk: the test
 * for which tables the grid draws, and the two facets the chunk and the
 * preview layers talk through. Eager, so it imports only types from the
 * syntax tree and the table parser - never `./index`.
 */

import type { Extension } from "@codemirror/state";
import { Facet } from "@codemirror/state";
import type { SyntaxNodeRef } from "@lezer/common";

import { parseTable } from "../tableModel";

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
 * The prose layers a cell editor draws with: the same ones the main view
 * gets, provided by `previewConfig` so the two never drift apart.
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
