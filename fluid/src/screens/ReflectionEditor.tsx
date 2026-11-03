/**
 * One reflection, edited live.
 *
 * The editor is the room and only the room: people and agents edit the same
 * text at once, saves land in the file without a draft, and there is no
 * second, solo save path. A room that cannot be joined says so and offers to
 * try again. The section id lines stay in the text, hidden. The frontmatter,
 * if the page has one, stays as quiet text at the top: this editor has no
 * Details rail, and a page's own `title:` must stay in reach.
 *
 * A daemon that restarted while this tab was away (a laptop that slept)
 * greets the reconnect with a new session; the hook keeps what this tab had
 * as a draft under the room's key first, and the next mount of this screen
 * offers it back.
 *
 * The Reflection's status and this person's right decide whether the room is
 * opened at all, once. After that the room is the authority: a status that
 * turns off or a refetch that fails is shown beside the editor, and the room
 * ends the session itself with its own `closed` control. Unmounting a live
 * room over a status refetch would drop text the server has not saved.
 */

import type { Extension } from "@codemirror/state";
import { Compartment, EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { EditorView as View, keymap } from "@codemirror/view";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { yCollab, yUndoManagerKeymap } from "y-codemirror.next";
import * as Y from "yjs";

import { problemDetail } from "../api/client";
import type { AssetHome } from "../api/files";
import { changedVersions } from "../api/assetVersions";
import {
  fetchReflection,
  fetchReflectionAssets,
  reflectionAssetHome,
  reflectionAssetsKey,
  reflectionKey,
} from "../api/reflection";
import { mayEditPage } from "../api/reflectionDomains";
import { useAuth } from "../auth/AuthContext";
import { CollabConflictDialog } from "../collab/CollabConflictDialog";
import { normalizePastedEndings } from "../collab/pastedEndings";
import { cursorFlags } from "../collab/cursorFlags";
import { PresenceChips } from "../collab/PresenceChips";
import {
  reflectionCollabUrl,
  reflectionOfferKey,
  reflectionRoomKey,
} from "../collab/provider";
import type { CollabMovedTo, CollabSession } from "../collab/useCollabSession";
import { NOT_LIVE, useCollabSession } from "../collab/useCollabSession";
import { useReflectionStatus } from "../components/ReflectionModeSwitch";
import { ReflectionStateNotice } from "../components/ReflectionStateNotice";
import { Breadcrumbs, reflectionCrumbsOf } from "../components/Breadcrumbs";
import { Loading } from "../components/Loading";
import { BUTTON } from "../components/primitives";
import { reflectionAdapter } from "../editor/adapter";
import CmEditor from "../editor/CmEditor";
import type { Draft } from "../editor/drafts";
import { assetRefsIn } from "../editor/imageFormat";
import {
  addOffer,
  clearDraft,
  newestFirst,
  OFFER_LIMIT,
  readDraft,
  readOffers,
  removeOffers,
  writeDraft,
} from "../editor/drafts";
import { EditorToolbar } from "../editor/EditorToolbar";
import { FloatingBarSlot } from "../editor/FloatingBarSlot";
import type { FloatingContext } from "../editor/floatingContext";
import { KeptTextNotice } from "../editor/KeptTextNotice";
import { imagePreviews } from "../editor/imagePreviews";
import { readScalar } from "../editor/frontmatterFields";
import type { SaveStateKind } from "../editor/SaveState";
import { SaveState } from "../editor/SaveState";
import type { ElementFace } from "../editor/elementContext";
import { roomUndo } from "../editor/roomUndo";
import { marksOnEditLine } from "../editor/marks/mode";
import { surfaceExtensions } from "../editor/surface";
import { useMarkSetting } from "../markSetting/context";
import type { TableUiContext } from "../editor/tableVerbs";
import {
  UploadScreenContext,
  useAttachmentUploads,
  useUploadScreen,
} from "../editor/useAttachmentUploads";
import { useRoomExit } from "../editor/useRoomExit";
import { formatInstant } from "../format";
import {
  reflectionEditRoute,
  reflectionPageRoute,
  reflectionRoute,
} from "../paths";
import { movePageDrafts } from "../reflectionPageDrafts";
import { firstTitle, stripIdLines, stripProvenance } from "../reflectionText";
import { useTheme } from "../theme/context";

const LINK =
  "text-link underline decoration-(--link-line) decoration-1 underline-offset-[3px] hover:no-underline";
const ALERT =
  "rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200";
/** The session banners' faces, as the engram editor draws them. */
const AMBER_BANNER =
  "flex flex-wrap items-baseline gap-3 rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-100";
const RED_BANNER =
  "flex flex-wrap items-baseline gap-3 rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-900 dark:border-red-700 dark:bg-red-950 dark:text-red-100";
const BANNER_BUTTON = "underline underline-offset-2 hover:no-underline";

/**
 * The page's title as the trail names it: the frontmatter `title`, else the
 * first `# ` heading, else the file name. Read once from the room's text when
 * the editor opens, the way the server reads it.
 */
function titleOf(text: string, path: string): string {
  const fromFrontmatter = readScalar(text, "title");
  if (fromFrontmatter !== null && fromFrontmatter.trim() !== "") {
    return fromFrontmatter.trim();
  }
  // The body after the frontmatter, so a `title:` line over the closing fence
  // is never read as a setext heading.
  const body = text.replace(/^---\r?\n[\s\S]*?\r?\n---[ \t]*(?:\r?\n|$)/, "");
  const heading = firstTitle(body);
  if (heading !== null && heading.trim() !== "") {
    return heading.trim();
  }
  return (path.split("/").at(-1) ?? path).replace(/\.md$/, "");
}

/** Whether a text links one of the Reflection's files; cheap when it cannot. */
function linksAFile(text: string, home: AssetHome): boolean {
  return text.includes("assets/") && assetRefsIn(text, home.resolve).length > 0;
}

export default function ReflectionEditor(): ReactElement {
  const { domain = "", "*": path = "" } = useParams();
  // Keyed by address: a page opened at its new place after a move is a new
  // editing session, which reads its drafts afresh.
  return <PageEditor key={`${domain}\n${path}`} domain={domain} path={path} />;
}

function PageEditor({
  domain,
  path,
}: {
  domain: string;
  path: string;
}): ReactElement {
  const status = useReflectionStatus(domain);
  // The screen an upload belongs to: this editor, whatever room or buffer
  // it shows (see `useAttachmentUploads`).
  const uploadScreen = useUploadScreen();
  const [attempt, setAttempt] = useState(0);
  const [admitted, setAdmitted] = useState(false);
  const data = status.data;
  const opens =
    data !== undefined && data.state === "on" && mayEditPage(data.right, path);
  if (opens && !admitted) {
    // Decided once: see the module comment.
    setAdmitted(true);
  }
  if (!admitted && !opens) {
    if (data === undefined) {
      return status.isPending ? (
        <Loading label="Loading the page" subject={path} />
      ) : (
        <p role="alert" className={ALERT}>
          {problemDetail(status.error)}
        </p>
      );
    }
    if (data.state !== "on") {
      return <ReflectionStateNotice status={data} />;
    }
    return (
      <div className="flex flex-col items-start gap-2 text-sm">
        <p>
          {data.right === "none"
            ? "You cannot edit this page."
            : "You can comment on this page, but not edit it."}
        </p>
        {data.right !== "none" && (
          <Link to={reflectionPageRoute(domain, path)} className={LINK}>
            Back to the page
          </Link>
        )}
      </div>
    );
  }
  return (
    <UploadScreenContext.Provider value={uploadScreen}>
      <div className="flex flex-col gap-4">
        {status.error !== null ? (
          <p role="alert" className={ALERT}>
            {problemDetail(status.error)}
          </p>
        ) : (
          data !== undefined &&
          data.state !== "on" && <ReflectionStateNotice status={data} />
        )}
        <Room
          key={attempt}
          domain={domain}
          path={path}
          retried={attempt > 0}
          onRetry={() => {
            setAttempt(attempt + 1);
          }}
        />
      </div>
    </UploadScreenContext.Provider>
  );
}

function Room({
  domain,
  path,
  retried,
  onRetry,
}: {
  domain: string;
  path: string;
  /** Whether this mount is a "Try again": focus then goes somewhere useful. */
  retried: boolean;
  onRetry: () => void;
}): ReactElement {
  const { user } = useAuth();
  const account = user?.name ?? "anonymous";
  const navigate = useNavigate();
  const target = useMemo(
    () => ({
      url: reflectionCollabUrl(domain, path),
      // The room holds the page without its provenance block, so the text a
      // conflict is judged against leaves it out too.
      readCurrent: () =>
        fetchReflection(domain, path).then((doc) =>
          stripProvenance(doc.markdown),
        ),
    }),
    [domain, path],
  );
  const collab = useCollabSession({
    domain,
    permalink: reflectionRoomKey(path),
    account,
    displayName: user?.display ?? user?.name ?? "someone",
    enabled: true,
    room: target,
  });
  const connecting =
    collab.mode === "connecting" ||
    (collab.mode === "collab" && collab.ytext === null);
  const failed =
    !connecting &&
    (collab.mode === "solo" ||
      collab.ytext === null ||
      collab.awareness === null);
  // The button that was pressed is gone with the room it remounted; when the
  // new room fails too, focus goes back to the new button.
  const retry = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    if (retried && failed) {
      retry.current?.focus();
    }
  }, [retried, failed]);
  if (connecting) {
    return <Loading label="Connecting the session" subject={path} />;
  }
  if (failed && collab.movedTo !== null) {
    // The page moved while this editor was still joining: its join was
    // refused while the move ran, and the next try was told where it went.
    // Nothing was typed here, so only drafts kept earlier go along.
    const to = collab.movedTo;
    return (
      <div role="status">
        <p className={AMBER_BANNER}>
          <span>{`This page was moved to ${movedPlace(domain, to)}.`}</span>
          <button
            type="button"
            className={BANNER_BUTTON}
            onClick={() => {
              movePageDrafts(account, domain, path, to.path, to.domain);
              void navigate(reflectionEditRoute(to.domain, to.path));
            }}
          >
            Open it there
          </button>
        </p>
      </div>
    );
  }
  if (failed) {
    return (
      <div className="flex flex-col items-start gap-2 text-sm">
        <p>Live editing is not available right now.</p>
        <button
          ref={retry}
          type="button"
          onClick={onRetry}
          className={BUTTON.secondary}
        >
          Try again
        </button>
        <Link to={reflectionPageRoute(domain, path)} className={LINK}>
          Back to the page
        </Link>
      </div>
    );
  }
  return (
    <Surface
      key={collab.epoch ?? ""}
      domain={domain}
      path={path}
      collab={collab}
      account={account}
      focusOnReady={retried}
    />
  );
}

