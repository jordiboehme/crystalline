/**
 * The lazy picture tools chunk, loaded by `loader.ts`: the keys around a
 * picture (`selectKeys`). The drop line, the drag-move and the resize
 * handles join here.
 */

import type { Extension } from "@codemirror/state";

import { selectKeys } from "./selectKeys";

/** Everything the tools add to a main view. */
export function pictureToolsExtension(): Extension {
  return [selectKeys];
}
