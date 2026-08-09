/**
 * The editor: the detail read handed to a CodeMirror buffer, saved back with
 * the If-Match token of the version it is based on.
 *
 * The buffer IS the file - frontmatter included - and nothing between the
 * read and the PUT reparses it. A save answers with the detail read of what
 * landed, at its permalink AFTER the write: an author who renamed the engram
 * through its frontmatter is followed to the new address rather than left
 * editing a page that now 404s.
 */

import type { Extension } from "@codemirror/state";
import { Compartment, EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { keymap } from "@codemirror/view";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";

import { problemDetail } from "../api/client";
import type { EngramDetail } from "../api/engram";
import { engramDetailKey, fetchEngramDetail } from "../api/engram";
import type { SaveConflict } from "../api/writes";
import { conflictOf, saveEngram } from "../api/writes";
import { useAuth } from "../auth/AuthContext";
import CmEditor from "../editor/CmEditor";
import { ConflictDialog } from "../editor/ConflictDialog";
import {
  clearDraft,
  DRAFT_DEBOUNCE_MS,
  readDraft,
  writeDraft,
} from "../editor/drafts";
import { livePreview } from "../editor/preview";
import {
  baseExtensions,
  buildEditorState,
  docText,
  lineSeparatorFor,
} from "../editor/setup";
import { editRoute, engramRoute } from "../paths";
import { useTheme } from "../theme/context";
import NotFound from "./NotFound";

interface Notice {
  kind: "problem" | "done";
  text: string;
}

/** The DOM event the buffer's save binding raises on the editor's own node. */
const SAVE_EVENT = "crystalline:save";

/** The buffer's accessible name - shared with the state a conflict rebuilds. */
const ARIA_LABEL = "Engram source";

/**
 * Mod-S inside the buffer, as an extension fixed at module level.
 *
 * It asks for a save rather than performing one: a keymap is built once and
 * lives as long as the view, so a handler that closed over the component's
 * save would be closing over the first render's copy of it forever. The event
 * it raises is listened for beside the view, where the current save is in
 * scope, and CodeMirror still owns the key so the browser never gets its own
 * save dialog.
 */
const saveKeymap = keymap.of([
  {
    key: "Mod-s",
    preventDefault: true,
    run: (view) => {
      view.dom.dispatchEvent(new CustomEvent(SAVE_EVENT));
      return true;
    },
  },
]);

/**
 * What the preview compartment holds, in the one place it is ever spelled.
 *
 * Three sites configure that compartment - the mount extensions, the Raw
 * toggle and the buffer rebuild in `replaceBuffer` - and a decoration layer
 * added to only two of them would vanish on whichever path was missed. Later
 * layers are appended to this array and reach all three at once.
 */
function previewConfig(off: boolean): Extension {
  return off ? [] : livePreview();
}

/** Preview is on when the editor opens. */
const RAW_AT_MOUNT = false;

/**
 * Replace the whole buffer with `content`, preserving whichever line ending
 * `content` actually uses - the mechanism both "take the server version"
 * (after a conflict) and "restore draft" share, since both swap in text that
 * was never typed into this session's own state.
 *
 * A plain `view.dispatch` splits an inserted string using the STATE's
 * existing line separator (`ChangeSet.of` reads
 * `state.facet(EditorState.lineSeparator)`), not the one the string itself
 * uses, so swapping a CRLF-mounted buffer for LF content - or the reverse -
 * through a dispatch alone collapses the result onto one line rather than
 * splitting it correctly. When the separators agree, the dispatch is fine
 * and cheaper; when they don't, the state is rebuilt fresh with `content`'s
 * own separator via `view.setState`, in place on the same view rather than a
 * full component remount, so the rest of the screen's state - `notice`,
 * `offeredDraft`, `conflict` and the rest - survives untouched.
 *
 * `setState` does not run the transaction pipeline, so the rebuilt state's
 * own doc-changed subscription never fires for the swap itself:
 * `onDocChanged` is called directly afterward so the caller's buffer state
 * reflects the swap right away.
 *
 * `preview` travels in because `setState` replaces the whole configuration:
 * the decoration compartment has to be rebuilt into the new state at whatever
 * setting the toggle is currently on, or a separator-changing swap would
 * silently drop live preview and leave the toggle lying about it.
 */
function replaceBuffer(
  view: EditorView,
  content: string,
  dark: boolean,
  preview: Extension,
  onDocChanged: (doc: string) => void,
): void {
  const mountedSeparator = view.state.facet(EditorState.lineSeparator) ?? "\n";
  const nextSeparator = content.includes("\r\n") ? "\r\n" : "\n";
  if (nextSeparator === mountedSeparator) {
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: content },
    });
    return;
  }
  view.setState(
    buildEditorState(
      content,
      [
        ...lineSeparatorFor(content),
        ...baseExtensions(dark),
        saveKeymap,
        preview,
      ],
      ARIA_LABEL,
      onDocChanged,
    ),
  );
  onDocChanged(content);
}