/**
 * What the banner says once the room has ended, by the server's reason and
 * the room's last save verdict. Only "ok" means the text landed: a refused
 * save, a conflict (whose close saves nothing) and a save still in flight
 * all leave the text with this tab. A page deleted or moved out of reach
 * asks for a copy instead of promising a draft: its drafts at this path go
 * on the next 404 there (the rule in `reflectionPageDrafts.ts`).
 */
function endedSentence(
  reason: string | null,
  saveState: CollabSession["saveState"],
): string {
  if (reason === "deleted") {
    return "This page was deleted. Copy your text if you want to keep it.";
  }
  // A move that named no address: the page went somewhere this person may
  // not open, so where is not theirs to learn.
  if (reason === "moved") {
    return "This page was moved to a place you cannot open. Copy your text if you want to keep it.";
  }
  if (saveState !== "ok") {
    return "The session ended; your text is kept as a draft";
  }
  return reason === "switched_off"
    ? "The Reflection was switched off. Your text was saved first."
    : "The session ended. Your text was saved.";
}

/** Where a moved page went, as the banner names it: its path, and its
 *  domain too when that changed. */
function movedPlace(domain: string, to: CollabMovedTo): string {
  return to.domain === domain ? to.path : `${to.domain} / ${to.path}`;
}

