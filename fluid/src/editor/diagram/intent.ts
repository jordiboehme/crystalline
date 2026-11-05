/**
 * Where the diagram intent will live (Task 9). Until then this is the one
 * teardown path as a stub: the field calls it when a drawn fence stops being
 * drawn, and nothing has an intent to close yet.
 *
 * Task 9 replaces the body and keeps the signature. It must not dispatch
 * synchronously: the field calls it from a view update's microtask, but a
 * caller inside an update would otherwise hit CodeMirror's re-entrancy error.
 */

import type { EditorView } from "@codemirror/view";

export type IntentCloseReason = "user" | "gone" | "raw" | "swap" | "destroy";

/** Where the focus goes after the intent closed. */
export type IntentFocus = "main-after" | "none";

export function closeDiagramIntent(
  _main: EditorView,
  _reason: IntentCloseReason,
  _focusTo: IntentFocus = "main-after",
): void {
  // No intent exists before Task 9.
}
