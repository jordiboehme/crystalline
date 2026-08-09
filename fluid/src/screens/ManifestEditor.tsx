/**
 * The MANIFEST editor: `EngramEditor`'s shape with the engram-specific panels
 * left out.
 *
 * A MANIFEST has no permalink of its own to rename through, no wikilinks to
 * resolve and no frontmatter form beside it - it is plain markdown with
 * frontmatter like any engram, but nothing here parses it into fields. What
 * survives the trim is exactly what both editors need to agree on: the
 * buffer is the file's own bytes, saved back with the If-Match token of the
 * version it was read from, gated by the same dry-run findings and landing in
 * the same 412 conflict view on a stale save.
 *
 * The extension set stays static rather than built from a shared options
 * object: there is no preview layer to toggle and no resolver to reconfigure,
 * so `[...lineSeparatorFor, ...baseExtensions, saveKeymap]` is the whole of
 * it, spelled once at mount and again wherever a swap rebuilds the buffer.
 */

import type { Extension } from "@codemirror/state";
import { EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { keymap } from "@codemirror/view";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { Link, useParams } from "react-router";

import { problemDetail } from "../api/client";
import type { ManifestDetail } from "../api/domain";
import {
  fetchManifestDetail,
  manifestDetailKey,
  manifestKey,
  saveManifest,
} from "../api/domain";
import type { ValidateResponse } from "../api/model";
import type { SaveConflict } from "../api/writes";
import { conflictOf, validateDocument, validateKey } from "../api/writes";
import { useAuth } from "../auth/AuthContext";
import CmEditor from "../editor/CmEditor";
import { ConflictDialog } from "../editor/ConflictDialog";
import {
  clearDraft,
  DRAFT_DEBOUNCE_MS,
  readDraft,
  writeDraft,
} from "../editor/drafts";
import { FindingsPanel, jumpToLine } from "../editor/FindingsPanel";
import {
  baseExtensions,
  buildEditorState,
  docText,
  lineSeparatorFor,
} from "../editor/setup";
import { manifestRoute } from "../paths";
import { useTheme } from "../theme/context";
import NotFound from "./NotFound";

interface Notice {
  kind: "problem" | "done";
  text: string;
}

/**
 * The MANIFEST's stand-in permalink in the draft store. A domain's MANIFEST
 * has no permalink of its own - `readDraft`/`writeDraft` key on one anyway,
 * so this is the fixed second half of that key rather than a value read off
 * anything: the engine's own name for the file is exactly this word.
 */
const MANIFEST_DRAFT_SLOT = "MANIFEST";

/** The DOM event the buffer's save binding raises on the editor's own node. */
const SAVE_EVENT = "crystalline:manifest-save";

/** The buffer's accessible name - shared with the state a conflict rebuilds. */
const ARIA_LABEL = "MANIFEST source";

/** How long a pause in typing waits before a dry-run validate fires. */
const VALIDATE_DEBOUNCE_MS = 500;

/**
 * Mod-S inside the buffer. See `EngramEditor`'s own copy of this pattern for
 * why it asks for a save rather than performing one directly: a keymap is
 * fixed at module level and would otherwise close over the first render's
 * save forever.
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

/** The whole extension set of this buffer, spelled once. */
function extensionsFor(content: string, dark: boolean): Extension[] {
  return [...lineSeparatorFor(content), ...baseExtensions(dark), saveKeymap];
}

/**
 * Replace the whole buffer with `content`, preserving whichever line ending
 * it actually uses. See `EngramEditor.tsx`'s `replaceBuffer` for the full
 * reasoning: a plain dispatch splits inserted text on the STATE's existing
 * separator rather than the incoming text's own, so a swap across CRLF and LF
 * has to rebuild the state fresh instead.
 */
function replaceBuffer(
  view: EditorView,
  content: string,
  dark: boolean,
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
      extensionsFor(content, dark),
      ARIA_LABEL,
      onDocChanged,
    ),
  );
  onDocChanged(content);
}

