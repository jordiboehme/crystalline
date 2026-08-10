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

import { autocompletion } from "@codemirror/autocomplete";
import type { Extension } from "@codemirror/state";
import { Compartment } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type { QueryClient } from "@tanstack/react-query";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";

import { problemDetail } from "../api/client";
import type { EngramDetail } from "../api/engram";
import { engramDetailKey, fetchEngramDetail } from "../api/engram";
import { NEIGHBORHOOD_DEPTH, fetchGraph, graphKey } from "../api/graph";
import type { Vocabulary } from "../api/vocabulary";
import { fetchVocabulary, fullVocabularyKey } from "../api/vocabulary";
import { saveEngram } from "../api/writes";
import { useAuth } from "../auth/AuthContext";
import { Skeleton } from "../components/Skeleton";
import CmEditor from "../editor/CmEditor";
import { ConflictDialog } from "../editor/ConflictDialog";
import {
  crystallineCompletions,
  crystallineLines,
} from "../editor/crystallineLines";
import { fencePreviews } from "../editor/fencePreviews";
import { FindingsPanel, jumpToLine } from "../editor/FindingsPanel";
import { FrontmatterForm } from "../editor/FrontmatterForm";
import { livePreview } from "../editor/preview";
import { baseExtensions, lineSeparatorFor } from "../editor/setup";
import { saveKeymap, useEditorSession } from "../editor/useEditorSession";
import {
  wikilinkChips,
  wikilinkCompletions,
  wikilinkResolverFacet,
} from "../editor/wikilinkChips";
import { editRoute, engramRoute } from "../paths";
import { useTheme } from "../theme/context";
import type { WikilinkResolver } from "../wikilinks";
import { buildWikilinkResolver } from "../wikilinks";
import NotFound from "./NotFound";

/** The buffer's accessible name - shared with the state a conflict rebuilds. */
const ARIA_LABEL = "Engram source";

/**
 * What the preview compartment holds, in the one place it is ever spelled.
 *
 * Two sites configure that compartment - `surfaceExtensions`, which every
 * state this screen builds goes through, and the Raw toggle - and a decoration
 * layer added to only one of them would vanish on whichever path was missed.
 * Later layers are appended to this array and reach both at once.
 */
function previewConfig(off: boolean, dark: boolean): Extension {
  return off
    ? []
    : [livePreview(), wikilinkChips(), crystallineLines(), fencePreviews(dark)];
}

/** Preview is on when the editor opens. */
const RAW_AT_MOUNT = false;

/**
 * How long a detail read stays fresh once it lands in the cache.
 *
 * Zero - TanStack Query's own default - would be right for a screen that
 * only ever reads; this one also writes into the same cache entry from
 * outside a normal fetch, twice: a save seeds the version it just wrote, and
 * a create seeds the engram this editor is about to mount onto before the
 * navigation that mounts it. Either way, an observer subscribing a moment
 * later would otherwise see already-known data as instantly stale and
 * re-request it in the background, a self-inflicted round trip for content
 * this tab watched land. A modest window absorbs that without leaving stale
 * content on screen for long: switching back to the tab still re-checks
 * regardless, since `refetchOnWindowFocus` stays on.
 */
const DETAIL_STALE_MS = 15_000;

/** What a reference resolves to before the two requests behind it land. */
const NO_RESOLUTION: WikilinkResolver = () => null;

/** Everything one buffer on this screen is built from. */
interface SurfaceOptions {
  /** The text the buffer opens with, whose line endings it inherits. */
  content: string;
  /** The cache the completion's title lookup goes through. */
  client: QueryClient;
  dark: boolean;
  /** The engram's own domain, which decides what a completion prefixes. */
  domain: string;
  /** The layer switch the Raw toggle reconfigures. */
  preview: Compartment;
  raw: boolean;
  /** The resolver's own switch, reconfigured when the graph lands. */
  resolverBox: Compartment;
  resolver: WikilinkResolver;
  /**
   * The domain's vocabulary, read fresh on every completion rather than
   * closed over: the fetch behind it lands after the buffer mounts, and a
   * plain value captured here would go on answering `null` forever once it
   * did.
   */
  vocab: () => Vocabulary | null;
}

/**
 * The whole extension set of one buffer, in the one place it is ever spelled.
 *
 * Two sites build a state for this screen - the mount and the wholesale
 * rebuild the session's `replaceWith` performs - and `setState` replaces the
 * configuration entirely, so anything listed in only one of them silently
 * disappears the first time a swap rebuilds the buffer.
 */