export default function EngramEditor() {
  const params = useParams();
  const domain = params.domain ?? "";
  const permalink = params["*"] ?? "";
  const { capabilities } = useAuth();

  const detail = useQuery({
    queryKey: engramDetailKey(domain, permalink),
    queryFn: () => fetchEngramDetail(domain, permalink),
    enabled: capabilities.canWrite,
  });

  if (!capabilities.canWrite) {
    return <NotFound />;
  }
  if (detail.error) {
    return (
      <p
        role="alert"
        className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
      >
        {problemDetail(detail.error)}
      </p>
    );
  }
  if (!detail.data) {
    return (
      <div role="status" aria-busy="true" aria-label="Loading the editor">
        <div aria-hidden="true" className="flex animate-pulse flex-col gap-2">
          {[0, 1, 2, 3].map((row) => (
            <div
              key={row}
              className="h-6 rounded bg-slate-100 dark:bg-slate-800"
            />
          ))}
        </div>
      </div>
    );
  }
  // Keyed by address: a different engram is a different editing session.
  return <EditorSurface key={`${domain}/${permalink}`} engram={detail.data} />;
}

function EditorSurface({ engram }: { engram: EngramDetail }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { resolved } = useTheme();
  const { user } = useAuth();
  // Anonymous can never reach this screen (`canWrite` gates it above); the
  // fallback only satisfies the types.
  const account = user?.name ?? "anonymous";
  const viewRef = useRef<EditorView | null>(null);
  /**
   * The decoration layer's on/off switch. Raw mode is not a second buffer or
   * a different document, it is this compartment reconfigured to hold nothing
   * - there is no second copy of the text to desync. Later layers append
   * their extensions inside the same compartment call, so the one toggle
   * turns the whole read-model off.
   *
   * State rather than a ref: the compartment is read while rendering the
   * mount extensions, and it is allocated once by the lazy initializer and
   * never set again.
   */
  const [preview] = useState(() => new Compartment());
  const [raw, setRaw] = useState(RAW_AT_MOUNT);
  // What the server holds, moved forward on every successful save.
  const [checksum, setChecksum] = useState(engram.checksum ?? "");
  const [savedText, setSavedText] = useState(engram.content);
  const [buffer, setBuffer] = useState(engram.content);
  const [notice, setNotice] = useState<Notice | null>(null);
  // The 412 view: set on a stale save, cleared by every one of its exits.
  const [conflict, setConflict] = useState<SaveConflict | null>(null);
  const dirty = buffer !== savedText;
  // A browser-stored draft newer than what the server sent, read once per
  // mount and offered through the recovery banner below.
  const [offeredDraft, setOfferedDraft] = useState(() => {
    const stored = readDraft(account, engram.domain, engram.permalink);
    return stored !== null && stored.content !== engram.content ? stored : null;
  });

  const save = useMutation({
    // The token travels with the content rather than being read from
    // `checksum` inside the mutation: a conflict's overwrite moves the
    // checksum state and fires the retry in the same handler, and a mutation
    // that read `checksum` from its closure would still see the pre-update
    // value on that first tick.
    mutationFn: ({ content, token }: { content: string; token: string }) =>
      saveEngram(engram.domain, engram.permalink, content, token),
    onSuccess: (saved) => {
      clearDraft(account, engram.domain, engram.permalink);
      setChecksum(saved.checksum ?? "");
      setSavedText(saved.content);
      setNotice({ kind: "done", text: "Saved" });
      queryClient.setQueryData(
        engramDetailKey(saved.domain, saved.permalink),
        saved,
      );
      if (saved.permalink !== engram.permalink) {
        // The rename receipt: the engram answers at its new address now, and
        // so does this editor.
        void navigate(editRoute(saved.domain, saved.permalink), {
          replace: true,
        });
      }
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
    const view = viewRef.current;
    if (view && !save.isPending) {
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
      writeDraft(account, engram.domain, engram.permalink, {
        content: buffer,
        baseChecksum: checksum,
        savedAt: new Date().toISOString(),
      });
    }, DRAFT_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [account, engram.domain, engram.permalink, buffer, checksum, dirty]);

  // Closing the tab or reloading it loses the draft's safety net too, so it
  // gets its own prompt. In-app navigation does not: the draft already
  // covers it, and the declarative router has no blocker to hook.
  useEffect(() => {
    if (!dirty) {
      return;
    }
    const prompt = (event: BeforeUnloadEvent) => {
      event.preventDefault();
    };
    window.addEventListener("beforeunload", prompt);
    return () => {
      window.removeEventListener("beforeunload", prompt);
    };
  }, [dirty]);

  const extensions = useMemo(
    () => [
      ...lineSeparatorFor(engram.content),
      ...baseExtensions(resolved === "dark"),
      saveKeymap,
      // Every later flip goes through the compartment rather than through
      // this array, which is read once at mount.
      preview.of(previewConfig(RAW_AT_MOUNT)),
    ],
    // Read once: `CmEditor` snapshots the extensions at mount, so a later
    // theme change reaches the buffer through a remount rather than through
    // this array.
    [engram.content, resolved, preview],
  );

  return (
    <div className="flex flex-col gap-4">
      <header className="flex flex-wrap items-baseline justify-between gap-3">
        <div className="flex flex-wrap items-baseline gap-3">
          <h1 className="text-xl font-semibold">Editing {engram.title}</h1>
          <span className="font-mono text-xs text-slate-500 dark:text-slate-400">
            {engram.permalink}
          </span>
        </div>
        <div className="flex items-center gap-2">
          {notice && (
            <p
              role={notice.kind === "problem" ? "alert" : "status"}
              className={
                notice.kind === "problem"
                  ? "rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
                  : "text-sm text-slate-500 dark:text-slate-400"
              }
            >
              {notice.text}
            </p>
          )}
          {dirty && !notice && (
            <p className="text-sm text-slate-500 dark:text-slate-400">
              Unsaved changes
            </p>
          )}
          {/*
            A toggle, so the label names the thing being switched and
            `aria-pressed` carries the state. A button whose text flipped to
            "Preview" while announcing itself as pressed would be telling a
            screen reader the opposite of what it shows.
          */}
          <button
            type="button"
            aria-pressed={raw}
            onClick={() => {
              const next = !raw;
              setRaw(next);
              viewRef.current?.dispatch({
                effects: preview.reconfigure(previewConfig(next)),
              });
            }}
            className={
              raw
                ? "rounded border border-sky-500 bg-sky-50 px-3 py-1 text-sm text-sky-800 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:bg-sky-950 dark:text-sky-200"
                : "rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
            }
          >
            Raw
          </button>
          <button
            type="button"
            onClick={requestSave}
            disabled={save.isPending}
            className="rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none disabled:opacity-50 dark:border-slate-700 dark:hover:bg-slate-800"
          >
            Save
          </button>
          <Link
            to={engramRoute(engram.domain, engram.permalink)}
            className="rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
          >
            Done
          </Link>
        </div>
      </header>
      {offeredDraft && (
        <aside
          role="note"
          className="flex flex-wrap items-baseline gap-3 rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-100"
        >
          <span>
            An unsaved draft of this engram from{" "}
            {offeredDraft.savedAt || "an earlier session"} is on this browser.
          </span>
          <button
            type="button"
            className="underline underline-offset-2 hover:no-underline"
            onClick={() => {
              const view = viewRef.current;
              if (view) {
                replaceBuffer(
                  view,
                  offeredDraft.content,
                  resolved === "dark",
                  preview.of(previewConfig(raw)),
                  setBuffer,
                );
              }
              setOfferedDraft(null);
            }}
          >
            Restore draft
          </button>
          <button
            type="button"
            className="underline underline-offset-2 hover:no-underline"
            onClick={() => {
              clearDraft(account, engram.domain, engram.permalink);
              setOfferedDraft(null);
            }}
          >
            Discard draft
          </button>
        </aside>
      )}
      <div className="rounded border border-slate-200 dark:border-slate-800">
        <CmEditor
          initialDoc={engram.content}
          extensions={extensions}
          ariaLabel={ARIA_LABEL}
          onReady={(view) => {
            viewRef.current = view;
          }}
          onDocChanged={setBuffer}
        />
      </div>
      {conflict && (
        <ConflictDialog
          conflict={conflict}
          mine={buffer}
          onClose={() => {
            setConflict(null);
          }}
          onOverwrite={() => {
            setChecksum(conflict.currentChecksum);
            setConflict(null);
            const view = viewRef.current;
            if (view) {
              // The explicit token, not `checksum`: the state update above
              // has not landed yet on this tick, and the mutation would
              // otherwise retry with the token that just got refused.
              save.mutate({
                content: docText(view.state),
                token: conflict.currentChecksum,
              });
            }
          }}
          onTakeServer={() => {
            // Mine is not discarded, it becomes the draft - the same store a
            // crash or a closed tab would have used, so it survives the
            // buffer being overwritten below.
            writeDraft(account, engram.domain, engram.permalink, {
              content: buffer,
              baseChecksum: checksum,
              savedAt: new Date().toISOString(),
            });
            const view = viewRef.current;
            if (view) {
              replaceBuffer(
                view,
                conflict.currentContent,
                resolved === "dark",
                preview.of(previewConfig(raw)),
                setBuffer,
              );
            }
            setChecksum(conflict.currentChecksum);
            setSavedText(conflict.currentContent);
            setConflict(null);
          }}
        />
      )}
    </div>
  );
}