export default function ManifestEditor() {
  const { domain = "" } = useParams();
  const { capabilities } = useAuth();

  const detail = useQuery({
    queryKey: manifestDetailKey(domain),
    queryFn: () => fetchManifestDetail(domain),
    enabled: capabilities.canAdminister,
  });

  if (!capabilities.canAdminister) {
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
  // Keyed by domain: a different domain's MANIFEST is a different session.
  return <EditorSurface key={domain} domain={domain} manifest={detail.data} />;
}

function EditorSurface({
  domain,
  manifest,
}: {
  domain: string;
  manifest: ManifestDetail;
}) {
  const queryClient = useQueryClient();
  const { resolved } = useTheme();
  const { user } = useAuth();
  // Anonymous can never reach this screen (`canAdminister` gates it above);
  // the fallback only satisfies the types.
  const account = user?.name ?? "anonymous";
  const dark = resolved === "dark";
  const viewRef = useRef<EditorView | null>(null);
  const [checksum, setChecksum] = useState(manifest.checksum ?? "");
  const [savedText, setSavedText] = useState(manifest.markdown);
  const [buffer, setBuffer] = useState(manifest.markdown);
  const [notice, setNotice] = useState<Notice | null>(null);
  // The 412 view: set on a stale save, cleared by every one of its exits.
  const [conflict, setConflict] = useState<SaveConflict | null>(null);
  const dirty = buffer !== savedText;
  // A browser-stored draft newer than what the server sent, read once per
  // mount and offered through the recovery banner below.
  const [offeredDraft, setOfferedDraft] = useState(() => {
    const stored = readDraft(account, domain, MANIFEST_DRAFT_SLOT);
    return stored !== null && stored.content !== manifest.markdown
      ? stored
      : null;
  });

  // The dry run: a pause in typing, not every keystroke, is what fires it.
  const [debouncedBuffer, setDebouncedBuffer] = useState(buffer);
  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedBuffer(buffer);
    }, VALIDATE_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [buffer]);
  const validation = useQuery({
    queryKey: validateKey(domain, "MANIFEST.md", debouncedBuffer),
    queryFn: () =>
      validateDocument({
        content: debouncedBuffer,
        domain,
        path: "MANIFEST.md",
      }),
  });
  // The server does not re-check these rule families on save, so this gate is
  // the only enforcement there is - see `EngramEditor.tsx`'s own copy of this
  // pattern for the full reasoning behind tracking the last landed verdict
  // rather than trusting `validation.data` alone.
  const [lastLanded, setLastLanded] = useState<ValidateResponse | null>(null);
  const [seen, setSeen] = useState({
    data: validation.data,
    isError: validation.isError,
  });
  if (validation.data !== seen.data || validation.isError !== seen.isError) {
    setSeen({ data: validation.data, isError: validation.isError });
    if (validation.data !== undefined) {
      setLastLanded(validation.data);
    } else if (validation.isError) {
      setLastLanded(null);
    }
  }
  const report = validation.data ?? lastLanded;
  const hardErrors = report?.errors ?? 0;
  const checking = validation.isFetching || buffer !== debouncedBuffer;
  const validationUnavailable =
    report === null && validation.isError && !checking;

  const save = useMutation({
    // The token travels with the content rather than being read from
    // `checksum` inside the mutation: a conflict's overwrite moves the
    // checksum state and fires the retry in the same handler.
    mutationFn: ({ content, token }: { content: string; token: string }) =>
      saveManifest(domain, content, token),
    onSuccess: (saved) => {
      clearDraft(account, domain, MANIFEST_DRAFT_SLOT);
      setChecksum(saved.checksum ?? "");
      setSavedText(saved.markdown);
      setNotice({ kind: "done", text: "Saved" });
      void queryClient.invalidateQueries({ queryKey: manifestKey(domain) });
      void queryClient.invalidateQueries({
        queryKey: manifestDetailKey(domain),
      });
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
    if (view && !save.isPending && hardErrors === 0) {
      setNotice(null);
      save.mutate({ content: docText(view.state), token: checksum });
    }
  };
  // The keymap's request, answered here where the current save is in scope.
  // Re-bound after every render: what the binding must reach is the latest
  // save, not the one that existed when the view was created.
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
      writeDraft(account, domain, MANIFEST_DRAFT_SLOT, {
        content: buffer,
        baseChecksum: checksum,
        savedAt: new Date().toISOString(),
      });
    }, DRAFT_DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [account, domain, buffer, checksum, dirty]);

  // Closing the tab or reloading it loses the draft's safety net too, so it
  // gets its own prompt.
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

  return (
    <div className="flex flex-col gap-4">
      <header className="flex flex-wrap items-baseline justify-between gap-3">
        <div className="flex flex-wrap items-baseline gap-3">
          <h1 className="text-xl font-semibold">Editing {domain} MANIFEST</h1>
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
          <button
            type="button"
            onClick={requestSave}
            disabled={save.isPending || hardErrors > 0}
            className="rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none disabled:opacity-50 dark:border-slate-700 dark:hover:bg-slate-800"
          >
            Save
          </button>
          <Link
            to={manifestRoute(domain)}
            className="rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
          >
            Done
          </Link>
        </div>
      </header>
      {hardErrors > 0 && (
        <p role="alert" className="text-sm text-red-800 dark:text-red-200">
          {String(hardErrors)} hard {hardErrors === 1 ? "error" : "errors"}{" "}
          block saving; see Findings.
        </p>
      )}
      {offeredDraft && (
        <aside
          role="note"
          className="flex flex-wrap items-baseline gap-3 rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-100"
        >
          <span>
            An unsaved draft of this MANIFEST from{" "}
            {offeredDraft.savedAt || "an earlier session"} is on this browser.
          </span>
          <button
            type="button"
            className="underline underline-offset-2 hover:no-underline"
            onClick={() => {
              const view = viewRef.current;
              if (view) {
                replaceBuffer(view, offeredDraft.content, dark, setBuffer);
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
              clearDraft(account, domain, MANIFEST_DRAFT_SLOT);
              setOfferedDraft(null);
            }}
          >
            Discard draft
          </button>
        </aside>
      )}
      <div className="grid gap-8 lg:grid-cols-[minmax(0,1fr)_18rem]">
        <div className="rounded border border-slate-200 dark:border-slate-800">
          <CmEditor
            initialDoc={manifest.markdown}
            extensions={extensionsFor(manifest.markdown, dark)}
            ariaLabel={ARIA_LABEL}
            onReady={(ready) => {
              viewRef.current = ready;
            }}
            onDocChanged={setBuffer}
          />
        </div>
        <aside className="flex flex-col gap-4">
          <FindingsPanel
            report={report}
            pending={checking}
            unavailable={validationUnavailable}
            onJump={(line) => {
              const view = viewRef.current;
              if (view) {
                jumpToLine(view, line);
              }
            }}
          />
        </aside>
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
            // crash or a closed tab would have used.
            writeDraft(account, domain, MANIFEST_DRAFT_SLOT, {
              content: buffer,
              baseChecksum: checksum,
              savedAt: new Date().toISOString(),
            });
            const view = viewRef.current;
            if (view) {
              replaceBuffer(view, conflict.currentContent, dark, setBuffer);
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
