/**
 * The shared editor session shell: one buffer's checksum/dirty/notice state,
 * the debounced dry-run validation gate, the Mod-S save event, drafts and the
 * 412 conflict flow - everything `EngramEditor` and `ManifestEditor` must
 * agree on, in the one place they now share. The screens keep what differs:
 * extensions, panels, navigation, cache seeding.
 */

import type { Extension } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { keymap } from "@codemirror/view";
import { useMutation } from "@tanstack/react-query";
import type { RefObject } from "react";
import { useCallback, useEffect, useRef, useState } from "react";

import { problemDetail } from "../api/client";
import type { ValidateResponse } from "../api/model";
import type { SaveConflict } from "../api/writes";
import { conflictOf } from "../api/writes";
import type { Draft } from "./drafts";
import { clearDraft, DRAFT_DEBOUNCE_MS, readDraft, writeDraft } from "./drafts";
import { docText, replaceBuffer } from "./setup";
import { useValidationGate } from "./useValidationGate";

export interface Notice {
  kind: "problem" | "done";
  text: string;
}

/** What a landed save tells the session about itself. */
export interface SessionSaveReceipt {
  content: string;
  checksum: string;
}

/**
 * How long a pause in typing waits before a dry-run validate fires. The rule
 * lives in the gate module now; re-exported here so the session's own import
 * surface is unchanged.
 */
export { VALIDATE_DEBOUNCE_MS } from "./useValidationGate";

/** The DOM event the buffer's save binding raises on the editor's own node. */
export const SAVE_EVENT = "crystalline:save";

/**
 * Mod-S inside the buffer, as an extension fixed at module level.
 *
 * It asks for a save rather than performing one: a keymap is built once and
 * lives as long as the view, so a handler that closed over a component's save
 * would be closing over the first render's copy of it forever. The event it
 * raises is listened for beside the view, where the current save is in scope,
 * and CodeMirror still owns the key so the browser never gets its own save
 * dialog. The event travels on the editor's own DOM node, so two editors on
 * one page could never hear each other's.
 */
export const saveKeymap = keymap.of([
  {
    key: "Mod-s",
    preventDefault: true,
    run: (view) => {
      view.dom.dispatchEvent(new CustomEvent(SAVE_EVENT));
      return true;
    },
  },
]);

export interface EditorSessionOptions {
  initialContent: string;
  /** "" when the server sent no checksum. */
  initialChecksum: string;
  /** Draft key triple; `ManifestEditor` passes its fixed "MANIFEST" slot. */
  draftUser: string;
  draftDomain: string;
  draftSlot: string;
  /** What /validate is asked about. */
  validateDomain: string;
  validatePath: string | null;
  /** The transport save; the hook owns when it fires. */
  save: (content: string, token: string) => Promise<SessionSaveReceipt>;
  /** After a successful save: rename-follow, cache seeding - screen-specific. */
  onSaved?: (receipt: SessionSaveReceipt) => void;
  /** Rebuild the surface extensions for swapped-in content (conflict/draft). */
  extensionsFor: (content: string) => Extension[];
  ariaLabel: string;
  /**
   * How saves travel. "solo" (the default) is the PUT + If-Match mutation
   * with the hook's own drafts, unload prompt and 412 flow. "collab" hands
   * the save path to the session server: `requestSave` - and so the
   * SAVE_EVENT/Mod-S path, which calls it - delegates to `flush` and NEVER
   * runs the PUT mutation; the beforeunload prompt and the conflict handlers
   * go inert (the collab surface owns both, keyed on session state); draft
   * snapshots run through `draftContent`; and buffer swaps never rebuild the
   * state (a co-editing binding must not be replaced via `setState`) and
   * always convert incoming text to LF session space first. The solo
   * machinery must not stay live under collab: a Mod-S PUT carrying the
   * mount-time checksum against a server that debounce-saves would raise
   * spurious 412s at best and clobber session saves at worst.
   */
  transport?: "solo" | "collab";
  /** Required when transport is "collab": the session flush request. */
  flush?: () => void;
  /** Collab only: maps the LF buffer to file space for draft snapshots so
   *  drafts stay interchangeable with the solo flow's. ONE writer: the
   *  hook's debounce is the only draft writer in either mode. */
  draftContent?: (buffer: string) => string;
}

