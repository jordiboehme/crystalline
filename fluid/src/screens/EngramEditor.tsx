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
import type { Extension, Transaction } from "@codemirror/state";
import { Compartment, EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { keymap } from "@codemirror/view";
import type { QueryClient } from "@tanstack/react-query";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, FileCode } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import type { YSyncConfig } from "y-codemirror.next";
import { yCollab, ySyncFacet, yUndoManagerKeymap } from "y-codemirror.next";
import type { Awareness } from "y-protocols/awareness";
import * as Y from "yjs";

import { problemDetail } from "../api/client";
import { domainTreeKey } from "../api/domain";
import type { EngramDetail } from "../api/engram";
import { engramDetailKey, fetchEngramDetail } from "../api/engram";
import { NEIGHBORHOOD_DEPTH, fetchGraph, graphKey } from "../api/graph";
import type { Vocabulary } from "../api/vocabulary";
import { fetchVocabulary, fullVocabularyKey } from "../api/vocabulary";
import { saveEngram } from "../api/writes";
import { useAuth } from "../auth/AuthContext";
import { CollabConflictDialog } from "../collab/CollabConflictDialog";
import { PresenceChips } from "../collab/PresenceChips";
import type { CollabConflict, CollabSession } from "../collab/useCollabSession";
import { fileSpace, useCollabSession } from "../collab/useCollabSession";
import { Breadcrumbs, crumbsOf } from "../components/Breadcrumbs";
import { BUTTON, ICON_TOGGLE, Tooltip } from "../components/primitives";
import { Skeleton } from "../components/Skeleton";
import CmEditor from "../editor/CmEditor";
import { ConfirmLeaveDialog } from "../editor/ConfirmLeaveDialog";
import { ConflictDialog } from "../editor/ConflictDialog";
import {
  crystallineCompletions,
  crystallineLines,
} from "../editor/crystallineLines";
import { EditorToolbar } from "../editor/EditorToolbar";
import { fenceMono } from "../editor/fenceMono";
import { fencePreviews } from "../editor/fencePreviews";
import { FindingsPanel, jumpToLine } from "../editor/FindingsPanel";
import { frontmatterFold } from "../editor/frontmatterFold";
import { FrontmatterForm } from "../editor/FrontmatterForm";
import { livePreview } from "../editor/preview";
import {
  RAW_MONO,
  baseExtensions,
  docText,
  lineSeparatorFor,
} from "../editor/setup";
import { tableContextListener } from "../editor/tableVerbs";
import { formattingKeymap } from "../editor/toolbar";
import { useAttachmentUploads } from "../editor/useAttachmentUploads";
import { useCloseFlow, useExitRequest } from "../editor/useCloseFlow";
import { saveKeymap, useEditorSession } from "../editor/useEditorSession";
import {
  wikilinkChips,
  wikilinkCompletions,
  wikilinkResolverFacet,
} from "../editor/wikilinkChips";
import { domainRoute, editRoute, engramRoute } from "../paths";
import { ENGRAM_PREFETCH } from "../prefetch";
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
    ? // Raw is deliberately the source view, so the whole buffer goes back to
      // mono: the shared theme sets prose proportional, and this is the one
      // place that undoes it.
      [RAW_MONO]
    : [
        livePreview(),
        wikilinkChips(),
        crystallineLines(),
        fencePreviews(dark),
        // The frontmatter form beside the buffer is the metadata surface, so
        // the block itself folds to one chip here rather than being shown
        // twice. The MANIFEST editor deliberately does not get this: it has
        // no form, so its frontmatter is only ever visible in the buffer.
        frontmatterFold(),
        // Prose is proportional, code is not. Raw mode does not need this -
        // the whole buffer is mono there - so it rides the preview branch
        // with the rest of the read model.
        fenceMono(),
      ];
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

/** The shared text and the room's presence, once a session has synced. */
interface Room {
  ytext: Y.Text;
  awareness: Awareness;
}