/**
 * Whether a kept draft says something the room does not. Id lines are left
 * out on both sides: a save adds an id under a new `##`, and a draft written
 * before that save would otherwise differ from the room by bookkeeping alone.
 */
function differsFromRoom(draft: Draft, room: string): boolean {
  const kept = stripIdLines(draft.content.replace(/\r\n/g, "\n"));
  return kept !== stripIdLines(room);
}

/** When an offered text was set aside, as the offer lists it. */
function offerTime(offer: Draft): string {
  return offer.savedAt === ""
    ? "an earlier session"
    : formatInstant(offer.savedAt);
}

/** How long an offer's preview line may be. */
const PREVIEW_CHARS = 60;

/**
 * One line of an offered text, so offers from close times can be told
 * apart: the first line that differs from the room's text, or the text's
 * start when no line does (lines were only taken out). Ids are left out.
 */
function offerPreview(offer: Draft, room: string): string {
  const mine = stripIdLines(offer.content.replace(/\r\n/g, "\n")).split("\n");
  const theirs = stripIdLines(room).split("\n");
  const at = mine.findIndex(
    (line, index) => line !== theirs[index] && line.trim() !== "",
  );
  const line = at === -1 ? mine.join(" ").trim() : (mine[at] ?? "").trim();
  return line.length > PREVIEW_CHARS
    ? `${line.slice(0, PREVIEW_CHARS)}...`
    : line;
}

/** The editor's switch for a room that has ended: the text stays, typing stops. */
function readOnly(ended: boolean): Extension {
  return ended ? [View.editable.of(false), EditorState.readOnly.of(true)] : [];
}

