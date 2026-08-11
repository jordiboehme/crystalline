/**
 * The MANIFEST editor: `EngramEditor`'s shape with the engram-specific panels
 * left out.
 *
 * A MANIFEST has no permalink of its own to rename through, no wikilinks to
 * resolve and no frontmatter form beside it - it is plain markdown with
 * frontmatter like any engram, but nothing here parses it into fields. What
 * survives the trim is exactly what both editors need to agree on, and that
 * part is not copied any more: `useEditorSession` holds it once for both. The
 * buffer is the file's own bytes, saved back with the If-Match token of the
 * version it was read from, gated by the same dry-run findings and landing in
 * the same 412 conflict view on a stale save.
 *
 * The extension set stays static rather than built from a shared options
 * object: there is no preview layer to toggle and no resolver to reconfigure,
 * so `[...lineSeparatorFor, ...baseExtensions, saveKeymap, RAW_MONO]` is the
 * whole of it, spelled once at mount and again wherever a swap rebuilds the
 * buffer.
 */

import type { Extension } from "@codemirror/state";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useParams } from "react-router";

import { problemDetail } from "../api/client";
import type { ManifestDetail } from "../api/domain";
import {
  fetchManifestDetail,
  manifestDetailKey,
  manifestKey,
  saveManifest,
} from "../api/domain";
import { useAuth } from "../auth/AuthContext";
import { Skeleton } from "../components/Skeleton";
import CmEditor from "../editor/CmEditor";
import { ConflictDialog } from "../editor/ConflictDialog";
import { FindingsPanel, jumpToLine } from "../editor/FindingsPanel";
import { RAW_MONO, baseExtensions, lineSeparatorFor } from "../editor/setup";
import { saveKeymap, useEditorSession } from "../editor/useEditorSession";
import { manifestRoute } from "../paths";
import { useTheme } from "../theme/context";
import NotFound from "./NotFound";

/**
 * The MANIFEST's stand-in permalink in the draft store. A domain's MANIFEST
 * has no permalink of its own - `readDraft`/`writeDraft` key on one anyway,
 * so this is the fixed second half of that key rather than a value read off
 * anything: the engine's own name for the file is exactly this word.
 */
const MANIFEST_DRAFT_SLOT = "MANIFEST";

/** The file this buffer validates as, in the engine's own vocabulary. */
const MANIFEST_PATH = "MANIFEST.md";

/** The buffer's accessible name - shared with the state a conflict rebuilds. */
const ARIA_LABEL = "MANIFEST source";

/**
 * The whole extension set of this buffer, spelled once.
 *
 * `RAW_MONO` is a fixture here rather than a mode: the shared theme sets
 * editor prose proportional for the surfaces that draw a live preview over
 * it, and this one draws none. It is the source of a file and nothing else,
 * so mono is its permanent face - the same face the engram editor's Raw
 * toggle switches into, carried statically because there is no toggle and no
 * preview compartment to carry it. One function, so the mount and every
 * buffer swap agree.
 */
function extensionsFor(content: string, dark: boolean): Extension[] {
  return [
    ...lineSeparatorFor(content),
    ...baseExtensions(dark),
    saveKeymap,
    RAW_MONO,
  ];
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
    return <Skeleton label="Loading the editor" rows={4} />;
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

  // The shared shell: buffer, checksum, dirty state, the dry-run gate,
  // drafts, the Mod-S save and the 412 flow. What this screen adds is the
  // transport below and the layout under it.
  const session = useEditorSession({
    initialContent: manifest.markdown,
    initialChecksum: manifest.checksum ?? "",
    draftUser: account,
    draftDomain: domain,
    draftSlot: MANIFEST_DRAFT_SLOT,
    validateDomain: domain,
    validatePath: MANIFEST_PATH,
    save: async (content, token) => {
      const saved = await saveManifest(domain, content, token);
      // The plain read and the detail read are two cache entries over one
      // file: both are stale the moment this lands.
      void queryClient.invalidateQueries({ queryKey: manifestKey(domain) });
      void queryClient.invalidateQueries({
        queryKey: manifestDetailKey(domain),
      });
      return { content: saved.markdown, checksum: saved.checksum ?? "" };
    },
    extensionsFor: (content) => extensionsFor(content, dark),
    ariaLabel: ARIA_LABEL,
  });

  return (
    <div className="flex flex-col gap-4">
      <header className="flex flex-wrap items-baseline justify-between gap-3">
        <div className="flex flex-wrap items-baseline gap-3">
          <h1 className="text-xl font-semibold">Editing {domain} MANIFEST</h1>
        </div>
        <div className="flex items-center gap-2">
          {session.notice && (
            <p
              role={session.notice.kind === "problem" ? "alert" : "status"}
              className={
                session.notice.kind === "problem"
                  ? "rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
                  : "text-sm text-slate-500 dark:text-slate-400"
              }
            >
              {session.notice.text}
            </p>
          )}
          {session.dirty && !session.notice && (
            <p className="text-sm text-slate-500 dark:text-slate-400">
              Unsaved changes
            </p>
          )}
          <button
            type="button"
            onClick={session.requestSave}
            disabled={session.saving || session.hardErrors > 0}
            className="rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none disabled:opacity-50 dark:border-slate-700 dark:hover:bg-slate-800"
          >
            Save
          </button>
          <Link
            to={manifestRoute(domain)}
            className="rounded border border-slate-300 px-3 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
          >
            Done
          </Link>
        </div>
      </header>
      {session.hardErrors > 0 && (
        <p role="alert" className="text-sm text-red-800 dark:text-red-200">
          {String(session.hardErrors)} hard{" "}
          {session.hardErrors === 1 ? "error" : "errors"} block saving; see
          Findings.
        </p>
      )}
      {session.offeredDraft && (
        <aside
          role="note"
          className="flex flex-wrap items-baseline gap-3 rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-100"
        >
          <span>
            An unsaved draft of this MANIFEST from{" "}
            {session.offeredDraft.savedAt || "an earlier session"} is on this
            browser.
          </span>
          <button
            type="button"
            className="underline underline-offset-2 hover:no-underline"
            onClick={session.restoreDraft}
          >
            Restore draft
          </button>
          <button
            type="button"
            className="underline underline-offset-2 hover:no-underline"
            onClick={session.discardDraft}
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
            onReady={session.onReady}
            onDocChanged={session.setBuffer}
          />
        </div>
        <aside className="flex flex-col gap-4">
          <FindingsPanel
            report={session.report}
            pending={session.checking}
            unavailable={session.validationUnavailable}
            onJump={(line) => {
              const view = session.viewRef.current;
              if (view) {
                jumpToLine(view, line);
              }
            }}
          />
        </aside>
      </div>
      {session.conflict && (
        <ConflictDialog
          conflict={session.conflict}
          mine={session.buffer}
          onClose={session.onConflictClose}
          onOverwrite={session.onConflictOverwrite}
          onTakeServer={session.onConflictTakeServer}
        />
      )}
    </div>
  );
}
