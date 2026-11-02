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
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown } from "lucide-react";
import type { ReactNode } from "react";
import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { Link, useNavigate, useParams } from "react-router";
import { yCollab, yUndoManagerKeymap } from "y-codemirror.next";
import type { Awareness } from "y-protocols/awareness";
import * as Y from "yjs";

import { changedVersions } from "../api/assetVersions";
import { problemDetail } from "../api/client";
import { domainTreeKey } from "../api/domain";
import { heldJoin } from "../api/draftLinks";
import { ASSETS_PREFIX, attachmentsKey, listAttachments } from "../api/files";
import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import type { EngramDetail, SimilarEngram } from "../api/engram";
import { engramDetailKey, fetchEngramDetail } from "../api/engram";
import { NEIGHBORHOOD_DEPTH, fetchGraph, graphKey } from "../api/graph";
import type { Vocabulary } from "../api/vocabulary";
import { fetchVocabulary, fullVocabularyKey } from "../api/vocabulary";
import { saveEngram } from "../api/writes";
import { useAuth } from "../auth/AuthContext";
import { useDomainWritable } from "../domainWritable";
import { CollabConflictDialog } from "../collab/CollabConflictDialog";
import { normalizePastedEndings } from "../collab/pastedEndings";
import { cursorFlags } from "../collab/cursorFlags";
import { PresenceChips } from "../collab/PresenceChips";
import type { CollabConflict, CollabSession } from "../collab/useCollabSession";
import {
  NOT_LIVE,
  fileSpace,
  useCollabSession,
} from "../collab/useCollabSession";
import { Breadcrumbs, crumbsOf } from "../components/Breadcrumbs";
import { CopyAddress } from "../components/DetailsPanel";
import { DraftLinkDialog } from "../components/DraftLinkDialog";
import { Loading } from "../components/Loading";
import { BUTTON, FOCUS_RING, TOGGLE, Tooltip } from "../components/primitives";
import { SimilarEngramsPanel } from "../components/SimilarEngramsPanel";
import { domainSpellings } from "../domainNames";
import type { DocumentAdapter } from "../editor/adapter";
import { engramAdapter } from "../editor/adapter";
import CmEditor from "../editor/CmEditor";
import { ConfirmLeaveDialog } from "../editor/ConfirmLeaveDialog";
import { ConflictDialog } from "../editor/ConflictDialog";
import { EditorToolbar } from "../editor/EditorToolbar";
import {
  imagePreviews,
  refreshPreviews,
  setPreviewVersions,
} from "../editor/imagePreviews";
import { FindingsPanel, jumpToLine } from "../editor/FindingsPanel";
import { FrontmatterForm } from "../editor/FrontmatterForm";
import { KeptTextNotice } from "../editor/KeptTextNotice";
import { docText, lineSeparatorFor } from "../editor/setup";
import type { ElementFace } from "../editor/elementContext";
import { previewConfig, surfaceExtensions } from "../editor/surface";
import type { SaveStateKind } from "../editor/SaveState";
import { SaveState } from "../editor/SaveState";
import {
  UploadScreenContext,
  useAttachmentUploads,
  useUploadScreen,
} from "../editor/useAttachmentUploads";
import { useCloseFlow, useExitRequest } from "../editor/useCloseFlow";
import { useRoomExit } from "../editor/useRoomExit";
import { saveKeymap, useEditorSession } from "../editor/useEditorSession";
import { wikilinkResolverFacet } from "../editor/wikilinkChips";
import { useIgnoredEngram } from "../events/ignored";
import { moveDraft } from "../editor/drafts";
import { editRoute, engramRoute, engramsRoute } from "../paths";
import { ENGRAM_PREFETCH } from "../prefetch";
import { useTheme } from "../theme/context";
import type { WikilinkResolver } from "../wikilinks";
import { buildWikilinkResolver } from "../wikilinks";
import NotFound from "./NotFound";

/** The buffer's accessible name - shared with the state a conflict rebuilds. */
const ARIA_LABEL = "Engram source";

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

/**
 * How long the "this engram was deleted" notice stays on screen before the
 * author is walked back to the domain.
 *
 * Long enough to be read: the editor they are looking at is about to be
 * replaced, and "your text is kept as a draft" is the sentence that says
 * where the work went. The navigation is not optional either - the engram is
 * gone, so the page they are on now 404s.
 */
const CLOSED_NOTICE_MS = 1200;

/** A refused save or upload: the server's own sentence. */
const ALERT =
  "rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200";
/** The session banners: moved, closed, an offered draft. */
const AMBER_BANNER =
  "flex flex-wrap items-baseline gap-3 rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-100";

/**
 * The Details rail: the frontmatter form, the findings and the address. A
 * column beside the text from the large breakpoint; on a narrower screen the
 * same element sits above the text behind a "Details" disclosure, so the
 * fields are one press away without pushing the text down a screen. One
 * element either way, so the two placements cannot drift apart.
 */
