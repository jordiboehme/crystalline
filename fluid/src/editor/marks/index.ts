/**
 * The hidden-mark layer. A placeholder until the drawing arrives: for now it
 * only declares that the layer is installed, which is what separates the
 * preview from Raw for `marksHiddenOnLine`.
 */

import type { Extension } from "@codemirror/state";

import { hiddenMarkLayerOn } from "./mode";

export function hiddenMarkLayer(): Extension {
  return hiddenMarkLayerOn.of(true);
}
