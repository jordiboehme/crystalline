/**
 * One reflection, read.
 *
 * The page is drawn section by section from the texts the engine anchors
 * comments on (`sectionTexts`): the frontmatter and the id lines are never
 * part of what is rendered, so neither can show. Each section is its own
 * `<section>` keyed by its id, which is what a search hit, a thread and a
 * review row link to (`#s-<id>`), and what a selection is read against.
 *
 * Reading is for everybody with at least the comment right; editing is a
 * separate screen over the live room (`reflection-edit/`), the way an engram
 * has a reading page and an editor. The source engrams behind a section are
 * one press away for a reader of engrams, and simply absent for a
 * contributor: the server leaves the field out, and so does this page.
 */

import type { QueryClient } from "@tanstack/react-query";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { KeyboardEvent, ReactElement } from "react";
import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { Download, FolderInput, Layers, Trash2 } from "lucide-react";
import { Link, useLocation, useNavigate, useParams } from "react-router";

import { ApiProblem, problemDetail } from "../api/client";
import type { AssetResolver } from "../api/files";
import type {
  Disclosed,
  ReflectionDoc,
  SectionView,
  SourceView,
} from "../api/reflection";
import {
  changedVersions,
  deleteReflection,
  fetchReflection,
  fetchReflectionAssets,
  reflectionAssetHome,
  reflectionAssetsKey,
  reflectionDocKey,
  saveReflection,
} from "../api/reflection";
import type { ReflectionRight } from "../api/reflectionDomains";
import {
  PACT_PATH,
  atLeast,
  fetchReflectionStatus,
  invalidateReflection,
  mayEditPage,
  reflectionStatusKey,
} from "../api/reflectionDomains";
import { useAuth } from "../auth/AuthContext";
import { AttachmentsSection } from "../components/AttachmentsSection";
import { DestructiveAction } from "../components/DestructiveAction";
import { LazyBoundary } from "../components/LazyBoundary";
import { Markdown } from "../components/Markdown";
import type { HeadingAside } from "../components/MarkdownBody";
import { useReflectionStatus } from "../components/ReflectionModeSwitch";
import { ReflectionStateNotice } from "../components/ReflectionStateNotice";
import { ReflectionThreads } from "../components/ReflectionThreads";
import { SourceRow } from "../components/SourceRow";
import {
  ActionItem,
  ActionSeparator,
  EditLink,
  MoreActions,
  PrintPageStyle,
  PrintTitleBlock,
  PrintViewItem,
} from "../components/PageActions";
import { FOCUS_RING } from "../components/primitives";
import { Breadcrumbs, reflectionCrumbsOf } from "../components/Breadcrumbs";
import { plural } from "../format";
import { PrintNotes } from "../components/PrintNotes";
import { Loading } from "../components/Loading";
import { absoluteUrl } from "../base";
import {
  reflectionEditRoute,
  reflectionPageRoute,
  reflectionRoute,
} from "../paths";
import { assetRefsIn } from "../editor/imageFormat";
import { FOCUS_HEADING, useHeadingFocusRequest } from "../focusHeading";
import { NO_COMMANDS, useRegisterCommands } from "../commands";
import { saveBlob } from "../components/downloads";
import { lazyChunk } from "../lazyChunk";
import { discardPageDrafts } from "../reflectionPageDrafts";
import { CHECKING_LOCK, useDomainWritable } from "../domainWritable";
import { tickAt } from "../taskTick";
import { printNotes } from "../printNotes";
import { reflectionFacts } from "../printSheet";
import { SECTION_ATTR } from "../reflectionQuotes";
import {
  distinctSources,
  hiddenSourcesText,
  sourceKey,
} from "../reflectionSources";
import type { SectionText } from "../reflectionText";
import {
  TOP_SECTION,
  hasOwnTitle,
  pageOffset,
  sectionTexts,
  stripProvenance,
} from "../reflectionText";

/** What a tick on a page that moved underneath says. */
const PAGE_CHANGED = "The page changed. Try again.";

const MoveReflectionDialog = lazyChunk(() =>
  import("../components/MoveReflectionDialog").then((m) => ({
    default: m.MoveReflectionDialog,
  })),
);

const MoveAssetDialog = lazyChunk(() =>
  import("../components/MoveAssetDialog").then((m) => ({
    default: m.MoveAssetDialog,
  })),
);