function EditorDetails({
  children,
  hasHardErrors,
}: {
  children: ReactNode;
  /** Opens the disclosure when hard errors appear: the alert above says
   *  "see Findings", and the findings are in here. */
  hasHardErrors: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [hadErrors, setHadErrors] = useState(hasHardErrors);
  if (hasHardErrors !== hadErrors) {
    setHadErrors(hasHardErrors);
    if (hasHardErrors) {
      setOpen(true);
    }
  }
  const body = useId();
  return (
    <aside
      aria-label="Details"
      className="order-2 flex flex-col gap-3 border-b border-hair pb-3 lg:col-start-2 lg:row-span-2 lg:row-start-1 lg:border-0 lg:pb-0"
    >
      <h2 className="hidden text-caption font-semibold text-ink-3 lg:block">
        Details
      </h2>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={body}
        onClick={() => {
          setOpen(!open);
        }}
        className={`flex items-center justify-between rounded py-1 text-sm font-medium text-ink-3 lg:hidden ${FOCUS_RING}`}
      >
        Details
        <ChevronDown
          aria-hidden="true"
          size={16}
          className={open ? "rotate-180" : ""}
        />
      </button>
      <div
        id={body}
        className={`flex-col gap-4 ${open ? "flex" : "hidden lg:flex"}`}
      >
        {children}
      </div>
    </aside>
  );
}

/** The shared text and the room's presence, once a session has synced. */
interface Room {
  ytext: Y.Text;
  awareness: Awareness;
}

/** Everything one buffer on this screen is built from. */
interface EngramSurfaceOptions {
  /** The text the buffer opens with, whose line endings it inherits. */
  content: string;
  /** What an engram adds to the shared surface. */
  adapter: DocumentAdapter;
  dark: boolean;
  /** The engram's own domain, which the image previews load from. */
  domain: string;
  /** The layer switch the Raw toggle reconfigures. */
  preview: Compartment;
  raw: boolean;
  /** The resolver's own switch, reconfigured when the graph lands. */
  resolverBox: Compartment;
  resolver: WikilinkResolver;
  /** The session this buffer is bound to, or null on the solo surface. */
  room: Room | null;
  /** Paste and drop uploads; see `SurfaceOptions`. */
  uploads: Extension;
  /** Told when the caret enters or leaves a table; see `SurfaceOptions`. */
  onTableContext: (inTable: boolean) => void;
  /** Told when the caret enters or leaves an image; see `SurfaceOptions`. */
  onImageContext: (onImage: boolean) => void;
  /** Told what callout or label the caret is in; see `SurfaceOptions`. */
  onElementContext: (face: ElementFace) => void;
}

/**
 * The whole extension set of one buffer, in the one place this screen spells
 * it: the shared surface plus what only this screen adds.
 *
 * Two sites build a state for this screen - the mount and the wholesale
 * rebuild the session's `replaceWith` performs - and `setState` replaces the
 * configuration entirely, so anything listed in only one of them silently
 * disappears the first time a swap rebuilds the buffer.
 */
function engramSurface(options: EngramSurfaceOptions): Extension[] {
  const room = options.room;
  return [
    // A session buffer pins LF rather than deriving a separator from its
    // text: the shared document is LF space by construction, and deriving it
    // would let a lone CR that somehow survived upstream pin CRLF and shift
    // every offset the binding maps between CodeMirror and Yjs.
    ...(room !== null
      ? [EditorState.lineSeparator.of("\n")]
      : lineSeparatorFor(options.content)),
    ...surfaceExtensions({
      adapter: options.adapter,
      dark: options.dark,
      // The room's own undo stack replaces CodeMirror's: see `baseExtensions`.
      history: room === null,
      screen: [
        ...(room !== null
          ? [
              yCollab(room.ytext, room.awareness, {
                undoManager: new Y.UndoManager(room.ytext),
              }),
              keymap.of(yUndoManagerKeymap),
              normalizePastedEndings,
              cursorFlags,
            ]
          : []),
        saveKeymap,
        options.resolverBox.of(wikilinkResolverFacet.of(options.resolver)),
      ],
      preview: options.preview,
      raw: options.raw,
      images: imagePreviews(options.domain),
      uploads: options.uploads,
      onTableContext: options.onTableContext,
      onImageContext: options.onImageContext,
      onElementContext: options.onElementContext,
    }),
  ];
}

export default function EngramEditor() {
  const params = useParams();
  const domain = params.domain ?? "";
  const permalink = params["*"] ?? "";
  const { canWrite } = useDomainWritable(domain);

  const detail = useQuery({
    queryKey: engramDetailKey(domain, permalink),
    queryFn: () => fetchEngramDetail(domain, permalink),
    enabled: canWrite,
    staleTime: DETAIL_STALE_MS,
  });

  if (!canWrite) {
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
    return <Loading label="Loading the editor" subject={permalink} />;
  }
  // Keyed by address: a different engram is a different editing session.
  return <EditorSurface key={`${domain}/${permalink}`} engram={detail.data} />;
}

/**
 * The mode switch: try the session first, and open the Group B surface
 * untouched when there is no room to join.
 *
 * The surface below is keyed by the session it is bound to, because that is
 * what a rebuild has to replace wholesale: a new epoch is a different server
 * session with a different shared document, and a buffer bound to the old one
 * cannot be carried across. The skeleton also covers the gap between an epoch
 * gap and the rebuilt binding - the moment where the room exists but the text
 * to bind to does not yet - so the surface never renders against a session
 * that has no document.
 */
function EditorSurface({ engram }: { engram: EngramDetail }) {
  // The room carries this document; the stream must not refetch its detail
  // or graph under it. The tree, listing and switcher rows still fire, which
  // a rename receipt wants anyway.
  useIgnoredEngram(engram.domain, engram.permalink);
  // The screen an upload belongs to: this editor, whatever room or buffer
  // it shows (see `useAttachmentUploads`).
  const uploadScreen = useUploadScreen();
  const { user } = useAuth();
  // Anonymous can never reach this screen (`canWrite` gates it above); the
  // fallback only satisfies the types.
  const account = user?.name ?? "anonymous";
  /**
   * Whose document this surface opens a room over, and whether there is a
   * room to open at all.
   *
   * A granted draft stands at the same address the team's own page stands at,
   * and the read says whose it is. The room has to agree: a session opened
   * with no owner named would be a room over THIS reader's own draft of that
   * page, saving every keystroke into it while the header says the work
   * belongs to somebody else.
   *
   * Editing somebody's draft is the second step after seeing it, and the join
   * is what took it - so the owner is sent only where this window holds a
   * join to exactly this draft. Without one there is no room: the buffer is
   * the draft the read handed over, and a save is refused by the server with
   * the sentence that says how to join. Read once for this surface, which is
   * keyed by address and is one editing session.
   */
  const [room] = useState<{ owner?: string; open: boolean }>(() => {
    const owner = engram.draftOwner;
    if (!owner) {
      return { open: true };
    }
    const join = heldJoin();
    const inside =
      join !== null &&
      join.domain === engram.domain &&
      join.permalink === engram.permalink &&
      join.owner === owner;
    return inside ? { owner, open: true } : { open: false };
  });
  const collab = useCollabSession({
    domain: engram.domain,
    permalink: engram.permalink,
    account,
    displayName: user?.display ?? user?.name ?? "someone",
    enabled: room.open,
    overlay: room.owner,
  });
  if (
    collab.mode === "connecting" ||
    (collab.mode === "collab" && collab.ytext === null)
  ) {
    return <Loading label="Connecting the session" subject={engram.title} />;
  }
  return (
    <UploadScreenContext.Provider value={uploadScreen}>
      <Surface
        key={`${collab.mode}:${collab.epoch ?? ""}`}
        engram={engram}
        collab={collab}
        account={account}
      />
    </UploadScreenContext.Provider>
  );
}

function Surface({
  engram,
  collab,
  account,
}: {
  engram: EngramDetail;
  collab: CollabSession;
  account: string;
}) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { resolved } = useTheme();
  const { ytext, awareness } = collab;
  /**
   * The session this buffer belongs to, or null on the solo surface. Fixed
   * for the life of this component: the switch above keys it by the session,
   * so a room that changes is a different surface.
   */
  const room = useMemo<Room | null>(
    () =>
      collab.mode === "collab" && ytext !== null && awareness !== null
        ? { ytext, awareness }
        : null,
    [collab.mode, ytext, awareness],
  );
  /**
   * What the buffer opens with. In a session that is the shared text as it
   * stood at mount - a Y.Text read rather than a CodeMirror read-back - and
   * it is LF session space, so the session's `dirty` compares like with like.
   */
  const [mountText] = useState(() => room?.ytext.toJSON() ?? engram.content);
  /**
   * The same view as state, for the one consumer that needs it while
   * rendering: the frontmatter form dispatches into it. A ref read during
   * render would hand the form `null` forever, because the mount that fills it
   * schedules no re-render of its own.
   */
  const [view, setView] = useState<EditorView | null>(null);
  // A file rewritten elsewhere (another tab, an agent) is announced by a
  // `files` event that refetches this listing; the previews then name the
  // file's current bytes (`changedVersions`, shared with the reading page).
  // Read once the text names a file; the upload flow fills the same entry.
  const listing = useQuery({
    queryKey: attachmentsKey(engram.domain),
    queryFn: () => listAttachments(engram.domain),
    enabled: mountText.includes(ASSETS_PREFIX),
  });
  const fileVersions = useMemo(
    () => changedVersions(queryClient, `engram:${engram.domain}`, listing.data),
    [queryClient, engram.domain, listing.data],
  );
  useEffect(() => {
    setPreviewVersions(engram.domain, fileVersions);
    view?.dispatch({ effects: refreshPreviews.of(null) });
  }, [view, engram.domain, fileVersions]);
  useEffect(
    () => () => {
      setPreviewVersions(engram.domain, new Map());
    },
    [engram.domain],
  );
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
  // Whether the share dialog is open. Mounted only while it is, so an editor
  // nobody shares from never pays for the dialog primitive behind it.
  const [sharing, setSharing] = useState(false);
  /**
   * Whether the caret is in a table, which is what the format bar's context
   * segment is drawn from. The listener inside the buffer reports crossings
   * only, and the setter is React's own stable one, so this costs a render
   * per crossing rather than one per keystroke.
   */
  const [tableActive, setTableActive] = useState(false);
  /** The same, for the caret sitting on an attachment image. */
  const [imageActive, setImageActive] = useState(false);
  /** The callout or status label the caret is in, for their segments. */
  const [element, setElement] = useState<ElementFace>(null);
  /**
   * WHICH conflict this tab has the resolution view open on, rather than a
   * bare open flag. The conflict itself is the server's - it stands until
   * somebody in the room resolves it, and a resolution the server re-raises
   * (a "mine" restore that found somebody else's bytes on disk) arrives as a
   * different one. Holding the conflict itself means the view closes when the
   * question changes instead of silently re-labelling the panes, and a
   * conflict raised later never pops a dialog nobody asked for.
   */
  const [resolving, setResolving] = useState<CollabConflict | null>(null);
  const [resolveProblem, setResolveProblem] = useState<string | null>(null);
  /**
   * Why the last attachment did not land, in the words of whoever refused it -
   * this tab for a file type or a size the server would bounce, the server for
   * everything else. It stands until the next batch starts rather than fading:
   * an author who dropped five files and had one refused needs to be able to
   * read which one.
   */
  const [attachError, setAttachError] = useState<string | null>(null);
  /**
   * The nearest existing engrams the last save found itself close to, and
   * the server's words on what to do about them - null when there is
   * nothing to show, whether because the last save carried no advisory or
   * because this one has already been dismissed. Local to this tab and
   * never round-tripped: dismissing it does not tell the server anything,
   * and the next save asks again from scratch.
   */
  const [advisory, setAdvisory] = useState<{
    similar: SimilarEngram[];
    guidance: string | null;
  } | null>(null);
  /**
   * The live buffer, for the upload flow. A ref rather than the `view` state
   * above, because the extensions are read once at mount and the handlers
   * inside them have to reach whatever view exists when a file is dropped.
   */
  const uploadView = useRef<EditorView | null>(null);
  const uploads = useAttachmentUploads({
    domain: engram.domain,
    subject: engram.title,
    view: () => uploadView.current,
    onError: setAttachError,
  });

  // The same pair the engram page reads, under the same cache keys: an author
  // who arrived from that page pays nothing on the wire for the chips.
  const graph = useQuery({
    queryKey: graphKey(engram.domain, engram.permalink, NEIGHBORHOOD_DEPTH),
    queryFn: () => fetchGraph(engram.domain, engram.permalink),
  });
  // Under the sidebar's own key, so this is a cache read rather than a
  // request: the chips need it to tell `[[Log: Weekly Notes]]`, a title, from
  // `[[ops:Runbook]]`, a domain, and only the registry knows which is which.
  const domains = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });
  const domainNames = useMemo(
    () =>
      domains.data === undefined
        ? undefined
        : domainSpellings(domains.data.domains),
    [domains.data],
  );
  const resolver = useMemo(
    () => buildWikilinkResolver(engram, graph.data, domainNames),
    [engram, graph.data, domainNames],
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
   * What an engram adds to the shared surface: its chips, lines, fold and
   * completions. One object for the screen's life, so the Raw toggle hands
   * the compartment back the same layers it took away.
   */
  const adapter = useMemo(
    () =>
      // `readVocab` only ever hands `vocabRef` to `crystallineCompletions`,
      // which stores it in a `CompletionSource` closure CodeMirror calls later
      // from typing, not here: nothing on this call stack reads `.current`
      // during this render, but the checker cannot see through the
      // `crystallineCompletions` call to confirm that.
      // eslint-disable-next-line react-hooks/refs
      engramAdapter({
        domain: engram.domain,
        client: queryClient,
        vocab: readVocab,
      }),
    [engram.domain, queryClient, readVocab],
  );

  /**
   * The extensions a buffer swapped in mid-session is rebuilt with: this
   * render's resolver, this render's Raw setting, and the incoming text's own
   * line separator.
   */
  const extensionsFor = (content: string) =>
    engramSurface({
      content,
      adapter,
      dark: resolved === "dark",
      domain: engram.domain,
      preview,
      raw,
      resolverBox,
      resolver,
      room,
      uploads: uploads.extension,
      onTableContext: setTableActive,
      onImageContext: setImageActive,
      onElementContext: setElement,
    });

  /**
   * How a draft is written. The session buffer is LF space, so a snapshot of
   * it is put back into the file's own endings before it is stored - the
   * stored draft is then interchangeable with the solo flow's, whichever
   * surface wrote it. Stable across renders on purpose: it is what re-arms
   * the session's draft debounce, and a room re-renders on every remote
   * keystroke, which an inline closure would let starve the draft to nothing.
   */
  const inRoom = room !== null;
  const separator = collab.separator;
  const draftContent = useCallback(
    (buffer: string) => (inRoom ? fileSpace(buffer, separator) : buffer),
    [inRoom, separator],
  );

  /**
   * The standing "leave when this save lands" flag, made before the session
   * because the save below is what answers it. The flow that arms and clears
   * it is built after the session, where the dirty state it asks about is.
   */
  const exit = useExitRequest();

  // Everything both editors agree on - the buffer's checksum and dirty state,
  // the dry-run gate, drafts, the Mod-S save and the 412 flow - lives in the
  // shared session; what is engram-specific stays here.
  const session = useEditorSession({
    initialContent: mountText,
    // A session has no token to hold: the PUT path it would be spent on is
    // unreachable, because the server owns saving in a room.
    initialChecksum: inRoom ? "" : (engram.checksum ?? ""),
    draftUser: account,
    draftDomain: engram.domain,
    draftSlot: engram.permalink,
    validateDomain: engram.domain,
    validatePath: engram.path,
    save: async (content, token) => {
      if (inRoom) {
        // Unreachable by construction, and loud rather than silent if the
        // transport switch above ever stops holding: a PUT from inside a
        // session would carry a checksum nobody granted, against a server
        // already debounce-saving the same engram.
        throw new Error("unreachable: the collab transport never PUTs");
      }
      let saved;
      try {
        saved = await saveEngram(
          engram.domain,
          engram.permalink,
          content,
          token,
        );
      } catch (error) {
        // A refused save is not an exit. The author stays on the buffer that
        // caused it, with the notice or the conflict view the session raises,
        // and the standing request to leave is dropped rather than left armed
        // for whatever save happens to land next.
        exit.disarm();
        throw error;
      }
      queryClient.setQueryData(
        engramDetailKey(saved.domain, saved.permalink),
        saved,
      );
      // What this save found itself close to, if anything: a fresh save
      // answers with its own advisory or none, so this replaces whatever the
      // last save (or this tab's own dismiss) left standing rather than
      // merging with it.
      setAdvisory(
        saved.similar.length > 0
          ? { similar: saved.similar, guidance: saved.guidance }
          : null,
      );
      // A save is the fourth write that moves the tree, after create, move and
      // retire: the whole file is the document here, so one save can change
      // the title a row is drawn from, the status that fades it, or - through
      // the frontmatter permalink - the address it points at. The tree is
      // fresh for a minute (`TREE_STALE_TIME`), and the sidebar stays mounted
      // for as long as a reader is inside a domain, so without this a renamed
      // engram leaves a row in the frame that 404s when it is clicked.
      void queryClient.invalidateQueries({
        queryKey: domainTreeKey(saved.domain),
      });
      /*
       * Whether this save is the one Save and leave asked for.
       *
       * What is compared is the SENT content rather than the returned content,
       * read back through `docText` at the moment the answer lands: a server
       * that normalizes what it stores would otherwise make every save look
       * stale and no exit would ever be taken. The rest of the rule - why it is
       * conditional at all, and who else clears the flag - lives on
       * `useCloseFlow`, which holds it for both editors.
       */
      if (exit.consume(view === null || docText(view.state) === content)) {
        // The server has confirmed the write, so closing ends where reading
        // begins - at the address the save answered from, which a rename
        // through the frontmatter has already moved. Nothing navigates before
        // this point: a save the server refused throws above and leaves the
        // author where they are.
        void navigate(engramRoute(saved.domain, saved.permalink));
      } else if (saved.permalink !== engram.permalink) {
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
    transport: inRoom ? "collab" : "solo",
    // The one save path in a room: the Save button and Mod-S both call
    // `requestSave`, which the collab transport routes here. A forgotten
    // `flush` would be a Save button that silently does nothing.
    flush: collab.flush,
    live: collab.live,
    draftContent,
  });

  /**
   * The way out, and the question it asks of a buffer with unsaved text in it.
   * Every rule it holds - what each answer means, and what may still act on a
   * save already in the air - lives in `useCloseFlow`, which both editors use.
   */
  const closing = useCloseFlow(
    exit,
    {
      dirty: session.dirty,
      hardErrors: session.hardErrors,
      requestSave: session.requestSave,
      abandon: session.abandon,
    },
    () => {
      void navigate(engramRoute(engram.domain, engram.permalink));
    },
  );

  /**
   * The session's own save receipt, folded back into the buffer's state: the
   * draft has done its job and the current text is the saved text.
   *
   * Only the transition into "ok" counts. Running on every render while the
   * state happened to be "ok" would settle `savedText` onto whatever had just
   * been typed, which would leave `dirty` false forever and stop the draft
   * debounce from ever writing anything.
   */
  const lastSaveState = useRef(collab.saveState);
  useEffect(() => {
    const previous = lastSaveState.current;
    lastSaveState.current = collab.saveState;
    if (inRoom && collab.saveState === "ok" && previous !== "ok") {
      session.noteSaved();
    }
  }, [inRoom, collab.saveState, session]);

  /**
   * The room accepted an external deletion. There is nothing left to save
   * into and nothing left to edit, so the author leaves with their text in
   * the draft store - the same place a crash would have left it - after a
   * beat on the notice that says so.
   */
  const closed = inRoom && collab.closed;
  /** The room's standing conflict, if this surface is in a room at all. */
  const conflict = inRoom ? collab.conflict : null;
  /*
   * Whose document this editor is, as the room decides it: in a domain
   * that reviews changes it is the reader's own draft unless it is
   * somebody else's granted one. Unknown until the listing answers.
   */
  const reviewing =
    domains.data === undefined
      ? null
      : (domains.data.domains.find((entry) => entry.name === engram.domain)
          ?.review ?? null) !== null;
  const walkedOut = useRef(false);
  useEffect(() => {
    if (!closed || walkedOut.current) {
      return;
    }
    // Once: this effect re-runs whenever the session object is rebuilt, and a
    // second snapshot would only restamp the same text.
    walkedOut.current = true;
    session.snapshotDraft();
  }, [closed, session]);
  /**
   * The room closed because the engram moved: nothing is gone, so the author
   * is not walked away. The banner offers the new address instead, and the
   * draft the snapshot above wrote goes along. An editor that was still
   * joining when the move came (its join is refused while the move runs, and
   * the provider tries again) is told the same on its next try, with no room
   * ever opened, so this is read on the solo surface too.
   */
  const movedTo = collab.movedTo;
  // Moved somewhere this person cannot open: the notice says so and stays.
  const movedAway = inRoom && collab.closedReason === "moved";
  useEffect(() => {
    if (!closed || movedTo !== null || movedAway) {
      return;
    }
    const timer = setTimeout(() => {
      // The engram view, not the domain's address, which may open on the
      // Reflection: the reader was working on engrams.
      void navigate(engramsRoute(engram.domain), { replace: true });
    }, CLOSED_NOTICE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [closed, movedTo, movedAway, engram.domain, navigate]);

  /**
   * The unload prompt in a room, keyed on the SESSION's verdict.
   *
   * The solo prompt is off under the collab transport (see
   * `useEditorSession`) because its `dirty` flag compares a file-space
   * mount against an LF buffer and would read true forever on a CRLF file.
   * What is actually at risk here is work the server has not confirmed:
   * "pending" is a save in flight, "failed" and "conflict" are saves it
   * refused. Anything else means the room and the file agree.
   */
  const owedSave =
    inRoom &&
    (collab.saveState === "pending" ||
      collab.saveState === "failed" ||
      collab.saveState === "conflict");
  useEffect(() => {
    if (!owedSave) {
      return;
    }
    const prompt = (event: BeforeUnloadEvent) => {
      event.preventDefault();
    };
    window.addEventListener("beforeunload", prompt);
    return () => {
      window.removeEventListener("beforeunload", prompt);
    };
  }, [owedSave]);

  // The session's rename receipt, followed the same way the solo save's is -
  // and the tree moved on the same way too. A room's saves are the server's
  // own debounce and there is no receipt for most of them, but a rename is a
  // discrete one, so the sidebar can be told exactly when the thing a row is
  // drawn from changed: one refetch per rename rather than one per save.
  useEffect(() => {
    if (inRoom && collab.permalink !== engram.permalink) {
      void queryClient.invalidateQueries({
        queryKey: domainTreeKey(engram.domain),
      });
      void navigate(editRoute(engram.domain, collab.permalink), {
        replace: true,
      });
    }
  }, [
    inRoom,
    collab.permalink,
    engram.domain,
    engram.permalink,
    navigate,
    queryClient,
  ]);

  // The resolver reaches the buffer through its compartment rather than
  // through a remount: the chips redraw, the text and the history stay.
  useEffect(() => {
    session.viewRef.current?.dispatch({
      effects: resolverBox.reconfigure(wikilinkResolverFacet.of(resolver)),
    });
  }, [resolver, resolverBox, session.viewRef]);

  const extensions = useMemo(
    () =>
      engramSurface({
        content: mountText,
        adapter,
        dark: resolved === "dark",
        domain: engram.domain,
        preview,
        raw: RAW_AT_MOUNT,
        resolverBox,
        // The graph has not landed at mount; the compartment above carries
        // the real resolver in as soon as it does.
        resolver: NO_RESOLUTION,
        room,
        uploads: uploads.extension,
        onTableContext: setTableActive,
        onImageContext: setImageActive,
        onElementContext: setElement,
      }),
    // Read once: `CmEditor` snapshots the extensions at mount, so a later
    // theme change reaches the buffer through a remount rather than through
    // this array.
    [
      mountText,
      engram.domain,
      adapter,
      resolved,
      preview,
      resolverBox,
      room,
      uploads.extension,
    ],
  );

  // Written once and drawn in whichever place the width puts it: beside the
  // buffer at the reading measure, under it at full width. The same element
  // either way, so the two placements cannot drift apart in what they hand
  // the panel.
  const findings = (
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
  );

  /*
   * The one save state the header shows. In a room the server saves and its
   * control channel is the only truth about whether that worked; on the solo
   * surface it is this tab's own mutation. A refused save is not a state but
   * the server's sentence, shown as an alert under the header.
   */
  const saveState: SaveStateKind | null = inRoom
    ? collab.status === "reconnecting"
      ? "offline"
      : collab.saveState === "pending"
        ? "saving"
        : collab.saveState === "ok"
          ? "saved"
          : null
    : session.saving
      ? "saving"
      : session.dirty
        ? // Unsaved text outranks the last receipt: a "Saved" beside a
          // buffer that has moved on would claim what is not on disk.
          "unsaved"
        : session.notice?.kind === "done"
          ? "saved"
          : null;
  /*
   * Save goes away once the buffer is clean, which is the moment a press of
   * it succeeds. Focus that stood on it would fall to the page, so it goes
   * back into the text the author was writing.
   */
  // Save stands only while a solo buffer holds work the file does not have:
  // in a room the server saves on its own, and Mod-S still flushes it.
  const showSave = !inRoom && session.dirty;
  /*
   * Done in a room waits a moment for a save the room still owes, and keeps
   * the text as a draft if it cannot land (see `useRoomExit`).
   */
  const roomExit = useRoomExit(
    collab,
    () => {
      session.snapshotDraft();
    },
    () => {
      void navigate(engramRoute(engram.domain, engram.permalink));
    },
    closed,
  );
  const saveFocused = useRef(false);
  useEffect(() => {
    if (!showSave && saveFocused.current) {
      saveFocused.current = false;
      view?.focus();
    }
  });
  const soloProblem =
    !inRoom && session.notice?.kind === "problem" ? session.notice.text : null;

  return (
    <div className="flex flex-col gap-4">
      {/*
        The page's name for assistive tech and for the frame's focus move after
        a route change. On screen the breadcrumb and the Editing chip say it.
      */}
      <h1 className="sr-only">Editing {engram.title}</h1>
      {/*
        The reading page's two columns, so the metadata sits where a reader
        already expects it: the text at the reading measure, the Details rail
        beside it. The form is a view over the buffer rather than a second
        place a value lives: it reads `buffer` and writes back through
        ordinary transactions on the view. It stays at full width too (the
        text column just widens), because the block it edits is hidden from
        the text and the rail is where it is reached outside Raw.
      */}
      <div className="grid gap-y-3 lg:grid-cols-[minmax(0,1fr)_18.75rem] lg:grid-rows-[auto_1fr] lg:gap-x-14">
        {/*
          Three blocks: the header and its notices, the rail, the text. On a
          phone they stack in that order, so the Details disclosure sits under
          the header; from the large breakpoint the rail spans both rows
          beside the other two, its top level with the header's.
        */}
        <div className="edit-column order-1 flex min-w-0 flex-col gap-3 lg:col-start-1 lg:row-start-1">
          <header className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <Breadcrumbs
                crumbs={crumbsOf(engram.domain, engram.permalink, engram.title)}
              />
              <span className="rounded-full bg-accent-tint px-2 py-0.5 text-caption font-medium text-accent-800 dark:text-accent-200">
                Editing
              </span>
              {engram.draftOwner && (
                <span className="rounded-full bg-amber-100 px-2 py-0.5 text-caption text-amber-900 dark:bg-amber-950 dark:text-amber-100">
                  {engram.draftOwner}&apos;s draft
                </span>
              )}
            </div>
            <div className="flex flex-wrap items-center gap-3">
              {inRoom && (
                <PresenceChips
                  participants={collab.participants}
                  offline={collab.status !== "connected"}
                />
              )}
              <SaveState state={saveState} room={inRoom} />
              {/*
                Sharing the draft, and only ever a draft: a link hands over
                work the team has not seen, and there is nothing to hand over
                about a page they all read already.
              */}
              {engram.draft && !engram.draftOwner && engram.path && (
                <Tooltip label="Share this draft with one person">
                  <button
                    type="button"
                    onClick={() => {
                      setSharing(true);
                    }}
                    className={BUTTON.secondary}
                  >
                    Share draft
                  </button>
                </Tooltip>
              )}
              {showSave && (
                <Tooltip label="Save and keep editing">
                  <button
                    type="button"
                    onClick={session.requestSave}
                    onFocus={() => {
                      saveFocused.current = true;
                    }}
                    onBlur={() => {
                      saveFocused.current = false;
                    }}
                    disabled={session.saving || session.hardErrors > 0}
                    className={BUTTON.secondary}
                  >
                    Save
                  </button>
                </Tooltip>
              )}
              {/*
                Done is the way out, and the one primary on this screen. It
                does not save on the way - one press with two meanings hid the
                one that could fail - so on the solo surface it is a button
                that asks what should become of unsaved text before it goes
                anywhere. In a room the server owns the write and is already
                making it; there is nothing for this control to do but leave,
                so it is a link.
              */}
              {inRoom ? (
                <Tooltip label="Leave the editor. The room keeps your work.">
                  {/*
                    Warmed like every other link to the reading screen: that
                    screen is lazy, and an author who deep-linked into the
                    editor has never loaded its chunk.
                  */}
                  <Link
                    to={engramRoute(engram.domain, engram.permalink)}
                    {...ENGRAM_PREFETCH}
                    onClick={roomExit.onDone}
                    aria-disabled={roomExit.phase !== "idle" || undefined}
                    className={BUTTON.primary}
                  >
                    Done
                  </Link>
                </Tooltip>
              ) : (
                <Tooltip label="Leave the editor">
                  <button
                    type="button"
                    onClick={closing.close}
                    className={BUTTON.primary}
                  >
                    Done
                  </button>
                </Tooltip>
              )}
            </div>
          </header>
          {roomExit.phase === "waiting" && (
            <p
              role="status"
              className="text-sm text-slate-500 dark:text-slate-400"
            >
              Saving before you leave.
            </p>
          )}
          {roomExit.phase === "kept" && (
            <p role="alert" className={AMBER_BANNER}>
              Not saved yet. Your text is kept as a draft on this device.
            </p>
          )}
          {soloProblem !== null && (
            <p role="alert" className={ALERT}>
              {soloProblem}
            </p>
          )}
          {inRoom &&
            collab.saveState === "failed" &&
            collab.saveDetail !== null && (
              <p role="alert" className={ALERT}>
                {collab.saveDetail}
              </p>
            )}
          {inRoom && collab.mergeNotice && (
            <p
              role="status"
              className="text-sm text-slate-500 dark:text-slate-400"
            >
              A change from outside was folded into this session.
            </p>
          )}
          {/*
            The save receipt's own advisory, as a panel of its own: a
            guidance sentence and a list of links need their own line. Guarded
            to the solo surface for the same reason `advisory` is only ever
            set there: a room's autosave never reaches the REST save route.
          */}
          {!inRoom && advisory && (
            <SimilarEngramsPanel
              similar={advisory.similar}
              guidance={advisory.guidance}
              onDismiss={() => {
                setAdvisory(null);
                // The Dismiss control held focus and leaves with the panel;
                // without this the next Tab restarts at the top of the page.
                view?.focus();
              }}
            />
          )}
          {session.hardErrors > 0 && (
            <p role="alert" className="text-sm text-red-800 dark:text-red-200">
              {String(session.hardErrors)} hard{" "}
              {session.hardErrors === 1 ? "error" : "errors"}{" "}
              {/*
                On the solo surface the findings hold the save back; in a room
                they cannot - the server saves the shared text on its own
                schedule and refuses what it cannot parse in its own words.
              */}
              {inRoom
                ? "in this engram; see Findings."
                : "block saving; see Findings."}
            </p>
          )}
          {attachError !== null && (
            // The same face a refused save wears: the server, or this tab on
            // the server's rules, would not take what was just handed to it.
            <p role="alert" className={ALERT}>
              {attachError}
            </p>
          )}
          {/*
            No room to join, and the attempt is over rather than still running:
            one quiet line, because editing solo is the ordinary older behavior
            and not a failure the author has to act on.
          */}
          {!inRoom && collab.status === "failed" && (
            <p
              role="status"
              className="text-sm text-slate-500 dark:text-slate-400"
            >
              Editing solo - live collaboration is not available here
            </p>
          )}
          {movedTo !== null && (
            <p role="status" className={AMBER_BANNER}>
              <span>
                {`This engram was moved to ${
                  movedTo.domain === engram.domain
                    ? movedTo.path
                    : `${movedTo.domain} / ${movedTo.path}`
                }.`}
              </span>
              <button
                type="button"
                className="underline underline-offset-2 hover:no-underline"
                onClick={() => {
                  const permalink = movedTo.permalink ?? engram.permalink;
                  // The snapshot taken when the room closed goes to the new
                  // address, where the editor offers it when it differs.
                  moveDraft(
                    account,
                    { domain: engram.domain, slot: engram.permalink },
                    { domain: movedTo.domain, slot: permalink },
                  );
                  void navigate(editRoute(movedTo.domain, permalink));
                }}
              >
                Open it there
              </button>
            </p>
          )}
          {closed && movedTo === null && (
            <p role="status" className={AMBER_BANNER}>
              {collab.closedReason === "deleted"
                ? "This engram was deleted; your text is kept as a draft"
                : collab.closedReason === "moved"
                  ? "This engram was moved to a place you cannot open. Your text is kept as a draft."
                  : "The session ended; your text is kept as a draft"}
            </p>
          )}
          {/*
            Saving is suspended until the room resolves, so the banner stays
            put rather than fading: it is the state of the session, not an
            event.
          */}
          {conflict && (
            <aside
              role="alert"
              className="flex flex-wrap items-baseline gap-3 rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-900 dark:border-red-700 dark:bg-red-950 dark:text-red-100"
            >
              <span>
                Saving is paused: this engram changed outside the session.
              </span>
              <button
                type="button"
                className="rounded underline underline-offset-2 hover:no-underline focus-visible:ring-2 focus-visible:ring-red-500 focus-visible:outline-none"
                onClick={() => {
                  setResolving(conflict);
                }}
              >
                Resolve
              </button>
            </aside>
          )}
          {session.offeredDraft && (
            <aside role="note" className={AMBER_BANNER}>
              <span>
                An unsaved draft of this engram from{" "}
                {session.offeredDraft.savedAt || "an earlier session"} is on
                this browser.
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
            A text the server kept for this engram when a room could not
            save it. Somebody else's draft has none of the reader's own.
          */}
          <KeptTextNotice
            domain={engram.domain}
            kind="engram"
            path={engram.draftOwner ? null : engram.path}
            draft={reviewing}
            ready={inRoom ? collab.live && !closed : view !== null}
            {...(inRoom
              ? {}
              : {
                  soloSaves: {
                    started: session.savesStarted,
                    landedAfter: (count: number) => session.landedSave > count,
                  },
                })}
            onReplace={(text) => {
              // One ordinary change on the buffer, so Undo takes it back: in
              // the mounted line endings, which in a room are LF. A
              // dispatch, never a rebuild, so even a one-line text in a CRLF
              // buffer keeps the history and the file's endings.
              const current = session.viewRef.current;
              if (current === null) {
                return false;
              }
              const separator =
                current.state.facet(EditorState.lineSeparator) ?? "\n";
              const lf = text.replace(/\r\n/g, "\n");
              const target =
                separator === "\n" ? lf : lf.replace(/\n/g, separator);
              current.dispatch({
                changes: {
                  from: 0,
                  to: current.state.doc.length,
                  insert: target,
                },
              });
              current.focus();
              return docText(current.state) === target;
            }}
            onDropped={() => {
              session.viewRef.current?.focus();
            }}
          />
        </div>
        <div className="edit-column order-3 min-w-0 lg:col-start-1 lg:row-start-2">
          {/*
              The format bar is a sticky row on the column, above the text it
              edits, and takes the live view rather than the session: what it
              does are transactions on the buffer, which in a room is the
              shared document and everywhere is the file.

              Raw sits at its right end: a toggle, so the label names the
              thing being switched and `aria-pressed` carries the state. It is
              also the way to the frontmatter as text, which the preview
              hides because the Details rail edits it.
            */}
          <EditorToolbar
            bare
            view={view}
            tableActive={tableActive}
            imageActive={imageActive}
            calloutActive={element?.kind === "callout" ? element : null}
            statusActive={element?.kind === "status" ? element.colour : null}
            onAttach={uploads.attach}
            adapter={adapter}
            trailing={
              <Tooltip label="Show the file as Markdown, frontmatter included">
                <button
                  type="button"
                  aria-pressed={raw}
                  onClick={() => {
                    const next = !raw;
                    setRaw(next);
                    session.viewRef.current?.dispatch({
                      effects: preview.reconfigure(
                        previewConfig(
                          next,
                          resolved === "dark",
                          imagePreviews(engram.domain),
                          adapter,
                        ),
                      ),
                    });
                  }}
                  className={raw ? TOGGLE.on : TOGGLE.off}
                >
                  Raw
                </button>
              </Tooltip>
            }
          />
          <CmEditor
            initialDoc={mountText}
            extensions={extensions}
            ariaLabel={ARIA_LABEL}
            onReady={(ready) => {
              session.onReady(ready);
              uploadView.current = ready;
              setView(ready);
            }}
            onDocChanged={session.setBuffer}
          />
        </div>
        <EditorDetails hasHardErrors={session.hardErrors > 0}>
          <FrontmatterForm
            doc={session.buffer}
            view={view}
            vocabulary={vocabulary.data ?? null}
          />
          <div className="border-t border-hair pt-4">{findings}</div>
          {/*
            The address, one press away: it is what a person hands an agent
            to join the document they are in. The permalink is here too, so
            a save that renamed the engram through its frontmatter shows the
            new one.
          */}
          <div className="flex flex-col gap-1 border-t border-hair pt-4">
            <span className="text-caption font-medium text-slate-600 dark:text-slate-300">
              Address
            </span>
            <div className="flex min-w-0 items-center gap-2">
              <span className="truncate font-mono text-caption text-slate-500 dark:text-slate-400">
                {engram.permalink}
              </span>
              <CopyAddress address={engram.url} />
            </div>
          </div>
        </EditorDetails>
      </div>
      {conflict !== null && resolving === conflict && (
        <CollabConflictDialog
          conflict={conflict}
          // The live buffer, in the room's own LF space: what the panes show
          // side by side is what each side would end up with.
          mine={session.buffer}
          problem={resolveProblem}
          onResolve={(choice) => {
            if (choice === "theirs") {
              // Their version is about to become the room's text (or, on a
              // deletion, the file is about to stay gone): the same
              // writeDraft-then-act order the solo 412 flow uses, so what
              // this author had is recoverable afterwards.
              session.snapshotDraft();
            }
            // Refused while the connection is not live: the dialog stays,
            // and the choice is the person's to make again.
            if (!collab.resolve(choice)) {
              setResolveProblem(NOT_LIVE);
              return;
            }
            setResolveProblem(null);
            setResolving(null);
          }}
          onClose={() => {
            setResolveProblem(null);
            setResolving(null);
          }}
        />
      )}
      {closing.confirming && (
        <ConfirmLeaveDialog
          hardErrors={session.hardErrors}
          onSaveAndClose={closing.saveAndClose}
          onDiscard={closing.discard}
          onKeepEditing={closing.keepEditing}
        />
      )}
      {session.conflict && (
        <ConflictDialog
          conflict={session.conflict}
          mine={session.buffer}
          onClose={session.onConflictClose}
          onOverwrite={session.onConflictOverwrite}
          onTakeServer={session.onConflictTakeServer}
        />
      )}
      {sharing && engram.path && (
        <DraftLinkDialog
          domain={engram.domain}
          path={engram.path}
          onClose={() => {
            setSharing(false);
          }}
        />
      )}
    </div>
  );
}
