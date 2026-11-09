/**
 * What every menu item and connect mode asks before it acts, whatever the kind of
 * diagram: where the fence is now, and whether it can be acted on.
 */

import type { EditorView } from "@codemirror/view";

import type { MermaidFence } from "./fence";
import { sayAt } from "./focus";
import { diagramIntent, intentStale } from "./intent";
import { fenceNow } from "./labelEditor";
import { verdictFor } from "./verdict";

/** Where the fence is now: the intent follows it through what a peer writes. */
export function placeNow(main: EditorView, anchor: number): number {
  return main.state.field(diagramIntent, false)?.anchor ?? anchor;
}

/**
 * The fence an item can act on, or null with the person told. The parse may be
 * behind, or a peer's line may read wrong just now, and a later try can differ.
 */
export function fenceToAct(
  main: EditorView,
  anchor: number,
): MermaidFence | null {
  const fence = fenceNow(main, anchor);
  if (fence === null) {
    sayAt(main, anchor, "gone");
    return null;
  }
  if (intentStale(main.state) || verdictFor(fence.body)?.ok === false) {
    sayAt(main, anchor, "stale");
    return null;
  }
  return fence;
}