/** What removing a Reflection file leaves behind, said before it happens. */
function removeFileText(name: string): string {
  return `Remove ${name}? Pages keep their links until edited. The reflect queue names them as missing.`;
}

const LINK =
  "text-link underline decoration-(--link-line) decoration-1 underline-offset-[3px] hover:no-underline";

/**
 * "3 sections · Built from 2 engrams · 3 open threads", from what the page
 * already holds. The engram part is left out for a reader without engram
 * rights (the server sends no sources then) and every zero part is left out.
 * It counts the distinct engrams the reader is shown: sources in a domain
 * the reader may not open arrive as a count per section, which cannot say
 * whether two sections name the same engram, so they are left out here and
 * counted on each section's own source button instead.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function reflectionMetaLine(
  page: Pick<ReflectionDoc, "sections" | "threads">,
  engramsVisible: boolean,
): string {
  const parts: string[] = [];
  if (page.sections.length > 0) {
    parts.push(plural(page.sections.length, "section", "sections"));
  }
  if (engramsVisible) {
    const sources = distinctSources(page.sections);
    if (sources > 0) {
      parts.push(`Built from ${plural(sources, "engram", "engrams")}`);
    }
  }
  const open = page.threads.filter((t) => t.status === "open").length;
  if (open > 0) {
    parts.push(plural(open, "open thread", "open threads"));
  }
  return parts.join(" \u00b7 ");
}

export default function ReflectionPage(): ReactElement {
  const { domain = "", "*": path = "" } = useParams();
  const status = useReflectionStatus(domain);
  // The move dialog while it is open: the title it names and the path it is
  // anchored on. A move re-anchors it on the new path (it has to outlive the
  // jump to that address); any other change of route shuts it.
  const [moving, setMoving] = useState<{ title: string; at: string } | null>(
    null,
  );
  // The file move dialog while it is open, by the file's stored path. It
  // belongs to the page it was opened on, so any change of route shuts it.
  const [movingFile, setMovingFile] = useState<string | null>(null);
  const [seen, setSeen] = useState(path);
  if (seen !== path) {
    setSeen(path);
    if (moving !== null && moving.at !== path) {
      setMoving(null);
    }
    setMovingFile(null);
  }
  const account = useAuth().user?.name ?? "anonymous";
  if (status.isPending) {
    return <Loading label="Loading the page" subject={path} />;
  }
  if (status.error) {
    return (
      <p className="text-sm text-red-800 dark:text-red-200">
        {problemDetail(status.error)}
      </p>
    );
  }
  if (status.data.state !== "on") {
    return <ReflectionStateNotice status={status.data} />;
  }
  return (
    <>
      {/*
        Keyed by the page: a comment half typed, a picked thread or an open
        sheet belongs to the page it was made on and must not follow the
        reader to the next one.
      */}
      <PageBody
        key={`${domain}/${path}`}
        domain={domain}
        path={path}
        right={status.data.right}
        engramsVisible={status.data.engramsVisible}
        onMove={(title) => {
          setMoving({ title, at: path });
        }}
        onMoveFile={setMovingFile}
        account={account}
      />
      {/*
        Held here, not in the body: after a move the body shows a skeleton
        while the new address loads, and the dialog must outlive that to list
        the pages it could not relink.
      */}
      {moving !== null && (
        <LazyBoundary fallback={null}>
          <MoveReflectionDialog
            domain={domain}
            path={path}
            title={moving.title}
            onMoved={(to) => {
              setMoving({ title: moving.title, at: to });
            }}
            onClose={() => {
              setMoving(null);
            }}
          />
        </LazyBoundary>
      )}
      {movingFile !== null && (
        <LazyBoundary fallback={null}>
          <MoveAssetDialog
            domain={domain}
            path={movingFile}
            onClose={() => {
              setMovingFile(null);
            }}
          />
        </LazyBoundary>
      )}
    </>
  );
}

/**
 * Whether a failed read says the page itself is gone: a 404 the server
 * described in a problem body. A bare 404 came from something in front of
 * the server and says nothing about the page.
 */
function pageIsGone(error: ApiProblem): boolean {
  return error.status === 404 && error.described;
}

