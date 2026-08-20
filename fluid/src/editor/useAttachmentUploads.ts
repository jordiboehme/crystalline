/**
 * One upload flow, three ways in, wired to a screen.
 *
 * The buffer's extensions are read once at mount, so the paste and drop
 * handlers cannot close over this render's values: they close over a ref that
 * every later render refreshes instead. That is what lets the same extension
 * live for the life of the buffer while still uploading into whatever view and
 * domain the screen holds now - and it is why `attach`, which the toolbar's
 * picker calls, is the same stable function rather than a second path with its
 * own copy of the rules.
 */

import type { Extension } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { useCallback, useEffect, useMemo, useRef } from "react";

import { attachmentUploads, uploadAttachments } from "./attachments";

/** What a screen needs to offer uploads on its buffer. */
export interface AttachmentUploadOptions {
  /** The domain the files are stored in. */
  domain: string;
  /** The live buffer, read at the moment of the upload rather than at mount. */
  view: () => EditorView | null;
  /** A refusal in words an author can act on; null clears a standing one. */
  onError: (message: string | null) => void;
}

/** The two halves a screen wires: into the buffer, and onto the toolbar. */
export interface AttachmentUploads {
  /** The paste and drop handlers, stable for the life of the buffer. */
  extension: Extension;
  /** What the toolbar's file picker hands its files to. */
  attach: (files: File[]) => void;
}

export function useAttachmentUploads(
  options: AttachmentUploadOptions,
): AttachmentUploads {
  const latest = useRef(options);
  // Written after the render rather than during it: what the handlers read is
  // always the last committed render's values.
  useEffect(() => {
    latest.current = options;
  });
  const attach = useCallback((files: File[]) => {
    const { domain, view, onError } = latest.current;
    const target = view();
    if (target === null) {
      return;
    }
    void uploadAttachments({ domain, files, view: target, onError });
  }, []);
  const extension = useMemo(
    // `attachmentUploads` only stores this callback in the DOM handlers
    // CodeMirror runs from a paste or a drop, so nothing on this call stack
    // reads `latest.current` during the render - but the checker cannot see
    // through the call to confirm it.
    // eslint-disable-next-line react-hooks/refs
    () => attachmentUploads(attach),
    [attach],
  );
  return { extension, attach };
}
