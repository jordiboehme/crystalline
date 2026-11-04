/**
 * The lazy picture tools chunk, loaded by `loader.ts`: the keys around a
 * picture (`selectKeys`), the drop line (`dropLine`) and moving a picture by
 * dragging it (`dragMove`) and resizing it by a corner handle (`resize`).
 */

import type { Extension } from "@codemirror/state";

import { dragMove } from "./dragMove";
import { dropLine } from "./dropLine";
import { resize } from "./resize";
import { selectKeys } from "./selectKeys";

/** Everything the tools add to a main view. */
export function pictureToolsExtension(): Extension {
  return [selectKeys, dropLine, dragMove, resize];
}