/**
 * Whether this reader may still read the domain's Reflection, asked afresh:
 * the server answers 404 for a page a reader may not see, so a 404 for the
 * page means "gone" only while the Reflection itself still answers them.
 * Any failure here keeps the drafts.
 */
async function stillReadable(
  queryClient: QueryClient,
  domain: string,
): Promise<boolean> {
  try {
    const status = await queryClient.fetchQuery({
      queryKey: reflectionStatusKey(domain),
      queryFn: () => fetchReflectionStatus(domain),
      staleTime: 0,
    });
    return status.state === "on" && status.right !== "none";
  } catch {
    return false;
  }
}

function PageBody({
  domain,
  path,
  right,
  engramsVisible,
  onMove,
  onMoveFile,
  account,
}: {
  domain: string;
  path: string;
  right: ReflectionRight;
  engramsVisible: boolean;
  onMove: (title: string) => void;
  /** Opens the file move dialog, which the page holds. */
  onMoveFile: (path: string) => void;
  account: string;
}): ReactElement {
  const [deleted, setDeleted] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const skipRefocus = useRef(false);
  const confirmRef = useRef<HTMLDivElement | null>(null);
  const doc = useQuery({
    queryKey: reflectionDocKey(domain, path),
    queryFn: () => fetchReflection(domain, path),
    // Off once the page is deleted: dropping it from the cache must not make
    // this screen fetch it again before the route leaves.
    enabled: !deleted,
  });
  const { hash } = useLocation();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  useHeadingFocusRequest(doc.data !== undefined);
  const remove = useMutation({
    mutationFn: () => deleteReflection(domain, path),
    onSuccess: () => {
      setDeleted(true);
      discardPageDrafts(account, domain, path);
      // The page is gone: drop it from the cache before leaving, so nothing
      // refetches it into a 404.
      queryClient.removeQueries({ queryKey: reflectionDocKey(domain, path) });
      invalidateReflection(queryClient, domain);
      void navigate(reflectionRoute(domain), {
        replace: true,
        state: FOCUS_HEADING,
      });
    },
  });
  const articleRef = useRef<HTMLElement | null>(null);
  /*
   * A task box ticked on the page: the whole page as the GET answered it
   * (frontmatter and id lines included) with three characters changed,
   * saved against the checksum it was read at. There is no draft path for
   * a Reflection: the save lands on disk or in the open live room, and the
   * page is read again either way, so the next tick has the new checksum.
   * The ticked text shows until that read lands. A tick alone is no
   * feedback for the reflect queue (the core leaves it out).
   */
  const { capabilities } = useAuth();
  const { domainLocked, lockKnown } = useDomainWritable(domain);
  const [tickProblem, setTickProblem] = useState<string | null>(null);
  const tick = useMutation({
    mutationFn: ({
      markdown,
      checksum,
    }: {
      markdown: string;
      checksum: string;
    }) =>
      saveReflection(domain, path, markdown, { expectedChecksum: checksum }),
    onMutate: () => {
      setTickProblem(null);
    },
    onSuccess: () =>
      queryClient.invalidateQueries({
        queryKey: reflectionDocKey(domain, path),
      }),
    onError: (error: Error) => {
      // Only a stale checksum is "the page changed": a 409 also says the
      // Reflection lives elsewhere now, in words that name the remedy.
      const stale =
        error instanceof ApiProblem &&
        error.status === 409 &&
        error.detail.startsWith("stale edit");
      setTickProblem(stale ? PAGE_CHANGED : problemDetail(error));
      void queryClient.invalidateQueries({
        queryKey: reflectionDocKey(domain, path),
      });
    },
  });
  const markdown = tick.isPending
    ? tick.variables.markdown
    : doc.data?.markdown;
  const sections = useMemo<SectionText[]>(
    () =>
      doc.data && markdown !== undefined
        ? sectionTexts(markdown, doc.data.sections)
        : [],
    [doc.data, markdown],
  );
  const { mutate: saveTick } = tick;
  const onSectionTick = useCallback(
    (index: number, offset: number, next: "[ ]" | "[x]") => {
      const page = doc.data;
      if (page === undefined) {
        return;
      }
      const at = pageOffset(page.markdown, page.sections, index, offset);
      const ticked = at === null ? null : tickAt(page.markdown, at, next);
      if (ticked === null) {
        setTickProblem(PAGE_CHANGED);
        void queryClient.invalidateQueries({
          queryKey: reflectionDocKey(domain, path),
        });
        return;
      }
      saveTick({ markdown: ticked, checksum: page.checksum });
    },
    [doc.data, domain, path, queryClient, saveTick],
  );
  const canTick: boolean | string = capabilities.anonymous
    ? "Sign in to change this page"
    : !mayEditPage(right, path) || capabilities.readOnly || domainLocked
      ? "You can read this page but not change it"
      : !lockKnown
        ? CHECKING_LOCK
        : tick.isPending
          ? "Saving the last change"
          : true;
  // The open threads as printed notes, numbered the way the threads column
  // numbers them in the text while the page prints.
  const notes = useMemo(
    () => printNotes(doc.data?.threads ?? [], sections),
    [doc.data, sections],
  );
  // The Reflection's file listing: the same cache entry the attachments
  // section reads, so this adds no request. A file whose sha256 changed
  // since this document first listed it gets a version in its address, so a
  // replaced file is drawn anew (see `changedVersions`).
  // A page that links no file asks nothing, the same rule the section keeps.
  const linksAFile = useMemo(
    () =>
      doc.data !== undefined &&
      assetRefsIn(doc.data.markdown, reflectionAssetHome(domain, path).resolve)
        .length > 0,
    [doc.data, domain, path],
  );
  const listing = useQuery({
    queryKey: reflectionAssetsKey(domain),
    queryFn: () => fetchReflectionAssets(domain),
    enabled: linksAFile,
  });
  const versions = useMemo(
    () => changedVersions(queryClient, domain, listing.data),
    [queryClient, domain, listing.data],
  );
  // One resolver per page: images and files load from the Reflection itself.
  const assets = useMemo(
    () => reflectionAssetHome(domain, path, versions),
    [domain, path, versions],
  );
  // A link to a section (`#s-<id>`) or a thread (`#<thread id>`, which is
  // t-<hex> already: add no prefix) scrolls there once
  // the page has something to scroll to.
  useEffect(() => {
    if (!doc.data || hash.length < 2) {
      return;
    }
    document
      .getElementById(decodeURIComponent(hash.slice(1)))
      ?.scrollIntoView({ block: "start" });
  }, [doc.data, hash]);

  // The page as received for this reader, minus the `reflection:` block
  // (Crystalline's own provenance). Id lines and other keys stay as written.
  const text = doc.data?.markdown;
  const download = useCallback(() => {
    if (text === undefined) {
      return;
    }
    saveBlob(
      new Blob([stripProvenance(text)], { type: "text/markdown" }),
      path.split("/").at(-1) ?? path,
    );
  }, [text, path]);
  const commands = useMemo(
    () =>
      text === undefined
        ? NO_COMMANDS
        : [
            {
              id: "download",
              title: "Download this page as Markdown",
              run: download,
            },
          ],
    [text, download],
  );
  useRegisterCommands(commands);

  const missing = doc.error instanceof ApiProblem && doc.error.status === 404;
  // The server says the page is not there: its drafts on this device go,
  // but only when the page is really gone (the rule is written down in
  // `reflectionPageDrafts.ts`).
  const gone = doc.error instanceof ApiProblem && pageIsGone(doc.error);
  useEffect(() => {
    if (!gone) {
      return;
    }
    let current = true;
    void stillReadable(queryClient, domain).then((readable) => {
      if (current && readable) {
        discardPageDrafts(account, domain, path);
      }
    });
    return () => {
      current = false;
    };
  }, [gone, queryClient, account, domain, path]);

  if (doc.error) {
    return (
      <div className="flex flex-col items-start gap-3">
        <h1 className="text-display">
          {missing ? "Page not found" : "The page could not be read"}
        </h1>
        {missing ? (
          <p className="text-sm">{`No page at ${path} in ${domain}.`}</p>
        ) : (
          <p role="alert" className="text-sm text-red-800 dark:text-red-200">
            {problemDetail(doc.error)}
          </p>
        )}
        <Link to={reflectionRoute(domain)} className={`text-sm ${LINK}`}>
          {`See the Reflection of ${domain}`}
        </Link>
      </div>
    );
  }
  if (!doc.data) {
    return <Loading label="Loading the page" subject={path} />;
  }
  const page = doc.data;
  // The page's own `# Title` is its heading when it has one; otherwise the
  // header draws the title, so the page always has exactly one.
  const ownTitle = hasOwnTitle(sections[0]?.text ?? "");
  // Replace, move and remove are curate verbs, offered on the reading page
  // only: a move waits while a linking page is open in a room, the
  // curator's own included.
  const curate = atLeast(right, "curate");
  const metaLine = reflectionMetaLine(page, engramsVisible);
  const movable = atLeast(right, "edit") && path !== PACT_PATH;
  // A Reflection page has no `crystalline://` name: on paper it is known by
  // its page in this app.
  const address = absoluteUrl(reflectionPageRoute(domain, path));
  return (
    // Below the width where the threads column fits, its pill floats over
    // the foot of the page: the padding keeps the last line clear of it.
    <div
      data-print-column=""
      className="grid gap-6 pb-16 xl:grid-cols-[minmax(0,1fr)_340px] xl:gap-14 xl:pb-0"
    >
      <div className="flex min-w-0 flex-col gap-6">
        {/*
          On paper the header gives way to a title block, which carries the
          title even when the text opens with its own (that one folds away in
          print), and the page margins carry the rest.
        */}
        <PrintPageStyle
          source={`${domain} / Reflection`}
          title={page.title}
          address={address}
        />
        <PrintTitleBlock
          trail={[domain, "Reflection", ...path.split("/").slice(0, -1)]}
          title={page.title}
          facts={reflectionFacts(page, engramsVisible)}
          address={address}
          webAddress={address}
        />
        <header
          className={`flex flex-col gap-2 print:hidden ${ownTitle ? "-mb-2" : ""}`}
        >
          <div className="flex items-center justify-between gap-2">
            <Breadcrumbs
              crumbs={reflectionCrumbsOf(domain, path, page.title)}
            />
            <div className="flex shrink-0 items-center gap-2 print:hidden">
              {mayEditPage(right, path) && (
                <EditLink to={reflectionEditRoute(domain, path)} />
              )}
              {/*
              Print view for every reader. Move and Delete are curate verbs
              and rare ones, so they sit under a rule, Delete in red; the
              pact can be neither moved nor deleted.
            */}
              <MoreActions
                triggerRef={moreRef}
                onCloseAutoFocus={(event) => {
                  // Delete hands focus to its confirmation, not back here.
                  if (skipRefocus.current) {
                    skipRefocus.current = false;
                    event.preventDefault();
                    confirmRef.current
                      ?.querySelector<HTMLElement>("button[aria-label]")
                      ?.focus();
                  }
                }}
              >
                <ActionItem
                  label="Download as Markdown"
                  icon={Download}
                  onSelect={download}
                />
                <PrintViewItem
                  onSelect={() => {
                    window.print();
                  }}
                />
                {movable && (
                  <>
                    <ActionSeparator />
                    <ActionItem
                      label="Move"
                      icon={FolderInput}
                      onSelect={() => {
                        onMove(page.title);
                      }}
                    />
                    <ActionItem
                      label="Delete"
                      icon={Trash2}
                      destructive
                      onSelect={() => {
                        skipRefocus.current = true;
                        setConfirmingDelete(true);
                      }}
                    />
                  </>
                )}
              </MoreActions>
            </div>
          </div>
          {confirmingDelete && atLeast(right, "edit") && path !== PACT_PATH && (
            <div ref={confirmRef}>
              <DestructiveAction
                label="Delete"
                confirmLabel="Confirm delete"
                hideTrigger
                confirming={confirmingDelete}
                onConfirmingChange={(open) => {
                  setConfirmingDelete(open);
                  if (!open) {
                    moreRef.current?.focus();
                  }
                }}
                pending={remove.isPending}
                onConfirm={() => {
                  remove.mutate();
                }}
              />
            </div>
          )}
          {remove.error && (
            <p
              role="alert"
              className="rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
            >
              {problemDetail(remove.error)}
            </p>
          )}
          {!ownTitle && <h1 className="text-display">{page.title}</h1>}
          {!ownTitle && metaLine !== "" && (
            <p className="text-sm text-ink-3">{metaLine}</p>
          )}
          {page.livePresent.length > 0 && (
            <p
              role="status"
              className="text-sm text-slate-500 dark:text-slate-400"
            >
              {`${page.livePresent.join(", ")} editing now`}
            </p>
          )}
        </header>
        {tickProblem !== null && (
          <p
            role="alert"
            className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 print:hidden dark:bg-red-950 dark:text-red-200"
          >
            {tickProblem}
          </p>
        )}
        <article
          ref={articleRef}
          className="flex flex-col gap-4"
          data-print-fold={ownTitle ? "" : undefined}
        >
          {sections.map((section, index) => (
            <PageSection
              key={section.id ?? `h${index}:${section.heading}`}
              domain={domain}
              assets={assets}
              index={index}
              // The meta line goes under the body's own title when it has
              // one; without one the header draws title and line itself.
              titleNote={
                index === 0 && ownTitle && metaLine !== ""
                  ? metaLine
                  : undefined
              }
              section={section}
              // The intro has a heading of its own only when the page
              // carries its own title; every other section starts with one.
              headed={section.id !== TOP_SECTION || ownTitle}
              engramsVisible={engramsVisible}
              onTick={onSectionTick}
              canTick={canTick}
              view={
                page.sections.find(
                  (s) => s.id === section.id && s.heading === section.heading,
                ) ?? null
              }
            />
          ))}
        </article>
        {/*
          Outside the article: selections for threads are read against it,
          and this list is not part of the page's text.
        */}
        <div className="empty:hidden print:hidden">
          <AttachmentsSection
            domain={domain}
            body={page.markdown}
            home={assets}
            canDelete={curate}
            canReplace={curate}
            onMove={curate ? onMoveFile : undefined}
            onRemoved={() => {
              invalidateReflection(queryClient, domain);
              void queryClient.invalidateQueries({
                queryKey: reflectionAssetsKey(domain),
              });
            }}
            confirmText={removeFileText}
          />
        </div>
        <PrintNotes notes={notes} />
      </div>
      {/*
          The column beside the page from `xl` up, a pill and a sheet below
          it: the threads draw their own frame for either.
        */}
      <ReflectionThreads
        domain={domain}
        path={path}
        doc={page}
        sections={sections}
        right={right}
        articleRef={articleRef}
      />
    </div>
  );
}