/**
 * Whether this transaction is the co-editing binding putting the ROOM's text
 * into the buffer, which must be handed back exactly as it arrived.
 *
 * y-codemirror.next skips its write-back into `Y.Text` only while the
 * transaction still carries the annotation its sync plugin dispatched with,
 * and a transaction rebuilt by the filter below has lost it: the plugin would
 * re-apply a remote insert INTO the shared text and every participant would
 * end up with it twice, with the buffer and the document permanently apart.
 * Session text may legitimately hold a lone CR - the server admits a stray-CR
 * file and broadcasts such lines from a merge - so this is a live path, not a
 * theoretical one.
 *
 * The annotation type is not reachable: y-codemirror.next 0.3.5, the current
 * release, exports `ySyncFacet` and `YSyncConfig` but not `ySyncAnnotation`,
 * and its `exports` map admits no deep import. So the binding is recognized by
 * what it guarantees instead. It dispatches from the `Y.Text` observer, with
 * the shared text ALREADY carrying the change, so the document this
 * transaction produces IS the shared text. A local edit runs the other way -
 * Yjs is written after the transaction lands - so the two still differ here.
 * A state with no binding has no facet and no write-back to protect.
 */
function isRoomWriteBack(tr: Transaction): boolean {
  const sync = tr.startState.facet(ySyncFacet) as YSyncConfig | undefined;
  if (sync === undefined) {
    return false;
  }
  const shared = sync.ytext as Y.Text;
  return tr.newDoc.toString() === shared.toJSON();
}

/**
 * Keep the shared text in LF space whatever gets pasted into it.
 *
 * A session document is LF by construction - the server strips a CRLF file's
 * endings on the way in and puts them back on the way out - so a CR arriving
 * through the clipboard is not this author's line ending, it is a stray byte
 * that would land in every participant's file and in the saved engram.
 */
const normalizePastedEndings = EditorState.transactionFilter.of((tr) => {
  if (!tr.docChanged) {
    return tr;
  }
  let needsRewrite = false;
  tr.changes.iterChanges((_fromA, _toA, _fromB, _toB, inserted) => {
    if (inserted.toString().includes("\r")) {
      needsRewrite = true;
    }
  });
  if (!needsRewrite) {
    return tr;
  }
  // Checked here rather than at the top: the comparison walks the shared
  // text, and only a frame carrying a CR can reach it at all. A sync
  // transaction with no CR in it is handed back untouched either way.
  if (isRoomWriteBack(tr)) {
    return tr;
  }
  const changes: { from: number; to: number; insert: string }[] = [];
  tr.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
    changes.push({
      from: fromA,
      to: toA,
      insert: inserted.toString().replace(/\r\n?/g, "\n"),
    });
  });
  // The selection is deliberately omitted rather than carried over: its
  // positions belong to the unrewritten, longer inserts and can point past
  // the end of the rewritten document, which CodeMirror refuses outright.
  // Given no selection of its own, it maps the existing one through these
  // changes instead, which is the correct place for the cursor anyway.
  //
  // Rebuilding drops the transaction's annotations, which is why the room's
  // own write-back is let through above: this path only ever rebuilds a local
  // edit, whose annotations nothing downstream depends on.
  return { changes };
});

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
  /** The session this buffer is bound to, or null on the solo surface. */
  room: Room | null;
  /**
   * Paste and drop uploads. Inside the options for the same reason the table
   * listener is: every state this screen builds goes through this function,
   * and handlers added beside it at one site would go silent the first time a
   * session rebuilt the buffer.
   */
  uploads: Extension;
  /**
   * Told when the caret enters or leaves a table, so the format bar can draw
   * its table verbs. It travels inside the options rather than being added
   * beside them, because every state this screen builds goes through this
   * function and a listener added at only one site would go silent the first
   * time a session rebuilt the buffer.
   */
  onTableContext: (inTable: boolean) => void;
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
  const room = options.room;
  return [
    // A session buffer pins LF rather than deriving a separator from its
    // text: the shared document is LF space by construction, and deriving it
    // would let a lone CR that somehow survived upstream pin CRLF and shift
    // every offset the binding maps between CodeMirror and Yjs.
    ...(room !== null
      ? [EditorState.lineSeparator.of("\n")]
      : lineSeparatorFor(options.content)),
    // The room's own undo stack replaces CodeMirror's: see `baseExtensions`.
    ...baseExtensions(options.dark, { history: room === null }),
    ...(room !== null
      ? [
          yCollab(room.ytext, room.awareness, {
            undoManager: new Y.UndoManager(room.ytext),
          }),
          keymap.of(yUndoManagerKeymap),
          normalizePastedEndings,
        ]
      : []),
    saveKeymap,
    // What the format bar's context segment watches. Outside the preview
    // compartment: a table is a table in Raw mode too.
    tableContextListener(options.onTableContext),
    // A pasted screenshot and a dropped deck are attachments in Raw mode too,
    // so these sit outside the compartment beside it.
    options.uploads,
    // The toolbar's own shortcuts. Beside `saveKeymap` rather than inside the
    // preview compartment: Mod-b and Mod-i are typing help like the
    // completions below, and raw mode is still markdown being written. Its
    // `Prec.high` wrapper - not its position here - is what beats
    // defaultKeymap's own Mod-i; see `formattingKeymap`.
    formattingKeymap,
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
  const { user } = useAuth();
  // Anonymous can never reach this screen (`canWrite` gates it above); the
  // fallback only satisfies the types.
  const account = user?.name ?? "anonymous";
  const collab = useCollabSession({
    domain: engram.domain,
    permalink: engram.permalink,
    account,
    displayName: user?.display ?? user?.name ?? "someone",
    enabled: true,
  });
  if (
    collab.mode === "connecting" ||
    (collab.mode === "collab" && collab.ytext === null)
  ) {
    return <Skeleton label="Connecting the session" rows={8} />;
  }
  return (
    <Surface
      key={`${collab.mode}:${collab.epoch ?? ""}`}
      engram={engram}
      collab={collab}
      account={account}
    />
  );
}

