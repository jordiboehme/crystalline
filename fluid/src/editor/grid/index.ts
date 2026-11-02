/**
 * The lazy chunk's entry. The loader imports this module dynamically, so
 * everything the grid needs and the eager bundle does not lands in its own
 * chunk. A placeholder until the widget arrives: it adds nothing, and does not
 * provide `gridLoaded`, so every table keeps the read-only preview meanwhile.
 */

import type { Extension } from "@codemirror/state";

export function gridExtension(): Extension {
  return [];
}
