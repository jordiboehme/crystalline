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
import { Compartment, EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { keymap } from "@codemirror/view";
import type { QueryClient } from "@tanstack/react-query";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";

import { problemDetail } from "../api/client";
import type { EngramDetail } from "../api/engram";
import { engramDetailKey, fetchEngramDetail } from "../api/engram";
import { NEIGHBORHOOD_DEPTH, fetchGraph, graphKey } from "../api/graph";
import type { ValidateResponse } from "../api/model";
import type { Vocabulary } from "../api/vocabulary";
import { fetchVocabulary, fullVocabularyKey } from "../api/vocabulary";
import type { SaveConflict } from "../api/writes";
import {
  conflictOf,
  saveEngram,
  validateDocument,
  validateKey,
} from "../api/writes";
import { useAuth } from "../auth/AuthContext";
import CmEditor from "../editor/CmEditor";
import { ConflictDialog } from "../editor/ConflictDialog";
import {
  crystallineCompletions,
  crystallineLines,
} from "../editor/crystallineLines";
import {
  clearDraft,
  DRAFT_DEBOUNCE_MS,
  readDraft,
  writeDraft,
} from "../editor/drafts";
import { fencePreviews } from "../editor/fencePreviews";
import { FindingsPanel, jumpToLine } from "../editor/FindingsPanel";
import { FrontmatterForm } from "../editor/FrontmatterForm";
import { livePreview } from "../editor/preview";
import {
  baseExtensions,
  buildEditorState,
  docText,
  lineSeparatorFor,
} from "../editor/setup";
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

interface Notice {
  kind: "problem" | "done";
  text: string;
}

/** The DOM event the buffer's save binding raises on the editor's own node. */
const SAVE_EVENT = "crystalline:save";

/** The buffer's accessible name - shared with the state a conflict rebuilds. */
const ARIA_LABEL = "Engram source";

/**
 * How long a pause in typing waits before a dry-run validate fires - one
 * request per pause, never one per keystroke. Exported for the tests.
 */
export const VALIDATE_DEBOUNCE_MS = 500;

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
 * rebuild in `replaceBuffer` - and `setState` replaces the configuration
 * entirely, so anything listed in only one of them silently disappears the
 * first time a swap rebuilds the buffer.
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
 * `extensionsFor` travels in because `setState` replaces the whole
 * configuration: every layer has to be rebuilt into the new state as it
 * stands right now - the decoration compartment at whatever setting the
 * toggle is on, the resolver at whatever the graph has answered - or a
 * separator-changing swap would silently drop them and leave the toggle lying
 * about it. It takes the content because a rebuilt state's line separator
 * comes from the text being swapped in, not from the one being replaced.
 */
function replaceBuffer(
  view: EditorView,
  content: string,
  extensionsFor: (content: string) => Extension[],
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
    buildEditorState(content, extensionsFor(content), ARIA_LABEL, onDocChanged),
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
  // The resolver reaches the buffer through its compartment rather than
  // through a remount: the chips redraw, the text and the history stay.
  useEffect(() => {
    viewRef.current?.dispatch({
      effects: resolverBox.reconfigure(wikilinkResolverFacet.of(resolver)),
    });
  }, [resolver, resolverBox]);

  // The dry run: a pause in typing, not every keystroke, is what fires it -
  // `debouncedBuffer` only catches up with `buffer` once typing has paused
  // for `VALIDATE_DEBOUNCE_MS`, and it is that settled value which becomes
  // part of the query key, mirroring how the search screen debounces typed
  // text into the value a query actually keys on.
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
    queryKey: validateKey(engram.domain, engram.path, debouncedBuffer),
    queryFn: () =>
      validateDocument({
        content: debouncedBuffer,
        domain: engram.domain,
        ...(engram.path !== null ? { path: engram.path } : {}),
      }),
  });
  // The server does not re-check these rule families on save, so the gate
  // below is the only enforcement there is - it must never blink open just
  // because a fresh keystroke changed the query key and `validation.data`
  // has nothing for the new key yet. `lastLanded` tracks the most recent
  // verdict that actually arrived, independently of whichever key is
  // currently in flight, and `report` falls back to it whenever the live
  // query has nothing of its own.
  //
  // Tracked beside the query with a plain `useState`, updated during render
  // rather than through `placeholderData: keepPreviousData` - react query's
  // own answer to this exact problem - for two reasons: that import lives in
  // a module this screen's lazy route already shares with several
  // eagerly-loaded ones, and pulling in one more named export from it grew
  // the ENTRY bundle by a few dozen bytes even though the code that calls it
  // never leaves the lazy chunk; and updating it from a `useEffect` (the
  // first shape this took) is exactly the "adjust state when a prop
  // changes" case React's own docs say to do in the render body instead -
  // an effect-scheduled update here would let one extra render slip through
  // on the old, wrong verdict before the effect ever ran.
  // `seen` is what makes that safe: comparing against the previous render's
  // own `validation.data`/`isError` is what stops this from setting state on
  // every render forever, the same guard the docs' own example keeps.
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
      // A settled failure, not a still-in-flight revalidation: nothing
      // kept-previous survives a genuine refusal, so a transport failure
      // reopens the gate exactly as it always has - see the comment on
      // `report` below.
      setLastLanded(null);
    }
  }
  // A transport failure never blocks writing - the save path has its own
  // errors - so a failed dry run reads as "nothing to report" rather than as
  // a hard error: `report` falls back to null (through `lastLanded`, cleared
  // above), and `hardErrors` falls back to zero right behind it. This is why
  // a settled failure is allowed to drop the gate while a still-pending
  // revalidation, held by `lastLanded` not yet being cleared, is not.
  const report = validation.data ?? lastLanded;
  const hardErrors = report?.errors ?? 0;
  // True from the moment a keystroke outruns the last check that landed,
  // not only while a request is actually in flight - a stale clean report
  // never gets to look current while newer, unverified text sits above it.
  const checking = validation.isFetching || buffer !== debouncedBuffer;
  // The dry run failed outright and there is nothing kept-previous to show
  // for it - a refused or unreachable `/validate`, not an ordinary pause in
  // typing. Distinct from "Checking" so the panel never promises a verdict
  // is still coming when it plainly is not; saves stay allowed regardless,
  // since `hardErrors` is already 0 whenever there is no report to read one
  // from.
  const validationUnavailable =
    report === null && validation.isError && !checking;

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
            onClick={requestSave}
            disabled={save.isPending || hardErrors > 0}
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
                  extensionsFor,
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
              viewRef.current = ready;
              setView(ready);
            }}
            onDocChanged={setBuffer}
          />
        </div>
        <aside className="flex flex-col gap-4">
          <FrontmatterForm
            doc={buffer}
            view={view}
            vocabulary={vocabulary.data ?? null}
          />
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
                extensionsFor,
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