function PageSection({
  domain,
  assets,
  index,
  section,
  headed,
  engramsVisible,
  view,
  titleNote,
  onTick,
  canTick,
}: {
  /** Flip a task box at `offset` into this section's text (`index` names the section). */
  onTick: (index: number, offset: number, next: "[ ]" | "[x]") => void;
  /** Whether the boxes tick, or why not. */
  canTick: boolean | string;
  /** Whether the section's text starts with a heading to carry its sources. */
  headed: boolean;
  /** A line under the section's first `# Title`, outside its text. */
  titleNote?: string | undefined;
  /** For the source links only; the rendered body takes no domain (see above). */
  domain: string;
  /** The Reflection's own files: images, file links and refused engram files. */
  assets: AssetResolver;
  /** Position on the page: makes the DOM id of an id-less section unique. */
  index: number;
  section: SectionText;
  engramsVisible: boolean;
  view: SectionView | null;
}): ReactElement {
  const key = section.id ?? section.heading;
  // A section without an id yet has no stable address; its position keeps
  // two same-named headings apart.
  const domId = section.id === null ? `s-h${index}` : `s-${section.id}`;
  const sources =
    engramsVisible && view?.sources !== null && view?.sources !== undefined
      ? view.sources
      : [];
  const hidden = engramsVisible ? (view?.hiddenSources ?? 0) : 0;
  const aside = useSources(
    domain,
    section.heading,
    sources,
    hidden,
    view?.disclosed ?? null,
  );
  const onSectionTick = useCallback(
    (offset: number, next: "[ ]" | "[x]") => {
      onTick(index, offset, next);
    },
    [onTick, index],
  );
  return (
    <section
      id={domId}
      {...{ [SECTION_ATTR]: key }}
      className="flex flex-col gap-1"
    >
      {/*
        No `domain`: it would resolve `assets/` links through the engram
        files route. `assets` resolves them through the Reflection's own.
      */}
      {/*
        A section with no heading of its own carries its sources on a line
        of their own above the text, skipped by the anchoring like the rest.
      */}
      {aside !== null && !headed && (
        <div
          data-anchor-skip=""
          className="relative flex justify-end select-none"
        >
          {aside.aside}
          <div className="absolute top-full right-0">{aside.after}</div>
        </div>
      )}
      <Markdown
        source={section.text}
        assets={assets}
        sourceSpans
        onTick={onSectionTick}
        canTick={canTick}
        {...(titleNote === undefined ? {} : { titleNote })}
        {...(aside !== null && headed ? { headingAside: aside } : {})}
      />
    </section>
  );
}