function Surface({
  domain,
  path,
  collab,
  account,
  focusOnReady,
}: {
  domain: string;
  path: string;
  collab: CollabSession;
  account: string;
  focusOnReady: boolean;
}): ReactElement {
  const { resolved } = useTheme();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const ytext = collab.ytext as Y.Text;
  const awareness = collab.awareness as NonNullable<CollabSession["awareness"]>;
  // Two keys, two jobs: `key` is the copy that follows the live text (the
  // hook's epoch-gap draft lands there too), `offerKey` holds text set aside
  // for a later mount, which that copy can therefore never overwrite.
  const key = reflectionRoomKey(path);
  const offerKey = reflectionOfferKey(path);

  /*
   * The room has ended: closed by the server, or a connection refused for
   * good after the join (a 44xx close without a `closed` control), which
   * leaves a room nobody will save from here either.
   */
  const ended = collab.closed || collab.status === "failed";
  /*
   * What this tab owes the person: text the server has not confirmed while
   * something stands between it and the file (a refusal, a conflict, a lost
   * connection, an ended room). A latch rather than a reading of the current
   * verdict, because a keystroke after a refusal turns the verdict into
   * "pending" and the server does not repeat the same refusal; only the next
   * landed save (or a healthy connection with nothing pending) releases it.
   * An ended room whose last verdict is "ok" owes nothing, unless the page
   * was deleted or moved somewhere this person cannot open: then the text in
   * this tab is the only copy they have. A page moved where they can follow
   * has its text go with them when they open it at its new path.
   */
  const healthy =
    !ended && collab.saveState === "ok" && collab.status === "connected";
  const troubled =
    ended ||
    collab.saveState === "failed" ||
    collab.saveState === "conflict" ||
    collab.status !== "connected";
  // A page moved to a place this person cannot open is gone for them, as a
  // deleted one is: the text in this tab is kept.
  const lost =
    collab.closedReason === "deleted" ||
    (collab.closedReason === "moved" && collab.movedTo === null);
  const latches = troubled && !(ended && collab.saveState === "ok" && !lost);
  const [owed, setOwed] = useState(latches);
  const [wasHealthy, setWasHealthy] = useState(healthy);
  // The server's refusal, held while the latch holds: a keystroke clears the
  // hook's `saveDetail`, but the sentence is still true until a save lands.
  const [refusal, setRefusal] = useState<string | null>(null);
  if (healthy !== wasHealthy) {
    setWasHealthy(healthy);
    if (healthy) {
      setOwed(false);
      setRefusal(null);
    }
  }
  if (latches && !owed) {
    setOwed(true);
  }
  if (
    collab.saveState === "failed" &&
    collab.saveDetail !== null &&
    collab.saveDetail !== refusal
  ) {
    setRefusal(collab.saveDetail);
  }

  /*
   * What this mount finds: a copy left under `key` by an earlier mount (a
   * refusal, a lost connection, an epoch gap) and the short history of
   * offers under `offerKey` (`readOffers`, newest first). Each is offered
   * when it says something the room does not; the copy is the newer work,
   * so it joins the history as its newest entry rather than replacing an
   * offer nobody answered. Read here, moved by the effect below, so a render
   * stays free of storage writes.
   */
  const [found] = useState(() => {
    const room = ytext.toJSON();
    const copy = readDraft(account, domain, key);
    const offered = readOffers(account, domain, offerKey);
    const copyDiffers = copy !== null && differsFromRoom(copy, room);
    const differing = offered.filter((offer) => differsFromRoom(offer, room));
    // Newest first by when each was saved, the copy included: it is
    // usually the newest, but a text set aside in another tab can be newer.
    const offers =
      copy !== null && copyDiffers
        ? newestFirst([
            copy,
            ...differing.filter((offer) => offer.content !== copy.content),
          ])
        : differing;
    return {
      copy,
      copyDiffers,
      // Offers that only repeat the room: dropped by the effect below.
      repeats: offered.filter((offer) => !differing.includes(offer)),
      offers: offers.slice(0, OFFER_LIMIT),
    };
  });
  const [kept, setKept] = useState<Draft[]>(found.offers);
  // Whether the copy under `key` is this mount's own.
  const wrote = useRef(false);
  const [resolving, setResolving] = useState(false);
  const [resolveProblem, setResolveProblem] = useState<string | null>(null);
  const [mountText] = useState(() => ytext.toJSON());
  const [title] = useState(() => titleOf(mountText, path));
  const [mountEnded] = useState(ended);
  const [view, setView] = useState<EditorView | null>(null);
  /*
   * Files: pasted, dropped or picked, stored in the Reflection's own files
   * under a name nobody holds, and linked relative to this page. A refusal
   * gets its own line: it is not a failed save, and it never keeps a draft.
   * The link lands in the room's text like any typing; a block inserted from
   * a heading line goes below the heading's id (`hiddenIdLines`), so nothing
   * has to place it first. A ref for the view, because the paste and drop
   * handlers are read once at mount and must reach the live buffer.
   */
  const home = useMemo(() => reflectionAssetHome(domain, path), [domain, path]);
  // The listing makes a preview's address name a file's current bytes once
  // they changed since this document first listed them (`changedVersions`,
  // shared with the reading page), so a file replaced elsewhere is drawn
  // anew. It is read once the room's text links a file, whoever wrote the
  // link; a page without files asks nothing. The previews sit in a
  // compartment of their own: the editor takes its extensions once, at mount.
  const [linksFiles, setLinksFiles] = useState(() =>
    linksAFile(mountText, home),
  );
  useEffect(() => {
    if (linksFiles) {
      return;
    }
    // Once true it stays true: the listing is then kept fresh by uploads
    // and by the change events, and the check stops running per keystroke.
    const check = () => {
      if (linksAFile(ytext.toJSON(), home)) {
        setLinksFiles(true);
      }
    };
    ytext.observe(check);
    return () => {
      ytext.unobserve(check);
    };
  }, [ytext, home, linksFiles]);
  const listing = useQuery({
    queryKey: reflectionAssetsKey(domain),
    queryFn: () => fetchReflectionAssets(domain),
    enabled: linksFiles,
  });
  const versions = useMemo(
    () => changedVersions(queryClient, domain, listing.data),
    [queryClient, domain, listing.data],
  );
  const previewHome = useMemo(
    () => reflectionAssetHome(domain, path, versions),
    [domain, path, versions],
  );
  const previews = useMemo(() => new Compartment(), []);
  const uploadView = useRef<EditorView | null>(null);
  const [attachError, setAttachError] = useState<string | null>(null);
  // The upload flow lists through the shared cache entry: a listing already
  // on its way is joined rather than asked again, and the answer fills the
  // cache the previews read. `staleTime: 0`, because a free path must be
  // free against what is there now.
  const uploadHome = useMemo<AssetHome>(
    () => ({
      ...home,
      list: () =>
        queryClient.fetchQuery({
          queryKey: reflectionAssetsKey(domain),
          queryFn: () => fetchReflectionAssets(domain),
          staleTime: 0,
        }),
    }),
    [home, queryClient, domain],
  );
  const uploads = useAttachmentUploads({
    domain,
    subject: title,
    home: uploadHome,
    view: () => uploadView.current,
    onError: setAttachError,
    onSettled: () => {
      void queryClient.invalidateQueries({
        queryKey: reflectionAssetsKey(domain),
      });
    },
  });
  const editable = useMemo(() => new Compartment(), []);
  /**
   * The engram editor's surface, with what a Reflection page adds: the
   * hidden section ids, which every block verb keeps under their headings,
   * and the question a heading change asks before it joins a section into
   * the one above. No Raw toggle and no frontmatter fold: the page's own
   * `title:` stays as text in reach.
   */
  const adapter = useMemo(() => reflectionAdapter(), []);
  const preview = useMemo(() => new Compartment(), []);
  /**
   * The marks setting's switch, outside the preview compartment: a flip
   * reconfigures this alone and never rebuilds the previews.
   */
  const marks = useMemo(() => new Compartment(), []);
  const { showOnEditLine } = useMarkSetting();
  useEffect(() => {
    // Nothing else in the transaction: no selection, no changes, no userEvent.
    view?.dispatch({
      effects: marks.reconfigure(marksOnEditLine.of(showOnEditLine)),
    });
  }, [view, marks, showOnEditLine]);
  /** Whether the caret is in a table or on an image: the bar's context verbs. */
  const [tableContext, setTableContext] = useState<TableUiContext>({
    inTable: false,
    drawn: false,
  });
  const [imageActive, setImageActive] = useState(false);
  /** The callout or status label the caret is in, for their segments. */
  const [element, setElement] = useState<ElementFace>(null);
  /** What the floating action bar stands by: a selection, a cell, nothing. */
  const [floating, setFloating] = useState<FloatingContext>(null);
  const extensions = useMemo(() => {
    const undoManager = new Y.UndoManager(ytext);
    return [
      // The shared text is LF space by construction (see EngramEditor).
      EditorState.lineSeparator.of("\n"),
      ...surfaceExtensions({
        adapter,
        dark: resolved === "dark",
        // The room's own undo stack replaces CodeMirror's.
        history: false,
        screen: [
          yCollab(ytext, awareness, { undoManager }),
          roomUndo.of(undoManager),
          keymap.of(yUndoManagerKeymap),
          normalizePastedEndings,
          cursorFlags,
          editable.of(readOnly(mountEnded)),
        ],
        preview,
        marks,
        // The effect above carries the real setting in once the view exists.
        showMarks: false,
        raw: false,
        images: previews.of(imagePreviews(home)),
        uploads: uploads.extension,
        onTableContext: setTableContext,
        onImageContext: setImageActive,
        onElementContext: setElement,
        onFloatingContext: setFloating,
      }),
    ];
  }, [
    ytext,
    awareness,
    resolved,
    adapter,
    preview,
    marks,
    editable,
    mountEnded,
    uploads.extension,
    home,
    previews,
  ]);
  useEffect(() => {
    view?.dispatch({
      effects: previews.reconfigure(imagePreviews(previewHome)),
    });
  }, [view, previews, previewHome]);
  useEffect(() => {
    view?.dispatch({ effects: editable.reconfigure(readOnly(ended)) });
  }, [view, editable, ended]);

  const keep = useMemo(
    () => () => {
      writeDraft(account, domain, key, {
        content: ytext.toJSON(),
        baseChecksum: "",
        savedAt: new Date().toISOString(),
      });
      wrote.current = true;
    },
    [account, domain, key, ytext],
  );

  /**
   * Drop the offers on screen from the offer key, and only those: text set
   * aside after the note appeared ("Take the file version") is newer, and
   * stays for the next mount to offer.
   */
  const dropOffers = (shown: readonly Draft[]) => {
    removeOffers(account, domain, offerKey, shown);
  };

  /**
   * Put one offered text back as the room's text, in one change so the
   * room's Undo takes it back. It answers the offer: every offer on screen
   * goes, and the text is owed until the room saves it (the copy follows
   * it there).
   */
  const putBack = (offer: Draft) => {
    const text = offer.content.replace(/\r\n/g, "\n");
    ytext.doc?.transact(() => {
      ytext.delete(0, ytext.length);
      ytext.insert(0, text);
    });
    dropOffers([offer]);
    setOwed(true);
    setKept((shown) => shown.filter((other) => other !== offer));
    view?.focus();
  };

  // Declared before the copy below, so a copy this mount writes at once is
  // never the one moved away here.
  useEffect(() => {
    if (found.repeats.length > 0) {
      removeOffers(account, domain, offerKey, found.repeats);
    }
    if (found.copy !== null) {
      clearDraft(account, domain, key);
      if (found.copyDiffers) {
        addOffer(account, domain, offerKey, found.copy);
      }
    }
  }, [found, account, domain, key, offerKey]);

  // While the latch holds, the draft follows the text: the server's refusal
  // tells the person "this tab keeps a copy", and the reconnect line says
  // edits are kept locally.
  useEffect(() => {
    if (!owed) {
      return undefined;
    }
    keep();
    ytext.observe(keep);
    return () => {
      ytext.unobserve(keep);
    };
  }, [owed, keep, ytext]);

  // The latch released by a landed save: this mount's own copy is no longer
  // needed. The offer key is never touched here.
  useEffect(() => {
    if (!owed && wrote.current) {
      clearDraft(account, domain, key);
      wrote.current = false;
    }
  }, [owed, account, domain, key]);

  // Text still owed when the screen goes ("Done", any navigation, a status
  // change above) is kept as a draft on the way out. Owed means the latch: a
  // plain "pending" on a connected room is text the room already holds and
  // saves on its own, and a draft of it would come back as a false offer.
  const owedSave =
    owed ||
    collab.saveState === "pending" ||
    collab.saveState === "failed" ||
    collab.saveState === "conflict";
  const leaving = useRef(owed);
  useEffect(() => {
    leaving.current = owed;
  }, [owed]);
  // Set once "Open it there" carried the drafts to the new path: the copy
  // left here would only be offered to a later page at this old path.
  const carried = useRef(false);
  useEffect(
    () => () => {
      if (leaving.current && !carried.current) {
        keep();
      }
    },
    [keep],
  );
  // And the browser asks before a tab with unsaved text unloads, as the engram
  // editor does.
  useEffect(() => {
    if (!owedSave) {
      return undefined;
    }
    const prompt = (event: BeforeUnloadEvent) => {
      event.preventDefault();
    };
    window.addEventListener("beforeunload", prompt);
    return () => {
      window.removeEventListener("beforeunload", prompt);
    };
  }, [owedSave]);

  /*
   * Done waits a moment for a save the room still owes, and keeps the text as
   * a draft if it cannot land (see `useRoomExit`).
   */
  /*
   * Where Done goes. The page, as a rule; once the room ended because the
   * page moved, the page at its new path when this reader may open it, and
   * the domain's Reflection when they may not or the page was deleted: the
   * old path answers nothing any more.
   */
  const movedTo = ended ? collab.movedTo : null;
  const doneTo =
    movedTo !== null
      ? reflectionPageRoute(movedTo.domain, movedTo.path)
      : ended &&
          (collab.closedReason === "moved" || collab.closedReason === "deleted")
        ? reflectionRoute(domain)
        : reflectionPageRoute(domain, path);
  const roomExit = useRoomExit(
    collab,
    () => {
      keep();
    },
    () => {
      void navigate(doneTo);
    },
    ended,
  );

  /*
   * The one save state the header shows. A refusal is not a state but the
   * server's sentence, shown as an alert under the header; an ended room says
   * what happened in its own banner.
   */
  const saveState: SaveStateKind | null =
    collab.status === "reconnecting"
      ? "offline"
      : ended
        ? null
        : collab.saveState === "pending" && refusal === null
          ? "saving"
          : collab.saveState === "ok"
            ? "saved"
            : null;

  return (
    <div className="edit-column flex min-w-0 flex-col gap-3">
      <h1 className="sr-only">Editing {title}</h1>
      <header className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <Breadcrumbs crumbs={reflectionCrumbsOf(domain, path, title)} />
          <span className="rounded-full bg-accent-tint px-2 py-0.5 text-caption font-medium text-accent-800 dark:text-accent-200">
            Editing
          </span>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <PresenceChips
            participants={collab.participants}
            offline={collab.status !== "connected"}
          />
          <SaveState state={saveState} room />
          <Link
            to={doneTo}
            onClick={(event) => {
              if (movedTo !== null) {
                // As "Open it there": the text in this tab goes with it.
                if (leaving.current) {
                  keep();
                }
                movePageDrafts(
                  account,
                  domain,
                  path,
                  movedTo.path,
                  movedTo.domain,
                );
                carried.current = true;
              }
              roomExit.onDone(event);
              void queryClient.invalidateQueries({
                queryKey: reflectionKey(domain),
              });
            }}
            aria-disabled={roomExit.phase !== "idle" || undefined}
            className={BUTTON.primary}
          >
            Done
          </Link>
        </div>
      </header>
      {roomExit.phase === "waiting" && (
        <p role="status" className="text-sm text-slate-500 dark:text-slate-400">
          Saving before you leave.
        </p>
      )}
      {roomExit.phase === "kept" && (
        <p role="alert" className={AMBER_BANNER}>
          Not saved yet. Your text is kept as a draft on this device.
        </p>
      )}
      {refusal !== null && (
        <p role="alert" className={ALERT}>
          {refusal}
        </p>
      )}
      {attachError !== null && (
        <p role="alert" className={ALERT}>
          {attachError}
        </p>
      )}
      {/*
        Mounted from the start and filled when the room ends, so a screen
        reader announces the change rather than meeting a region that
        arrived with its text already in it.
      */}
      <div role="status">
        {ended && collab.movedTo !== null && (
          <p className={AMBER_BANNER}>
            <span>{`This page was moved to ${movedPlace(domain, collab.movedTo)}.`}</span>
            <button
              type="button"
              className={BANNER_BUTTON}
              onClick={() => {
                const to = collab.movedTo;
                if (to === null) {
                  return;
                }
                // The text in this tab goes with the person: kept under this
                // path first, then both drafts carried to the new one, where
                // the next mount offers what the page does not hold.
                keep();
                movePageDrafts(account, domain, path, to.path, to.domain);
                carried.current = true;
                void navigate(reflectionEditRoute(to.domain, to.path));
              }}
            >
              Open it there
            </button>
          </p>
        )}
        {ended && collab.movedTo === null && (
          <p className={AMBER_BANNER}>
            {endedSentence(collab.closedReason, collab.saveState)}
          </p>
        )}
      </div>
      {/*
        A note, not a status: the offer is there when the screen opens and
        waits for a choice, it is not news that happened while the person
        was looking.
      */}
      {kept.length > 0 && (
        <aside role="note" className={AMBER_BANNER}>
          {kept.length === 1 ? (
            <span>Text from an earlier session is kept on this device.</span>
          ) : (
            <>
              <span className="basis-full">{`Texts from ${String(kept.length)} earlier sessions are kept on this device. Newest first.`}</span>
              {/*
                A short history (the last few per page), by time, each with
                a line of its text. Putting one back takes only that one off
                the list; the others stay offered until Discard all, or
                until newer texts push them out of the history.
              */}
              <ul className="flex basis-full flex-col gap-1">
                {kept.map((offer, index) => {
                  const when = offerTime(offer);
                  const preview = offerPreview(offer, mountText);
                  return (
                    <li
                      key={`${String(index)}-${offer.savedAt}`}
                      className="flex min-w-0 flex-wrap items-baseline gap-3"
                    >
                      <span>{when}</span>
                      <span className="min-w-0 truncate font-mono text-xs">
                        {preview}
                      </span>
                      {!ended && (
                        <button
                          type="button"
                          aria-label={
                            preview === ""
                              ? `Put it back: ${when}`
                              : `Put it back: ${when}, ${preview}`
                          }
                          className={BANNER_BUTTON}
                          onClick={() => {
                            putBack(offer);
                          }}
                        >
                          Put it back
                        </button>
                      )}
                    </li>
                  );
                })}
              </ul>
            </>
          )}
          {!ended && kept.length === 1 && (
            <button
              type="button"
              className={BANNER_BUTTON}
              onClick={() => {
                putBack(kept[0] as Draft);
              }}
            >
              Put it back
            </button>
          )}
          <button
            type="button"
            className={BANNER_BUTTON}
            onClick={() => {
              dropOffers(kept);
              setKept([]);
              view?.focus();
            }}
          >
            {kept.length === 1 ? "Discard" : "Discard all"}
          </button>
        </aside>
      )}
      {/*
        A text the server kept for this page when a room could not save it:
        not this browser's copy above, but the server's.
      */}
      <KeptTextNotice
        domain={domain}
        kind="reflection"
        path={path}
        draft={false}
        ready={!ended && collab.live}
        onReplace={(text) => {
          // One change on the shared text, as Put it back makes it, so the
          // room's Undo takes it back; owed until the room saves it.
          const lf = text.replace(/\r\n/g, "\n");
          ytext.doc?.transact(() => {
            ytext.delete(0, ytext.length);
            ytext.insert(0, lf);
          });
          if (ytext.toJSON() !== lf) {
            return false;
          }
          setOwed(true);
          view?.focus();
          return true;
        }}
        onDropped={() => {
          view?.focus();
        }}
      />
      {collab.conflict !== null && (
        <aside role="alert" className={RED_BANNER}>
          <span>Saving is paused: this page changed outside the session.</span>
          <button
            type="button"
            className={`rounded ${BANNER_BUTTON} focus-visible:ring-2 focus-visible:ring-red-500 focus-visible:outline-none`}
            onClick={() => {
              setResolving(true);
            }}
          >
            Resolve
          </button>
        </aside>
      )}
      <div className="relative">
        {/*
          The engram editor's format bar, in the same sticky row. Its block
          verbs know the hidden section ids through the adapter: a block
          inserted from a heading lands below the heading's id
          (`newLineBelowId`), a list or quote over a range leaves the ids and
          their headings alone, and a heading change that joins a section
          into the one above asks first. Every verb is off once the room has
          ended. Attach is off too while the room is away: the bytes would
          not wait for it, but a person should not start what the page
          cannot take now. A paste while reconnecting still uploads, and the
          room merges the link on its return.
        */}
        <EditorToolbar
          bare
          view={view}
          tableActive={tableContext.inTable}
          tableDrawn={tableContext.drawn}
          raw={false}
          cellOpen={floating?.kind === "cell"}
          imageActive={imageActive}
          calloutActive={element?.kind === "callout" ? element : null}
          statusActive={element?.kind === "status" ? element.colour : null}
          onAttach={uploads.attach}
          adapter={adapter}
          disabled={ended}
          attachDisabled={collab.status !== "connected"}
        />
        <CmEditor
          initialDoc={mountText}
          extensions={extensions}
          ariaLabel="Page source"
          onReady={(ready) => {
            uploadView.current = ready;
            setView(ready);
            if (focusOnReady) {
              ready.focus();
            }
          }}
        />
        {/* The floating action bar; none once the room has ended (R17). */}
        <FloatingBarSlot
          main={view}
          context={ended ? null : floating}
          wikilinks={adapter.wikilinks}
        />
      </div>
      {collab.conflict !== null && resolving && (
        <CollabConflictDialog
          conflict={collab.conflict}
          mine={ytext.toJSON()}
          problem={resolveProblem}
          onResolve={(choice) => {
            if (choice === "theirs") {
              // Taking their text drops this tab's: set it aside for the
              // next mount to offer back.
              addOffer(account, domain, offerKey, {
                content: ytext.toJSON(),
                baseChecksum: "",
                savedAt: new Date().toISOString(),
              });
            }
            // Refused while the connection is not live: the dialog stays,
            // and the choice is the person's to make again.
            if (!collab.resolve(choice)) {
              setResolveProblem(NOT_LIVE);
              return;
            }
            setResolveProblem(null);
            setResolving(false);
          }}
          onClose={() => {
            setResolveProblem(null);
            setResolving(false);
          }}
        />
      )}
    </div>
  );
}