export interface EditorSession {
  buffer: string;
  setBuffer: (next: string) => void;
  dirty: boolean;
  checksum: string;
  viewRef: RefObject<EditorView | null>;
  onReady: (view: EditorView) => void;
  report: ValidateResponse | null;
  hardErrors: number;
  checking: boolean;
  validationUnavailable: boolean;
  requestSave: () => void;
  saving: boolean;
  notice: Notice | null;
  setNotice: (notice: Notice | null) => void;
  conflict: SaveConflict | null;
  onConflictClose: () => void;
  onConflictOverwrite: () => void;
  onConflictTakeServer: () => void;
  offeredDraft: Draft | null;
  restoreDraft: () => void;
  discardDraft: () => void;
  /**
   * Store the buffer as a draft right now, in draft space. For the deliberate
   * snapshots that precede giving the text up: the collab conflict's "take
   * the file version" and the walk-out from an accepted deletion.
   */
  snapshotDraft: () => void;
  /** Swap the buffer wholesale (the conflict and draft paths use it). */
  replaceWith: (content: string) => void;
  /** Collab only: the server reported a landed save - clear the draft and
   *  treat the current buffer as the saved text. */
  noteSaved: () => void;
}

export function useEditorSession(options: EditorSessionOptions): EditorSession {
  const {
    initialContent,
    initialChecksum,
    draftUser,
    draftDomain,
    draftSlot,
    validateDomain,
    validatePath,
    save: transportSave,
    onSaved,
    extensionsFor,
    ariaLabel,
    transport = "solo",
    flush,
    draftContent,
  } = options;
  const viewRef = useRef<EditorView | null>(null);
  // What the server holds, moved forward on every successful save.
  const [checksum, setChecksum] = useState(initialChecksum);
  const [savedText, setSavedText] = useState(initialContent);
  const [buffer, setBuffer] = useState(initialContent);
  const [notice, setNotice] = useState<Notice | null>(null);
  // The 412 view: set on a stale save, cleared by every one of its exits.
  const [conflict, setConflict] = useState<SaveConflict | null>(null);
  const dirty = buffer !== savedText;
  /**
   * The buffer as a draft is stored: file space in both modes, because
   * `draftContent` maps the collab buffer on the way out. Identity on the
   * solo surface, where the buffer already carries the file's own endings.
   */
  const asStoredDraft = useCallback(
    (text: string) => (draftContent ? draftContent(text) : text),
    [draftContent],
  );
  // A browser-stored draft newer than what the server sent, read once per
  // mount and offered through the screen's recovery banner.
  //
  // The comparison happens in DRAFT space, not in buffer space: a session
  // buffer is LF while its stored draft carries the file's endings, so
  // comparing the two directly would find every draft of a CRLF file
  // "different" and offer it on every mount - and accepting one dispatches a
  // whole-document rewrite into the shared text of a room that never changed.
  const [offeredDraft, setOfferedDraft] = useState(() => {
    const stored = readDraft(draftUser, draftDomain, draftSlot);
    const mounted = asStoredDraft(initialContent);
    return stored !== null && stored.content !== mounted ? stored : null;
  });

  // The dry run, its debounce and its last-landed rule: one gate, shared with
  // whatever else has to judge a buffer.
  const { report, hardErrors, checking, validationUnavailable } =
    useValidationGate(validateDomain, validatePath, buffer);

  /**
   * Store the buffer as it stands, in draft space. The debounce below is the
   * only writer that runs on its own; the deliberate snapshots - taking the
   * server's version, handing a room's conflict to the file, walking out of a
   * deleted engram, keeping what a landed save did not carry - go through the
   * same spelling rather than a second one.
   */
  const snapshotDraft = useCallback(
    (text: string) => {
      writeDraft(draftUser, draftDomain, draftSlot, {
        // In collab mode the buffer is LF session space; `draftContent` maps
        // it to file space so the stored draft matches the solo flow's.
        content: asStoredDraft(text),
        baseChecksum: checksum,
        savedAt: new Date().toISOString(),
      });
    },
    [draftUser, draftDomain, draftSlot, checksum, asStoredDraft],
  );

  const save = useMutation({
    // The token travels with the content rather than being read from
    // `checksum` inside the mutation: a conflict's overwrite moves the
    // checksum state and fires the retry in the same handler, and a mutation
    // that read `checksum` from its closure would still see the pre-update
    // value on that first tick.
    mutationFn: ({ content, token }: { content: string; token: string }) =>
      transportSave(content, token),
    onSuccess: (saved, sent) => {
      const view = viewRef.current;
      // What the draft is for is the text the server does not have. A save
      // that carried the buffer as it stands has made the draft redundant and
      // it goes; a save whose content the buffer has already moved past has
      // not, and clearing the draft on it would delete the only copy of
      // everything typed while it was in flight. Read back through `docText`
      // rather than from `buffer`, which is a render's value and this handler
      // runs whenever the answer happens to arrive.
      if (view && docText(view.state) !== sent.content) {
        snapshotDraft(docText(view.state));
      } else {
        clearDraft(draftUser, draftDomain, draftSlot);
      }
      setChecksum(saved.checksum);
      setSavedText(saved.content);
      setNotice({ kind: "done", text: "Saved" });
      onSaved?.(saved);
    },
    onError: (error: Error) => {
      const stale = conflictOf(error);
      if (stale) {
        setConflict(stale);
        return;
      }
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const requestSave = () => {
    if (transport === "collab") {
      // The server owns saving in a session; Mod-S and the Save button both
      // land here and both ask it to flush. The PUT mutation, its checksum
      // and its 412 flow never run in this mode - a stale-If-Match PUT
      // against a debounce-saving server would fight the session.
      flush?.();
      return;
    }
    const view = viewRef.current;
    // Hard errors gate both paths a save can start from, the button and the
    // keyboard, because both call this function rather than dispatching a
    // mutation of their own: a check here is a check for either.
    if (view && !save.isPending && hardErrors === 0) {
      setNotice(null);
      // `docText` rather than `doc.toString()`: what goes on the wire is the
      // file's own bytes, line endings included.
      save.mutate({ content: docText(view.state), token: checksum });
    }
  };
  // The keymap's request, answered here where the current save is in scope.
  // Re-bound after every render on purpose: what the binding must reach is
  // the latest save, not the one that existed when the view was created.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) {
      return;
    }
    const onSaveRequested = () => {
      requestSave();
    };
    view.dom.addEventListener(SAVE_EVENT, onSaveRequested);
    return () => {
      view.dom.removeEventListener(SAVE_EVENT, onSaveRequested);
    };
  });

  // The safety net: a pause in typing snapshots the buffer to browser
  // storage, so a crash, a closed tab or an accidental navigation away loses
  // at most a debounce window of work.
  useEffect(() => {
    if (!dirty) {
      return;
    }
    const timer = setTimeout(() => {
      snapshotDraft(buffer);
    }, DRAFT_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [buffer, dirty, snapshotDraft]);

  // Closing the tab or reloading it loses the draft's covering banner, so it
  // gets its own prompt; in-app navigation is already covered by the draft.
  // Collab mode opts out: its prompt keys on SESSION save state, and this one
  // would misfire - a CRLF file's file-space `initialContent` never equals
  // the LF buffer, so `dirty` would read true forever.
  useEffect(() => {
    if (!dirty || transport === "collab") {
      return;
    }
    const prompt = (event: BeforeUnloadEvent) => {
      event.preventDefault();
    };
    window.addEventListener("beforeunload", prompt);
    return () => {
      window.removeEventListener("beforeunload", prompt);
    };
  }, [dirty, transport]);

  const replaceWith = (content: string) => {
    const view = viewRef.current;
    if (!view) {
      return;
    }
    if (transport === "collab") {
      // A co-editing-bound state must never be rebuilt via `setState` (the
      // binding would detach from the swapped-in state), and the shared text
      // only ever holds LF session space: convert, then dispatch an ordinary
      // transaction the binding propagates to the room.
      const session = content.replace(/\r\n/g, "\n");
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: session },
      });
      return;
    }
    replaceBuffer(view, content, extensionsFor, ariaLabel, setBuffer);
  };

  return {
    buffer,
    setBuffer,
    dirty,
    checksum,
    viewRef,
    onReady: (view) => {
      viewRef.current = view;
    },
    report,
    hardErrors,
    checking,
    validationUnavailable,
    requestSave,
    saving: save.isPending,
    notice,
    setNotice,
    conflict,
    onConflictClose: () => {
      setConflict(null);
    },
    onConflictOverwrite: () => {
      if (!conflict) {
        return;
      }
      setChecksum(conflict.currentChecksum);
      setConflict(null);
      const view = viewRef.current;
      if (view) {
        // The explicit token: the state update above has not landed on this
        // tick, and the mutation would otherwise retry the refused token.
        save.mutate({
          content: docText(view.state),
          token: conflict.currentChecksum,
        });
      }
    },
    onConflictTakeServer: () => {
      if (!conflict) {
        return;
      }
      // Mine is not discarded, it becomes the draft - the same store a crash
      // or a closed tab would have used, so it survives the buffer being
      // overwritten below.
      snapshotDraft(buffer);
      replaceWith(conflict.currentContent);
      setChecksum(conflict.currentChecksum);
      setSavedText(conflict.currentContent);
      setConflict(null);
    },
    offeredDraft,
    restoreDraft: () => {
      if (offeredDraft) {
        replaceWith(offeredDraft.content);
        setOfferedDraft(null);
      }
    },
    discardDraft: () => {
      clearDraft(draftUser, draftDomain, draftSlot);
      setOfferedDraft(null);
    },
    snapshotDraft: () => {
      snapshotDraft(buffer);
    },
    replaceWith,
    noteSaved: () => {
      // Collab only: the Saved control landed, so the current buffer is the
      // saved text and the draft has done its job.
      clearDraft(draftUser, draftDomain, draftSlot);
      setSavedText(buffer);
    },
  };
}
