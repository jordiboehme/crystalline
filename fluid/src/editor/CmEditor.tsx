/**
 * The React binding for the editor view: one component owning one
 * EditorView's lifecycle, created on mount and destroyed on unmount.
 *
 * Deliberately not controlled: the buffer is the source of truth while the
 * editor is open, and a parent that needs the text subscribes through
 * onDocChanged rather than re-rendering the view per keystroke. A different
 * engram is a different mount - key this component by its address.
 */

import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { ReactElement } from "react";
import { useEffect, useRef } from "react";

import { buildEditorState } from "./setup";

export interface CmEditorProps {
  /** The document at mount. The component is keyed by its address; it never
   *  chases prop changes - a new engram is a new mount. */
  initialDoc: string;
  extensions: Extension[];
  ariaLabel: string;
  /** The live view, for parents that dispatch (form panel, jump-to-line). */
  onReady?: (view: EditorView) => void;
  onDocChanged?: (doc: string) => void;
}

export default function CmEditor({
  initialDoc,
  extensions,
  ariaLabel,
  onReady,
  onDocChanged,
}: CmEditorProps): ReactElement {
  const host = useRef<HTMLDivElement | null>(null);
  // Mount-time snapshots: the view is created once, and recreating it on a
  // render would drop selection, history and focus for nothing.
  const initial = useRef({
    initialDoc,
    extensions,
    ariaLabel,
    onReady,
    onDocChanged,
  });

  useEffect(() => {
    const node = host.current;
    if (!node) {
      return;
    }
    const {
      initialDoc: doc,
      extensions: exts,
      ariaLabel: label,
      onReady: ready,
      onDocChanged: changed,
    } = initial.current;
    const view = new EditorView({
      state: buildEditorState(doc, exts, label, changed),
      parent: node,
    });
    ready?.(view);
    return () => {
      view.destroy();
    };
  }, []);

  return <div ref={host} className="min-h-64" />;
}