/**
 * What the room says about saving, which in a session is the server's job
 * rather than this tab's: the control channel's verdict, in its own words
 * when it refused.
 */
function SessionStatus({ collab }: { collab: CollabSession }) {
  return (
    <>
      {collab.status === "reconnecting" && (
        // The one thing an author needs to know while the socket is down:
        // typing is not being thrown away. The buffer stays live and the
        // session resyncs on the same epoch when it comes back - a drop
        // never forks the room into a second history.
        <p role="status" className="text-sm text-amber-700 dark:text-amber-300">
          Reconnecting - edits are kept locally
        </p>
      )}
      {collab.mergeNotice && (
        <p role="status" className="text-sm text-slate-500 dark:text-slate-400">
          A change from outside was folded into this session.
        </p>
      )}
      {collab.saveState === "failed" && collab.saveDetail !== null && (
        <p
          role="alert"
          className="rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {collab.saveDetail}
        </p>
      )}
      {collab.saveState === "pending" && (
        <p className="text-sm text-slate-500 dark:text-slate-400">Saving...</p>
      )}
      {collab.saveState === "ok" && (
        <p className="inline-flex items-center gap-1 text-sm text-slate-500 dark:text-slate-400">
          <Check aria-hidden="true" size={14} strokeWidth={2} />
          Saved
        </p>
      )}
    </>
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
  /**
   * Whether the caret is in a table, which is what the format bar's context
   * segment is drawn from. The listener inside the buffer reports crossings
   * only, and the setter is React's own stable one, so this costs a render
   * per crossing rather than one per keystroke.
   */
  const [tableActive, setTableActive] = useState(false);
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
  /**
   * Why the last attachment did not land, in the words of whoever refused it -
   * this tab for a file type or a size the server would bounce, the server for
   * everything else. It stands until the next batch starts rather than fading:
   * an author who dropped five files and had one refused needs to be able to
   * read which one.
   */
  const [attachError, setAttachError] = useState<string | null>(null);
  /**
   * The live buffer, for the upload flow. A ref rather than the `view` state
   * above, because the extensions are read once at mount and the handlers
   * inside them have to reach whatever view exists when a file is dropped.
   */
  const uploadView = useRef<EditorView | null>(null);
  const uploads = useAttachmentUploads({
    domain: engram.domain,
    view: () => uploadView.current,
    onError: setAttachError,
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
      room,
      uploads: uploads.extension,
      onTableContext: setTableActive,
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
       * Whether this save is the one Save and close asked for.
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
  useEffect(() => {
    if (!closed) {
      return;
    }
    const timer = setTimeout(() => {
      void navigate(domainRoute(engram.domain), { replace: true });
    }, CLOSED_NOTICE_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [closed, engram.domain, navigate]);

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
      // `readVocab` only ever hands `vocabRef` to `crystallineCompletions`,
      // which stores it in a `CompletionSource` closure CodeMirror calls later
      // from typing, not here: nothing on this call stack reads `.current`
      // during this render, but the checker cannot see through the
      // `autocompletion` and `crystallineCompletions` calls to confirm that.
      // eslint-disable-next-line react-hooks/refs
      surfaceExtensions({
        content: mountText,
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
        room,
        uploads: uploads.extension,
        onTableContext: setTableActive,
      }),
    // Read once: `CmEditor` snapshots the extensions at mount, so a later
    // theme change reaches the buffer through a remount rather than through
    // this array.
    [
      mountText,
      engram.domain,
      queryClient,
      resolved,
      preview,
      resolverBox,
      readVocab,
      room,
      uploads.extension,
    ],
  );

  return (
    <div className="flex flex-col gap-4">
      <header className="flex flex-col gap-2">
        {/*
          Where this engram lives, above its name - the same trail the read
          page carries, so moving between reading and editing does not move
          the address to a different place on the screen.

          The controls sit at the right end of that same trail row, which is
          where the reading page carries its own. They used to stand one row
          lower, beside the title, so every toggle between reading and editing
          shunted the row of controls up or down the screen under the pointer
          that had just pressed one of them.
        */}
        <div className="flex flex-wrap items-center justify-between gap-2">
          <Breadcrumbs
            crumbs={crumbsOf(engram.domain, engram.permalink, engram.title)}
          />
          <div className="flex flex-wrap items-center gap-2">
            {/*
              Who reports on saving depends on who does it. In a room the
              server saves and its control channel is the only truth about
              whether that worked; on the solo surface it is this tab's own
              mutation.
            */}
            {inRoom ? (
              <SessionStatus collab={collab} />
            ) : (
              <>
                {session.notice && (
                  <p
                    role={
                      session.notice.kind === "problem" ? "alert" : "status"
                    }
                    className={
                      session.notice.kind === "problem"
                        ? "rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
                        : "inline-flex items-center gap-1 text-sm text-slate-500 dark:text-slate-400"
                    }
                  >
                    {/*
                      The done notice is the solo surface's save receipt -
                      the same "Saved" the room reports, so it wears the same
                      tick. A problem notice is the server's words and gets
                      no mark.
                    */}
                    {session.notice.kind === "done" && (
                      <Check aria-hidden="true" size={14} strokeWidth={2} />
                    )}
                    {session.notice.text}
                  </p>
                )}
                {session.dirty && !session.notice && (
                  <p className="text-sm text-slate-500 dark:text-slate-400">
                    Unsaved changes
                  </p>
                )}
              </>
            )}
            {/*
              A toggle, so the label names the thing being switched and
              `aria-pressed` carries the state. A button whose text flipped to
              "Preview" while announcing itself as pressed would be telling a
              screen reader the opposite of what it shows.

              A glyph rather than the word, because this row's two verbs - Save
              and Close - are what should read as words on it, and a mode switch
              standing between them in the same letters competes with both. The
              name survives where a name is what a control is known by: the
              accessible name and the tooltip both still say Raw, and the
              document glyph says which of the two faces the buffer is wearing.
            */}
            <Tooltip label="Raw">
              <button
                type="button"
                aria-pressed={raw}
                aria-label="Raw"
                onClick={() => {
                  const next = !raw;
                  setRaw(next);
                  session.viewRef.current?.dispatch({
                    effects: preview.reconfigure(
                      previewConfig(next, resolved === "dark"),
                    ),
                  });
                }}
                // Quiet until it is on, then an accent wash. The two faces are
                // whole strings rather than a base plus overrides: see `TOGGLE`
                // for why layering accent utilities onto the ghost tier renders
                // nothing at all.
                className={raw ? ICON_TOGGLE.on : ICON_TOGGLE.off}
              >
                <FileCode aria-hidden="true" size={16} strokeWidth={1.75} />
              </button>
            </Tooltip>
            {/*
              What each of the two verbs promises, in one line, where the
              pointer already is. The buttons say what they do; these say what
              happens next - through the app's own tooltip, because Raw next to
              them wears that surface and a native `title` here was a second
              one, drawn a beat later in a font the page does not choose.
            */}
            <Tooltip label="Save and keep editing">
              <button
                type="button"
                onClick={session.requestSave}
                // In a room the client's verdict never gates a save: the
                // server owns the write, it debounce-saves whatever the shared
                // text holds regardless of this tab, and its own parse refusal
                // is the authoritative gate - it comes back as a save-failed
                // control in the server's words. A button disabled here while
                // Mod-S flushed and the server saved anyway would be a control
                // lying about what is happening.
                disabled={session.saving || (!inRoom && session.hardErrors > 0)}
                className={BUTTON.secondary}
              >
                Save
              </button>
            </Tooltip>
            {/*
              The primary verb of this screen: Save keeps the work, Close is
              the way out. It does not save on the way - one press with two
              meanings hid the one that could fail - so on the solo surface it
              is a button that asks what should become of unsaved text before
              it goes anywhere. In a room the server owns the write and is
              already making it; there is nothing for this control to do but
              leave, so it stays the link it has always been.
            */}
            {inRoom ? (
              <Tooltip label="Close; the server keeps the room's work">
                {/*
                  Warmed like every other link to the reading screen: that
                  screen is lazy now, and an author who deep-linked into the
                  editor has never loaded its chunk, which makes this the one
                  link to it most likely to be cold when it is pressed.
                */}
                <Link
                  to={engramRoute(engram.domain, engram.permalink)}
                  {...ENGRAM_PREFETCH}
                  className={BUTTON.primary}
                >
                  Close
                </Link>
              </Tooltip>
            ) : (
              <Tooltip label="Close the editor">
                <button
                  type="button"
                  onClick={closing.close}
                  className={BUTTON.primary}
                >
                  Close
                </button>
              </Tooltip>
            )}
          </div>
        </div>
        <div className="flex flex-wrap items-baseline gap-3">
          <h1 className="text-title">Editing {engram.title}</h1>
          {/*
            The permalink stays beside the title rather than folding into the
            trail above, which carries folders and the title but never the
            slug. This screen has no details panel to hold the address, and a
            save that renamed the engram through its frontmatter answers at a
            new one: this line is where an author sees that happen.
          */}
          <span className="font-mono text-caption text-slate-500 dark:text-slate-400">
            {engram.permalink}
          </span>
          {inRoom && (
            <PresenceChips
              participants={collab.participants}
              offline={collab.status !== "connected"}
            />
          )}
        </div>
      </header>
      {session.hardErrors > 0 && (
        <p role="alert" className="text-sm text-red-800 dark:text-red-200">
          {String(session.hardErrors)} hard{" "}
          {session.hardErrors === 1 ? "error" : "errors"}{" "}
          {/*
            Two different truths about the same findings: on the solo
            surface they hold the save back, in a room they cannot - the
            server saves the shared text on its own schedule and refuses
            what it cannot parse in its own words.
          */}
          {inRoom
            ? "in this engram; see Findings."
            : "block saving; see Findings."}
        </p>
      )}
      {attachError !== null && (
        // The same face a refused save wears on this screen, because it is the
        // same kind of news: the server, or this tab on the server's rules,
        // would not take what was just handed to it. Nothing was inserted, so
        // the buffer still says what it said.
        <p
          role="alert"
          className="rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {attachError}
        </p>
      )}
      {/*
        No room to join, and the attempt is over rather than still running:
        one quiet line, because editing solo is the ordinary older behavior
        and not a failure the author has to act on.
      */}
      {!inRoom && collab.status === "failed" && (
        <p role="status" className="text-sm text-slate-500 dark:text-slate-400">
          Editing solo - live collaboration is not available here
        </p>
      )}
      {closed && (
        <p
          role="status"
          className="rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-100"
        >
          This engram was deleted; your text is kept as a draft
        </p>
      )}
      {/*
        Saving is suspended until the room resolves, so the banner stays put
        rather than fading: it is the state of the session, not an event.
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
          {/*
            The format bar sits inside the card, above the text it edits, and
            takes the live view rather than the session: what it does are
            transactions on the buffer, which in a room is the shared document
            and everywhere is the file.
          */}
          <EditorToolbar
            view={view}
            tableActive={tableActive}
            onAttach={uploads.attach}
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
      {conflict !== null && resolving === conflict && (
        <CollabConflictDialog
          conflict={conflict}
          // The live buffer, in the room's own LF space: what the panes show
          // side by side is what each side would end up with.
          mine={session.buffer}
          onResolve={(choice) => {
            if (choice === "theirs") {
              // Their version is about to become the room's text (or, on a
              // deletion, the file is about to stay gone): the same
              // writeDraft-then-act order the solo 412 flow uses, so what
              // this author had is recoverable afterwards.
              session.snapshotDraft();
            }
            collab.resolve(choice);
            setResolving(null);
          }}
          onClose={() => {
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
    </div>
  );
}