/**
 * The engrams a section was built from, as a small count at the right end of
 * its heading: "1 source", with an amber dot when one of them changed since
 * (or was retired or is missing, or its domain cannot be checked here). The
 * count is a button named "1 source for <heading>"; the list it opens is laid
 * over the text below the heading, outside it, and shuts on Escape, a press
 * elsewhere or focus leaving it. A source of another domain links into that
 * domain; `hidden` sources, in a domain the reader may not open, are counted
 * and said in one line, never named. A curator also reads who shared a
 * source from a domain fewer people read on purpose, and when. Null for a
 * section with no sources: nothing is drawn.
 */
function useSources(
  domain: string,
  heading: string,
  sources: SourceView[],
  hidden: number,
  disclosed: Disclosed | null,
): HeadingAside | null {
  const [open, setOpen] = useState(false);
  const panel = useId();
  const note = useId();
  const button = useRef<HTMLButtonElement | null>(null);
  const list = useRef<HTMLUListElement | null>(null);
  const changed = sources.some((source) => source.state === "changed");
  const gone = sources.some(
    (source) => source.state === "retired" || source.state === "missing",
  );
  const stale = sources.some((source) => source.state !== "current");
  const count = plural(sources.length + hidden, "source", "sources");
  useEffect(() => {
    if (!open) {
      return;
    }
    const away = (event: Event) => {
      const at = event.target as Node;
      if (!button.current?.contains(at) && !list.current?.contains(at)) {
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", away);
    document.addEventListener("focusin", away);
    return () => {
      document.removeEventListener("pointerdown", away);
      document.removeEventListener("focusin", away);
    };
  }, [open]);
  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape" && open) {
      event.stopPropagation();
      setOpen(false);
      button.current?.focus();
    }
  };
  if (sources.length + hidden === 0) {
    return null;
  }
  return {
    aside: (
      <>
        <button
          ref={button}
          type="button"
          aria-expanded={open}
          aria-controls={panel}
          aria-label={`${count} for ${heading}`}
          aria-describedby={stale ? note : undefined}
          onKeyDown={onKeyDown}
          onClick={() => {
            setOpen(!open);
          }}
          className={`inline-flex items-center gap-1.5 rounded-md px-2 py-0.5 text-[12.5px] leading-5 font-medium text-muted hover:bg-sunken hover:text-ink-3 ${FOCUS_RING}`}
        >
          <Layers aria-hidden size={14} />
          {count}
          {stale && (
            <span
              aria-hidden
              data-testid="sources-changed"
              className="size-1.5 rounded-full bg-(--mark-rule)"
            />
          )}
        </button>
        {stale && (
          <span id={note} hidden>
            {changed
              ? "A source changed."
              : gone
                ? "A source is retired or missing."
                : "A source cannot be checked here."}
          </span>
        )}
      </>
    ),
    after: (
      <ul
        ref={list}
        id={panel}
        hidden={!open}
        onKeyDown={onKeyDown}
        className="absolute top-0 right-0 z-30 flex w-max max-w-[min(22rem,calc(100vw-2rem))] flex-col gap-1 rounded-lg border border-hair bg-raised p-2 text-sm text-ink shadow-pop"
      >
        {sources.map((source) => (
          <SourceRow
            key={sourceKey(domain, source)}
            pageDomain={domain}
            source={source}
            className="flex items-center gap-2 px-1 py-0.5"
          />
        ))}
        {hidden > 0 && (
          <li className="px-1 py-0.5 text-muted">
            {hiddenSourcesText(hidden)}
          </li>
        )}
        {disclosed !== null && (
          <li className="px-1 py-0.5 text-muted">
            {`Shared on purpose by ${disclosed.by} on ${disclosed.at.slice(0, 10)}`}
          </li>
        )}
      </ul>
    ),
  };
}