function surfaceExtensions(options: SurfaceOptions): Extension[] {
  return [
    ...lineSeparatorFor(options.content),
    ...baseExtensions(options.dark),
    saveKeymap,
    // Typing help rather than preview, so it stays on in raw mode: outside
    // the compartment the Raw toggle empties.
    autocompletion({
      override: [
        wikilinkCompletions(options.domain, options.client),
        crystallineCompletions(options.vocab),
      ],
    }),
    options.resolverBox.of(wikilinkResolverFacet.of(options.resolver)),
    // Every later flip goes through the compartment rather than through this
    // array, which is read once per state.
    options.preview.of(previewConfig(options.raw, options.dark)),
  ];
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
    staleTime: DETAIL_STALE_MS,
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
    return <Skeleton label="Loading the editor" rows={8} />;
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
  /**
   * The same view as state, for the one consumer that needs it while
   * rendering: the frontmatter form dispatches into it. A ref read during
   * render would hand the form `null` forever, because the mount that fills it
   * schedules no re-render of its own.
   */
  const [view, setView] = useState<EditorView | null>(null);
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
  /**
   * The resolver's own switch. It is not part of the preview compartment
   * because it is not a layer: it is the answer the chips inside that layer
   * ask for, and it arrives once the neighborhood request lands rather than
   * at mount.
   */
  const [resolverBox] = useState(() => new Compartment());
  const [raw, setRaw] = useState(RAW_AT_MOUNT);

  // The same pair the engram page reads, under the same cache keys: an author
  // who arrived from that page pays nothing on the wire for the chips.
  const graph = useQuery({
    queryKey: graphKey(engram.domain, engram.permalink, NEIGHBORHOOD_DEPTH),
    queryFn: () => fetchGraph(engram.domain, engram.permalink),
  });
  const resolver = useMemo(
    () => buildWikilinkResolver(engram, graph.data),
    [engram, graph.data],
  );
  // `fullVocabularyKey` rather than `vocabularyKey`: `DomainHome` caches
  // `fetchTags` under the latter, a different shape, and the two landing on
  // one key would mean whichever query resolved second overwrote the other
  // with data it cannot parse.
  const vocabulary = useQuery({
    queryKey: fullVocabularyKey(engram.domain),
    queryFn: () => fetchVocabulary(engram.domain),
  });
  // A ref, not state: `crystallineCompletions` reads this getter fresh on
  // every completion request rather than once at mount, so the vocabulary
  // fetch - which lands after the buffer is already on screen - reaches
  // completions offered later in the session instead of only the ones asked
  // for after this particular render. Written from an effect rather than
  // inline during render, and read through a `useCallback`-stable getter
  // rather than a closure built at each call site: a ref may not be written
  // or handed to a render-time call while React is still rendering.
  const vocabRef = useRef<Vocabulary | null>(null);
  useEffect(() => {
    vocabRef.current = vocabulary.data ?? null;
  }, [vocabulary.data]);
  const readVocab = useCallback(() => vocabRef.current, []);

  /**
   * The extensions a buffer swapped in mid-session is rebuilt with: this
   * render's resolver, this render's Raw setting, and the incoming text's own
   * line separator.
   */
  const extensionsFor = (content: string) =>
    surfaceExtensions({
      content,
      client: queryClient,
      dark: resolved === "dark",
      domain: engram.domain,
      preview,
      raw,
      resolverBox,
      resolver,
      vocab: readVocab,
    });

  // Everything both editors agree on - the buffer's checksum and dirty state,
  // the dry-run gate, drafts, the Mod-S save and the 412 flow - lives in the
  // shared session; what is engram-specific stays here.
  const session = useEditorSession({
    initialContent: engram.content,
    initialChecksum: engram.checksum ?? "",
    draftUser: account,
    draftDomain: engram.domain,
    draftSlot: engram.permalink,
    validateDomain: engram.domain,
    validatePath: engram.path,
    save: async (content, token) => {
      const saved = await saveEngram(
        engram.domain,
        engram.permalink,
        content,
        token,
      );
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
      return { content: saved.content, checksum: saved.checksum ?? "" };
    },
    extensionsFor,
    ariaLabel: ARIA_LABEL,
  });

  // The resolver reaches the buffer through its compartment rather than
  // through a remount: the chips redraw, the text and the history stay.
  useEffect(() => {
    session.viewRef.current?.dispatch({
      effects: resolverBox.reconfigure(wikilinkResolverFacet.of(resolver)),
    });
  }, [resolver, resolverBox, session.viewRef]);

  const extensions = useMemo(
    () =>
      // `readVocab` only ever hands `vocabRef` to `crystallineCompletions`,
      // which stores it in a `CompletionSource` closure CodeMirror calls later
      // from typing, not here: nothing on this call stack reads `.current`
      // during this render, but the checker cannot see through the
      // `autocompletion` and `crystallineCompletions` calls to confirm that.
      // eslint-disable-next-line react-hooks/refs
      surfaceExtensions({
        content: engram.content,
        client: queryClient,
        dark: resolved === "dark",
        domain: engram.domain,
        preview,
        raw: RAW_AT_MOUNT,
        resolverBox,
        // The graph has not landed at mount; the compartment above carries
        // the real resolver in as soon as it does.
        resolver: NO_RESOLUTION,
        vocab: readVocab,
      }),
    // Read once: `CmEditor` snapshots the extensions at mount, so a later
    // theme change reaches the buffer through a remount rather than through
    // this array.
    [
      engram.content,
      engram.domain,
      queryClient,
      resolved,
      preview,
      resolverBox,
      readVocab,
    ],
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
              session.viewRef.current?.dispatch({
                effects: preview.reconfigure(
                  previewConfig(next, resolved === "dark"),
                ),
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
            onClick={session.requestSave}
            disabled={session.saving || session.hardErrors > 0}
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
            An unsaved draft of this engram from{" "}
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
      {/*
        The same two-column shape the read page has, so the metadata sits
        where a reader already expects it. The form is a view over the buffer
        rather than a second place a value lives: it reads `buffer` and writes
        back through ordinary transactions on the view.
      */}
      <div className="grid gap-8 lg:grid-cols-[minmax(0,1fr)_18rem]">
        <div className="rounded border border-slate-200 dark:border-slate-800">
          <CmEditor
            initialDoc={engram.content}
            extensions={extensions}
            ariaLabel={ARIA_LABEL}
            onReady={(ready) => {
              session.onReady(ready);
              setView(ready);
            }}
            onDocChanged={session.setBuffer}
          />
        </div>
        <aside className="flex flex-col gap-4">
          <FrontmatterForm
            doc={session.buffer}
            view={view}
            vocabulary={vocabulary.data ?? null}
          />
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
