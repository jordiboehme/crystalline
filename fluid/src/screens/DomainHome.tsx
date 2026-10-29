/**
 * One route, two pages: the domain, and one folder inside it.
 *
 * The domain page is the domain's front page: its MANIFEST read as a page,
 * with the configuration and the ways into the domain's settings in a rail
 * beside it, the team's sync and proposals as banners, and its engrams. The
 * folder page (`?path=`) is
 * one folder of it, headed by its path and its size, with the same engrams
 * section and nothing else: a folder is browsed, not administered. The two
 * share the section and share the route, so every folder link ever sent
 * keeps working.
 *
 * There are two ways of looking at what is in a domain, and exactly one is
 * on screen at a time with a line above the list saying which: a folder, or
 * a frontmatter filter across the whole domain. Blending them would mean
 * filters that quietly ignored the folder they sit under, or a folder that
 * quietly dropped what the filter did not match. A filter keeps the page it
 * is on, so a filtered folder page still carries the folder's trail and its
 * count while the list beneath spans the domain and says so.
 *
 * Both are the same endpoint. The listing pages a folder (`path`) exactly as
 * it pages a filter, so a folder holding thousands of engrams costs one page
 * rather than the folder, and the count under a folder's heading is the
 * server's own. What the tree is still for is navigation: the subfolders of
 * the folder being browsed, which is a level rather than a list.
 *
 * Both views live in the URL, so a folder or a filter is a link somebody can
 * send, and the back button moves between them.
 */

import {
  useInfiniteQuery,
  useMutation,
  useMutationState,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import type { LucideIcon } from "lucide-react";
import {
  ChevronDown,
  Download,
  ListFilter,
  Plus,
  Settings,
  TextCursorInput,
  Upload,
} from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { Fragment, useEffect, useId, useMemo, useState } from "react";
import type { ReactNode } from "react";
import {
  Link,
  useLocation,
  useNavigate,
  useParams,
  useSearchParams,
} from "react-router";

import {
  archiveDownloadUrl,
  fetchSyncStatus,
  finishManifestRename,
  keepManifestRename,
  renameReportKey,
  syncPullKey,
  syncStatusKey,
} from "../api/admin";
import type {
  ManifestRenameResult,
  RenameReport,
  SyncStatus,
} from "../api/admin";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { ApiProblem, problemDetail } from "../api/client";
import { fetchManifest, manifestKey, treeQuery } from "../api/domain";
import type { ManifestView } from "../api/domain";
import { fetchReflectionTree, reflectionTreeKey } from "../api/reflection";
import type { TreeNode } from "../api/reflection";
import { DomainLockChip } from "../components/DomainLockChip";
import { useDomainWritable } from "../domainWritable";
import { fetchMembers, governanceOf, membersKey } from "../api/members";
import { GithubLinkHint } from "../components/GithubLinkHint";
import { useUnlinkedFromGithub } from "../githubLink";
import type {
  EngramFilters,
  EngramPage,
  EngramRow,
  ListingOrder,
} from "../api/engrams";
import {
  NO_FILTERS,
  domainEngramsKey,
  fetchDomainEngrams,
  hasFilters,
  hasNextPage,
} from "../api/engrams";
import { fetchTags, vocabularyKey } from "../api/vocabulary";
import type { TagCount } from "../api/vocabulary";
import { useAuth } from "../auth/AuthContext";
import { navigateTo } from "../base";
import { NO_COMMANDS, useRegisterCommands } from "../commands";
import type { PaletteCommand } from "../commands";
import { CreateEngramDialog } from "../components/CreateEngramDialog";
import { RENAMING_REASON } from "../components/DestructiveAction";
import { DomainRail } from "../components/DomainRail";
import { EngramList } from "../components/EngramList";
import { EngramsOrderMenu } from "../components/EngramsOrderMenu";
import { FilterFields, TagChips } from "../components/FilterControls";
import { ImportArchiveDialog } from "../components/ImportArchiveDialog";
import { InlineMarkdown } from "../components/InlineMarkdown";
import { EmptySection, starterFor } from "../components/ManifestStarter";
import { Markdown } from "../components/Markdown";
import {
  ActionSeparator,
  EditLink,
  MoreActions,
} from "../components/PageActions";
import { ProposalsCard } from "../components/ProposalsCard";
import { RenameDomainDialog } from "../components/RenameDomainDialog";
import { Loading } from "../components/Loading";
import { SyncCard } from "../components/SyncCard";
import { ITEM_CLASSES } from "../components/menu";
import { BUTTON, Chip, FOCUS_RING } from "../components/primitives";
import { slugAssigner } from "../anchors";
import { MissingOrReflection } from "./DomainMissing";
import { isMissing, useDomainRights } from "../domainRights";
import { orderQuery, useEngramsOrder } from "../engramsOrder";
import { frontmatterFilters } from "../filters";
import { isWebAddress, plural } from "../format";
import { statusLabelWords } from "../markdown/statusText";
import {
  WHEN_TO_USE_KEY,
  configKeys,
  sectionKey,
  splitManifest,
} from "../manifestProse";
import type { ManifestProse } from "../manifestProse";
import {
  WHOLE_MANIFEST,
  domainSettingsRoute,
  engramsRoute,
  folderRoute,
  manifestEditRoute,
} from "../paths";
import {
  HomeElsewhere,
  useReflectionStatus,
} from "../components/ReflectionModeSwitch";
import { PREFETCH, prefetchManifestEditor } from "../prefetch";

export default function DomainHome() {
  const { domain = "" } = useParams();
  const [params] = useSearchParams();
  const path = params.get("path") ?? "";
  // The tree is what says whether the domain exists at all - a 404 from it
  // is a wrong address, not an empty shelf - and what the folder buttons on
  // both pages are drawn from. Read here, once, so neither page asks twice.
  const tree = useQuery(treeQuery(domain, path));
  if (isMissing(tree.error)) {
    return <MissingOrReflection domain={domain} />;
  }
  const folders = tree.data?.folders ?? [];
  return path === "" ? (
    <DomainPage
      domain={domain}
      folders={folders}
      manifestPermalink={tree.data?.manifestPermalink ?? null}
    />
  ) : (
    <FolderPage domain={domain} path={path} folders={folders} />
  );
}

/**
 * What `EngramsSection`'s callers hand it to change the URL.
 *
 * An alias rather than an interface, because `apply` below walks it with
 * `Object.entries`: a type alias carries an implicit index signature and an
 * interface does not, so the walk over an interface reads as `any`.
 */
type ListingChange = {
  path?: string;
  type?: string | null;
  status?: string | null;
  tags?: string[];
};

/**
 * The listing state both pages share, read from the URL, which is the whole
 * of it.
 *
 * The frontmatter view is the whole domain: the shared reader leaves `path`
 * empty deliberately, because scoping a filter to the folder being browsed
 * is a different feature - the line above the list says "every folder
 * included" and means it. Shared with the sidebar, which reads the same URL
 * to decide whether any folder may call itself the current page: one
 * reading, so the frame and the screen cannot disagree about which of the
 * two views is up.
 */
function useListingState() {
  const [params, setParams] = useSearchParams();
  const { domain = "" } = useParams();
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const path = params.get("path") ?? "";
  const filters: EngramFilters = useMemo(
    () => frontmatterFilters(params),
    [params],
  );
  // The browse view: one folder, no frontmatter filter, paged by the server.
  const browse: EngramFilters = useMemo(
    () => ({ ...NO_FILTERS, path }),
    [path],
  );
  const filtering = hasFilters(filters);
  // The reader's order, the frame's to keep, and derived here rather than in
  // each page that draws a listing: it rides on the cache key and on the
  // request of every listing either page makes, so changing it starts a new
  // list wherever one is being paged.
  const { order } = useEngramsOrder();
  const listingOrder = orderQuery(order);
  /** Change the URL, which is the whole of both pages' listing state. */
  function apply(next: ListingChange) {
    const updated = new URLSearchParams(params);
    for (const [key, value] of Object.entries(next)) {
      const written = Array.isArray(value) ? value.join(",") : (value ?? "");
      if (written === "") {
        updated.delete(key);
      } else {
        updated.set(key, written);
      }
    }
    // Always on the engram view's own address: the domain's bare address
    // sends any query on to it, and a hop through that forward would
    // remount this page and drop the reader's focus.
    const engrams = engramsRoute(domain);
    if (pathname === engrams) {
      setParams(updated);
    } else {
      const query = updated.toString();
      void navigate(query === "" ? engrams : `${engrams}?${query}`);
    }
  }
  return { path, filters, browse, filtering, listingOrder, apply };
}

/**
 * The domain's front page (spec 5.3): its MANIFEST, read as a page. The
 * MANIFEST's H1 is the eyebrow over the domain's name, its first paragraph
 * the lead, and its sections the prose, When to Use first. The sections an
 * agent's tools read - Provisioning, Tag Aliases and any later one the
 * server's registry knows - are cut from the prose and shown once in the
 * rail, beside what the domain holds, the ways into its settings and what
 * was recorded here lately. Below the prose, its engrams.
 *
 * Members, review mode, the policies, the archive round trip and the danger
 * zone live on the domain's settings screen (`DomainSettings`).
 */
function DomainPage({
  domain,
  folders,
  manifestPermalink,
}: {
  domain: string;
  folders: string[];
  /** The MANIFEST's permalink, off the root tree, or null for none. */
  manifestPermalink: string | null;
}) {
  const { capabilities } = useAuth();
  const { canWrite, domainLocked } = useDomainWritable(domain);
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  /**
   * What the page the reader came from did before it stopped existing: the
   * engram screen hands a discarded path over in the navigation state, since
   * there is nothing left at that address to read it back from.
   */
  const arrived =
    (useLocation().state as { discarded?: string } | null)?.discarded ?? null;
  const { path, filters, browse, filtering, listingOrder, apply } =
    useListingState();
  const [creating, setCreating] = useState(false);
  const [importing, setImporting] = useState(false);
  const [renamingOpen, setRenamingOpen] = useState(false);
  const renameReasonId = useId();
  const bannerRenameReasonId = useId();
  const writeReasonId = useId();
  /**
   * A rename this reader just made, on the address it landed on: the report
   * waits for this page in the query cache rather than in the navigation's
   * own state (`RenameDomainDialog`'s module doc says why), keyed by the
   * domain this page is now showing.
   *
   * Taken into this component's OWN state, once, rather than read from the
   * cache on every render: browsing into a folder, opening an engram or its
   * editor all unmount this page, so a report read from the cache on a later
   * mount would show a rename that did not just happen. The `useState`
   * initializer seeds it at this instance's first render; the effect below
   * empties the cache slot on every commit that shows a new domain, so no
   * other mount can find anything left to replay.
   *
   * `renamedFor` carries the domain the state was captured for, because a
   * rename itself lands on the SAME route with a new param, so React reuses
   * this instance rather than remounting it. Comparing it against the
   * render's own `domain` and calling `setState` during render when they
   * disagree is React's documented "adjusting state when a prop changes"
   * pattern, StrictMode-safe, and it is what makes two renames in a row show
   * only the second summary.
   */
  const [renamedFor, setRenamedFor] = useState<{
    domain: string;
    report: RenameReport | null;
  }>(() => ({
    domain,
    report:
      queryClient.getQueryData<RenameReport>(renameReportKey(domain)) ?? null,
  }));
  if (renamedFor.domain !== domain) {
    setRenamedFor({
      domain,
      report:
        queryClient.getQueryData<RenameReport>(renameReportKey(domain)) ?? null,
    });
  }
  const renamed = renamedFor.report;
  /**
   * What finishing a MANIFEST rename did. Kept here, not in the banner,
   * because the banner is gone as soon as the question is.
   */
  const [finished, setFinished] = useState<{
    domain: string;
    result: ManifestRenameResult;
  } | null>(null);
  useEffect(() => {
    queryClient.removeQueries({ queryKey: renameReportKey(domain) });
  }, [domain, queryClient]);

  const {
    summary,
    members,
    own,
    isRenaming,
    renameDisabledReason,
    writeDisabledReason,
  } = useDomainRights(domain);
  const manifest = useQuery({
    queryKey: manifestKey(domain),
    queryFn: () => fetchManifest(domain),
  });
  const tags = useQuery({
    queryKey: vocabularyKey(domain),
    queryFn: () => fetchTags(domain),
  });
  // The count beside the Engrams heading is the list's own total: the same
  // key the list below pages under, so one request serves both.
  const rootListing = useInfiniteQuery({
    queryKey: domainEngramsKey(domain, browse, listingOrder),
    queryFn: ({ pageParam }) =>
      fetchDomainEngrams(domain, browse, pageParam, listingOrder),
    initialPageParam: 1,
    getNextPageParam: (last: EngramPage) =>
      hasNextPage(last) ? last.page + 1 : undefined,
  });
  const engramTotal = rootListing.data?.pages[0]?.total ?? null;
  // The Reflection's page count for the rail, read only where this caller
  // reads the Reflection here: the tree the Reflection's own screens read,
  // under the same key.
  const reflection = useReflectionStatus(domain).data;
  const pagesTree = useQuery({
    queryKey: reflectionTreeKey(domain),
    queryFn: () => fetchReflectionTree(domain),
    enabled: reflection?.state === "on" && reflection.right !== "none",
  });
  const pages =
    pagesTree.data === undefined ? null : countPages(pagesTree.data.nodes);
  // Off the listing every screen already reads, and deliberately not off the
  // domain's sync status, which carries the same count: that route is gated
  // with the share verbs, so a plain member of a reviewing domain could not
  // reach it, and their own count is exactly what this line is for. A domain
  // that takes changes directly carries no count at all, which is null here.
  const myDrafts = summary?.review == null ? null : summary.myDrafts;

  // Editing the MANIFEST is offered on a read that landed, or on a domain
  // that has no MANIFEST yet, which is precisely what an admin opens the
  // editor to fix. Only a refused read is nothing to edit, and a read still
  // in flight is nothing to edit yet. Named once so the doors cannot drift.
  const manifestEditable =
    manifest.data !== undefined || isMissing(manifest.error);
  const mayEditManifest =
    capabilities.canAdminister && canWrite && manifestEditable;
  const onStart =
    mayEditManifest && !isRenaming
      ? (section: string) => {
          void navigate(manifestEditRoute(domain), {
            state: { seedSection: section },
          });
        }
      : null;
  const mayImport = capabilities.canAdminister && canWrite;

  // The writes this screen offers, on the palette under the gates the
  // controls are under. Every write row drops out while renaming: the
  // keyboard route does not get to reach a control the page shows disabled.
  // Unregistering lives with the danger zone on the settings screen; the row
  // goes there with the question already asked, never skipping it.
  const commands = useMemo<readonly PaletteCommand[]>(() => {
    const rows: PaletteCommand[] = [];
    if (canWrite && !isRenaming) {
      rows.push({
        id: "create",
        title: "New engram",
        run: () => {
          setCreating(true);
        },
      });
    }
    if (capabilities.canAdminister) {
      if (canWrite && manifestEditable && !isRenaming) {
        rows.push({
          id: "manifest-edit",
          title: "Edit MANIFEST",
          run: () => {
            void navigate(manifestEditRoute(domain));
          },
        });
      }
      rows.push({
        id: "download-archive",
        title: "Download archive",
        // The address, navigated: the download is a cookie-authenticated GET
        // that the browser saves on its own. Not a write, so it is not
        // withheld while renaming.
        run: () => {
          navigateTo(archiveDownloadUrl(domain));
        },
      });
      if (canWrite && !isRenaming) {
        rows.push({
          id: "import-archive",
          title: "Import archive",
          run: () => {
            setImporting(true);
          },
        });
      }
    }
    if (own && !isRenaming) {
      rows.push({
        id: "unregister-domain",
        title: "Unregister domain",
        run: () => {
          void navigate(domainSettingsRoute(domain, "danger-zone"), {
            state: { unregister: true },
          });
        },
      });
    }
    return rows.length === 0 ? NO_COMMANDS : rows;
  }, [
    own,
    canWrite,
    capabilities.canAdminister,
    domain,
    isRenaming,
    manifestEditable,
    navigate,
  ]);
  useRegisterCommands(commands);

  const sections = manifest.data?.sections ?? null;
  const keys = useMemo(() => configKeys(sections), [sections]);
  const prose = useMemo(
    () =>
      manifest.data === undefined
        ? null
        : splitManifest(manifest.data.markdown, keys),
    [manifest.data, keys],
  );

  // The MANIFEST's H1 over the domain's name, unless it only says that name
  // again, as the scaffold every new domain starts from (`# <name>`) does.
  const eyebrow =
    prose?.eyebrow == null ||
    prose.eyebrow === "" ||
    sectionKey(prose.eyebrow) === sectionKey(domain) ||
    (summary?.canonicalName != null &&
      sectionKey(prose.eyebrow) === sectionKey(summary.canonicalName))
      ? null
      : prose.eyebrow;
  // The MANIFEST is the page itself, so it is not listed again below it.
  // The root tree says whether there is one, so every count on the page
  // leaves it out the same way from the first render.
  const hasManifest = manifestPermalink !== null;
  const listedTotal =
    engramTotal === null
      ? null
      : Math.max(0, engramTotal - (hasManifest ? 1 : 0));

  return (
    <div
      data-print-column=""
      className="grid gap-x-14 gap-y-10 lg:grid-cols-[minmax(0,1fr)_18.75rem]"
    >
      {/* The lead sits close under the heading, as in the mockup. */}
      <div className="flex min-w-0 flex-col gap-3">
        <header>
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              {eyebrow !== null && (
                <p className="mb-1 text-sm font-semibold text-accent-700 dark:text-accent-300">
                  <InlineMarkdown source={eyebrow} />
                </p>
              )}
              <div className="flex flex-wrap items-center gap-2">
                <h1 className="text-display">{domain}</h1>
                {/*
                  Siblings of the heading rather than inside it: the
                  heading's accessible name stays exactly the domain's name.
                  Off the listing the sidebar and the home cards badge from,
                  so the three cannot disagree.
                */}
                {summary?.private === true && (
                  <Chip variant="accent">private</Chip>
                )}
                {domainLocked && <DomainLockChip />}
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              {mayEditManifest && (
                <EditLink
                  to={manifestEditRoute(domain)}
                  label="Edit MANIFEST"
                  disabledReasonId={
                    writeDisabledReason !== undefined
                      ? writeReasonId
                      : undefined
                  }
                  onPointerEnter={prefetchManifestEditor}
                  onFocus={prefetchManifestEditor}
                />
              )}
              <DomainActions
                domain={domain}
                own={own}
                renameDisabledReasonId={
                  renameDisabledReason !== undefined
                    ? renameReasonId
                    : undefined
                }
                mayDownload={capabilities.canAdminister}
                mayImport={mayImport}
                importDisabledReasonId={
                  writeDisabledReason !== undefined ? writeReasonId : undefined
                }
                onRename={() => {
                  setRenamingOpen(true);
                }}
                onImport={() => {
                  setImporting(true);
                }}
              />
            </div>
          </div>
          {/*
            The reasons the controls above are shut, outside the menu's
            portal so the ids always resolve.
          */}
          {own && renameDisabledReason !== undefined && (
            <span id={renameReasonId} className="sr-only">
              {renameDisabledReason}
            </span>
          )}
          {writeDisabledReason !== undefined && (
            <span id={writeReasonId} className="sr-only">
              {writeDisabledReason}
            </span>
          )}
          <ElsewhereWithoutLink domain={domain} />
          {/*
            The name this domain's content declares, when it reads differently
            from the local name the heading already said. Former names follow
            the same rule: nothing to say when there are none.
          */}
          {summary !== undefined &&
            summary.canonicalName !== null &&
            summary.canonicalName !== domain && (
              <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
                {`Known everywhere as ${summary.canonicalName}`}
              </p>
            )}
          {summary !== undefined && summary.aliases.length > 0 && (
            <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
              {`Former names: ${summary.aliases.join(", ")}`}
            </p>
          )}
          {isRenaming && (
            <p
              role="status"
              className="mt-1 text-sm text-slate-500 dark:text-slate-400"
            >
              Renaming...
            </p>
          )}
          {/*
            What the page somebody came from did before it stopped existing.
            A discarded addition or draft takes its own page with it, so the
            engram screen sends the reader here with the sentence.
          */}
          {arrived !== null && (
            <p
              role="status"
              className="mt-1 text-sm text-slate-600 dark:text-slate-300"
            >
              {`Discarded ${arrived}.`}
            </p>
          )}
          {/*
            What the reader is holding here that no share would pick up: a
            reviewing domain takes every write into its author's own draft.
            Drawn only where there is something to say.
          */}
          {myDrafts !== null && myDrafts > 0 && (
            <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
              {`You have ${plural(myDrafts, "draft change", "draft changes")} here, waiting to be shared.`}
            </p>
          )}
        </header>

        {renamed !== null && <RenameSummary report={renamed} />}
        {finished !== null && finished.domain === domain && (
          <ManifestRenameSummary result={finished.result} />
        )}

        {/*
          The MANIFEST's own name changed by hand and nobody has answered yet.
          Only for somebody who may rename the domain, because both answers
          go through the rename gate.
        */}
        {own && summary?.renamePending != null && (
          <PendingRenameBanner
            domain={domain}
            from={summary.renamePending.from}
            to={summary.renamePending.to}
            disabledReason={renameDisabledReason}
            onFinished={(result) => setFinished({ domain, result })}
            reasonId={renameReasonId}
          />
        )}

        {/*
          Another domain here already answers to this one's own canonical
          name, so a link spelled with it reaches that domain instead. Shown to
          every reader, with the rename action withheld from anybody who is
          not the owner or an admin.
        */}
        {summary !== undefined && summary.shadowed && (
          <div
            role="status"
            className="flex flex-wrap items-center justify-between gap-3 rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-800 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-300"
          >
            <p>
              {`This domain calls itself '${summary.canonicalName ?? domain}', but '${summary.canonicalName ?? domain}' is another domain here, so links that name '${summary.canonicalName ?? domain}' reach that one. Rename one of them to line them up.`}
            </p>
            {own && (
              <>
                <button
                  type="button"
                  aria-describedby={
                    renameDisabledReason !== undefined
                      ? bannerRenameReasonId
                      : undefined
                  }
                  aria-disabled={renameDisabledReason !== undefined}
                  onClick={() => {
                    if (renameDisabledReason !== undefined) {
                      return;
                    }
                    setRenamingOpen(true);
                  }}
                  className={`${BUTTON.secondary} aria-disabled:cursor-default aria-disabled:opacity-50 aria-disabled:hover:bg-transparent dark:aria-disabled:hover:bg-transparent`}
                >
                  Rename
                </button>
                {renameDisabledReason !== undefined && (
                  <span id={bannerRenameReasonId} className="sr-only">
                    {renameDisabledReason}
                  </span>
                )}
              </>
            )}
          </div>
        )}

        {/*
          Team domains: the sync and the proposals, as compact banners above
          the lead. Only for a session that may share, because the endpoints
          behind them are gated the same way; each draws nothing on a domain
          with no origin.
        */}
        {capabilities.canShare && <TeamBanners domain={domain} />}

        <ManifestProsePanel
          domain={domain}
          manifest={manifest.data}
          prose={prose}
          pending={manifest.isPending}
          error={manifest.error}
          onStart={onStart}
        />
      </div>

      <div className="min-w-0">
        <EngramsSection
          domain={domain}
          path={path}
          filters={filters}
          browse={browse}
          filtering={filtering}
          listingOrder={listingOrder}
          folders={folders}
          tags={tags.data ?? []}
          creating={creating}
          onCreatingChange={setCreating}
          onApply={apply}
          renaming={isRenaming}
          total={listedTotal}
          manifestPermalink={manifestPermalink}
        />
      </div>

      {/*
        Last in the document, so a phone reads the prose, then the engrams,
        then the rail; beside both from lg up.
      */}
      <div className="min-w-0 lg:col-start-2 lg:row-span-2 lg:row-start-1">
        <DomainRail
          domain={domain}
          summary={summary}
          pages={pages}
          sections={sections}
          config={prose?.config ?? []}
          members={members.data}
          showReviewMode={capabilities.canShare}
          showBackup={capabilities.canAdminister}
          hasManifest={hasManifest}
        />
      </div>

      {renamingOpen && (
        <RenameDomainDialog
          domain={domain}
          onClose={() => {
            setRenamingOpen(false);
          }}
        />
      )}
      {importing && (
        <ImportArchiveDialog
          domain={domain}
          onClose={() => {
            setImporting(false);
          }}
        />
      )}
    </div>
  );
}

/**
 * The team's sync and proposals as compact banners above the lead. On a
 * phone they fold behind one line that says what is waiting, so the
 * MANIFEST is still the first thing on the screen; from `md` up they are
 * always open. Anything to warn about - a refused or failed check, no
 * connection, conflicts, declined proposals, a failed Sync now - keeps them
 * open on a phone too, where the cards announce the warning themselves
 * (alerts and statuses), because a fold must never hide a problem. The status read is the sync card's own, under its
 * key, so the line costs no request; a domain with no origin draws nothing.
 */
function TeamBanners({ domain }: { domain: string }) {
  const [open, setOpen] = useState(false);
  const panelId = useId();
  const status = useQuery({
    queryKey: syncStatusKey(domain),
    queryFn: () => fetchSyncStatus(domain),
    retry: false,
  });
  const pulls = useMutationState({
    filters: { mutationKey: syncPullKey(domain) },
    select: (mutation) => mutation.state.status,
  });
  const sync = status.data ?? null;
  const warned = hasTeamWarning(
    sync,
    status.error !== null && !isMissing(status.error),
    pulls[pulls.length - 1] === "error",
  );
  const unfolded = open || sync === null || warned;
  // Siblings in the page's column rather than a box of their own, so an
  // origin-less domain (both cards draw nothing) leaves no gap behind.
  return (
    <>
      {/*
        With a warning standing the panel is open and its own card says it,
        so no amber summary line repeats it above; the line is only for a
        folded panel, and a folded panel has no warning to repeat.
      */}
      {sync !== null && !warned && (
        <button
          type="button"
          aria-expanded={open}
          aria-controls={panelId}
          onClick={() => {
            setOpen(!open);
          }}
          className={`flex items-center justify-between gap-2 rounded-lg border border-hair bg-chrome px-4 py-2.5 text-left text-sm md:hidden ${FOCUS_RING}`}
        >
          <span className="min-w-0">
            <span className="font-semibold">Team sync</span>{" "}
            <span className="text-muted">
              {`· ${plural(sync.localChanges, "local change", "local changes")}, ${plural(sync.openProposals, "open proposal", "open proposals")}`}
            </span>
          </span>
          <ChevronDown
            aria-hidden="true"
            size={16}
            className={`shrink-0 text-muted transition-transform ${open ? "rotate-180" : ""}`}
          />
        </button>
      )}
      <div
        id={panelId}
        className={`flex-col gap-3 empty:hidden md:flex md:empty:hidden ${unfolded ? "flex" : "hidden"}`}
      >
        <SyncCard domain={domain} />
        <ProposalsCard domain={domain} />
      </div>
    </>
  );
}

/**
 * Whether anything is wrong that the team banners must not fold away: a
 * refused or failed check, no connection, conflicts, declined proposals or a
 * failed Sync now.
 */
function hasTeamWarning(
  sync: SyncStatus | null,
  refused: boolean,
  pullFailed: boolean,
): boolean {
  return (
    refused ||
    pullFailed ||
    (sync !== null &&
      (sync.probeError !== null ||
        sync.connected === false ||
        sync.conflicts > 0 ||
        sync.declinedProposals > 0))
  );
}

/** How many pages a Reflection tree holds, folders walked. */
function countPages(nodes: TreeNode[]): number {
  return nodes.reduce(
    (sum, node) =>
      sum + (node.kind === "folder" ? countPages(node.children) : 1),
    0,
  );
}

/**
 * The front page's one "More actions" menu: Rename domain, the archive round
 * trip and the way to the domain's settings. Each row keeps the rights rule
 * it had as a button. A row that is shut right now stays in the menu and
 * says why (`aria-disabled` with the reason as its description) rather than
 * vanishing, the trade every control on this page makes.
 */
function DomainActions({
  domain,
  own,
  renameDisabledReasonId,
  mayDownload,
  mayImport,
  importDisabledReasonId,
  onRename,
  onImport,
}: {
  domain: string;
  /** An admin, or a private domain's owner: who may rename. */
  own: boolean;
  renameDisabledReasonId: string | undefined;
  mayDownload: boolean;
  mayImport: boolean;
  importDisabledReasonId: string | undefined;
  onRename: () => void;
  onImport: () => void;
}) {
  return (
    <MoreActions>
      {own && (
        <ShutItem
          label="Rename domain"
          icon={TextCursorInput}
          reasonId={renameDisabledReasonId}
          onSelect={onRename}
        />
      )}
      {mayDownload && (
        <DropdownMenu.Item asChild className={ITEM_CLASSES}>
          {/*
            An anchor: the archive route is a cookie-authenticated GET, so the
            browser saves the file itself.
          */}
          <a href={archiveDownloadUrl(domain)} download>
            <Download aria-hidden="true" size={16} strokeWidth={1.75} />
            Download archive
          </a>
        </DropdownMenu.Item>
      )}
      {mayImport && (
        <ShutItem
          label="Import archive"
          icon={Upload}
          reasonId={importDisabledReasonId}
          onSelect={onImport}
        />
      )}
      {(own || mayDownload || mayImport) && <ActionSeparator />}
      <DropdownMenu.Item asChild className={ITEM_CLASSES}>
        <Link to={domainSettingsRoute(domain)} {...PREFETCH.domainSettings}>
          <Settings aria-hidden="true" size={16} strokeWidth={1.75} />
          Domain settings
        </Link>
      </DropdownMenu.Item>
    </MoreActions>
  );
}

/** A menu row that may be shut, and says why when it is. */
function ShutItem({
  label,
  icon: Icon,
  reasonId,
  onSelect,
}: {
  label: string;
  icon: LucideIcon;
  reasonId: string | undefined;
  onSelect: () => void;
}) {
  const shut = reasonId !== undefined;
  return (
    <DropdownMenu.Item
      className={`${ITEM_CLASSES} aria-disabled:cursor-default aria-disabled:opacity-50`}
      aria-disabled={shut || undefined}
      aria-describedby={reasonId}
      onSelect={(event) => {
        if (shut) {
          event.preventDefault();
          return;
        }
        onSelect();
      }}
    >
      <Icon aria-hidden="true" size={16} strokeWidth={1.75} />
      {label}
    </DropdownMenu.Item>
  );
}

/**
 * The banner for a rename the MANIFEST made: finish it (respell the links)
 * or keep the links as they are. A finish hands its report up, because the
 * banner is gone once the listing is read again.
 */
function PendingRenameBanner({
  domain,
  from,
  to,
  disabledReason,
  onFinished,
  reasonId,
}: {
  domain: string;
  from: string;
  to: string;
  disabledReason: string | undefined;
  onFinished: (result: ManifestRenameResult) => void;
  /** The id of the element that already says why renaming is shut. */
  reasonId: string;
}) {
  const queryClient = useQueryClient();
  const [failure, setFailure] = useState<string | null>(null);
  const answer = useMutation({
    mutationFn: async (finish: boolean) => {
      if (finish) {
        onFinished(await finishManifestRename(domain));
      } else {
        await keepManifestRename(domain);
      }
    },
    onMutate: () => setFailure(null),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY }),
    onError: (error) =>
      setFailure(
        error instanceof ApiProblem && error.status === 409
          ? "Someone changed the name again. Reload to see the new question."
          : problemDetail(error),
      ),
  });
  const blocked = disabledReason !== undefined || answer.isPending;
  const button = `${BUTTON.secondary} aria-disabled:cursor-default aria-disabled:opacity-50 aria-disabled:hover:bg-transparent dark:aria-disabled:hover:bg-transparent`;
  return (
    <div
      role="status"
      className="flex flex-wrap items-center justify-between gap-3 rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-800 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-300"
    >
      <p>{`This domain was renamed from ${from} to ${to} in its MANIFEST.`}</p>
      <button
        type="button"
        aria-disabled={blocked}
        aria-describedby={disabledReason !== undefined ? reasonId : undefined}
        onClick={() => {
          if (!blocked) {
            answer.mutate(true);
          }
        }}
        className={button}
      >
        Finish the rename
      </button>
      <button
        type="button"
        aria-disabled={blocked}
        aria-describedby={disabledReason !== undefined ? reasonId : undefined}
        onClick={() => {
          if (!blocked) {
            answer.mutate(false);
          }
        }}
        className={button}
      >
        Keep links as they are
      </button>
      {failure !== null && (
        <p role="alert" className="basis-full">
          {failure}
        </p>
      )}
    </div>
  );
}

/** What a rename this reader just made did, on the address it landed on. */
function RenameSummary({ report }: { report: RenameReport }) {
  return (
    <div
      role="status"
      className="flex flex-col gap-1 rounded bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:bg-slate-900 dark:text-slate-300"
    >
      <p>{`Renamed from ${report.previous}.`}</p>
      {report.manifestDraft && <p>The MANIFEST change waits for review.</p>}
      {report.note !== null && <p>{report.note}</p>}
      <RenameRows rewritten={report.rewritten} leftBehind={report.leftBehind} />
    </div>
  );
}

/** The rewritten and left-behind rows a rename or a finish reports. */
function RenameRows({
  rewritten,
  leftBehind,
}: Pick<ManifestRenameResult, "rewritten" | "leftBehind">) {
  return (
    <>
      {rewritten.map((row) => (
        <p key={row.domain}>
          {`${row.domain}: ${plural(row.engrams, "engram", "engrams")} rewritten, ${plural(row.references, "reference", "references")}.`}
        </p>
      ))}
      {leftBehind.length > 0 && (
        <div>
          {/*
            Two different reasons share this list: a domain the caller could
            only read, and one whose rewrite failed, where `reason` says why.
          */}
          <p>Links left as they were:</p>
          <ul className="list-disc pl-5">
            {leftBehind.map((row) => (
              <li key={`${row.domain}:${row.path}`}>
                {`${row.domain}/${row.path} (${plural(row.references, "reference", "references")})${row.reason !== null ? `: ${row.reason}` : ""}`}
              </li>
            ))}
          </ul>
        </div>
      )}
    </>
  );
}

/** What finishing a MANIFEST rename did to the links, not to the name. */
function ManifestRenameSummary({ result }: { result: ManifestRenameResult }) {
  const nothing =
    result.rewritten.length === 0 && result.leftBehind.length === 0;
  return (
    <div
      role="status"
      className="flex flex-col gap-1 rounded bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:bg-slate-900 dark:text-slate-300"
    >
      <p>{`Links now use ${result.to} instead of ${result.from}.`}</p>
      {nothing && <p>No links needed a change.</p>}
      <RenameRows rewritten={result.rewritten} leftBehind={result.leftBehind} />
    </div>
  );
}

/**
 * The MANIFEST as the page: the lead at 18px, then When to Use with the line
 * that says what agents do with it, then every other prose section in file
 * order. A routing section the MANIFEST lacks says what it is for and, for a
 * reader who may edit, offers to start it.
 */
function ManifestProsePanel({
  domain,
  manifest,
  prose,
  pending,
  error,
  onStart,
}: {
  domain: string;
  manifest: ManifestView | undefined;
  prose: ManifestProse | null;
  pending: boolean;
  error: Error | null;
  /** Seed a section into the editor, or null for a reader who may not. */
  onStart: ((section: string) => void) | null;
}) {
  if (pending) {
    return (
      <Loading label="Loading the manifest" subject={`${domain} manifest`} />
    );
  }
  // A refusal is announced before the gap below is, because an errored read
  // carries no markdown either and would otherwise read as a domain that
  // never wrote one. Only a 404 is that gap.
  if (error !== null && !isMissing(error)) {
    return (
      <p
        role="alert"
        className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
      >
        {problemDetail(error)}
      </p>
    );
  }
  const empty =
    isMissing(error) ||
    manifest === undefined ||
    manifest.missing ||
    manifest.markdown.trim() === "";
  const sections = manifest?.sections ?? null;
  const whenToUse =
    prose?.sections.filter((s) => s.key === WHEN_TO_USE_KEY) ?? [];
  const rest = prose?.sections.filter((s) => s.key !== WHEN_TO_USE_KEY) ?? [];
  const missing = (name: string) =>
    sections !== null && sections.missing.includes(name);
  const routingHint =
    sections === null
      ? null
      : sections.routing === "when_to_use"
        ? "Agents read this to decide when to use this domain."
        : sections.routing === "scope"
          ? "Agents read Scope to decide when to use this domain, because When to Use is empty."
          : "Agents cannot route here until When to Use has a bullet.";
  // The ids the document renderer gives its H2s, in the order they are drawn
  // here, so a section is addressed the same way on every page.
  const slug = slugAssigner();
  const whenToUseHeading = whenToUse[0]?.heading ?? "When to Use";
  const whenToUseId =
    whenToUse.length > 0 || missing("When to Use")
      ? slug(headingText(whenToUseHeading))
      : "";
  const scopeId = missing("Scope") ? slug("Scope") : "";
  const restIds = rest.map((section) => slug(headingText(section.heading)));
  return (
    <article aria-label="MANIFEST" className="flex flex-col gap-8">
      {empty && (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          This domain has no MANIFEST yet, so nothing tells an agent what it is
          for.
        </p>
      )}
      {empty && sections !== null && onStart !== null && (
        <button
          type="button"
          className={`self-start ${BUTTON.primary}`}
          onClick={() => {
            onStart(WHOLE_MANIFEST);
          }}
        >
          Create a MANIFEST
        </button>
      )}
      {prose !== null && prose.lead !== null && (
        <div
          // The lead (spec 3.3): 18px at 1.6 in ink-3, through the lg token
          // so large text grows it with the rest of the document.
          className="[&_.text-body]:text-lg [&_.text-body]:leading-[1.6] [&_.text-body]:text-ink-3 [&_p:first-of-type]:mt-0"
        >
          <Markdown source={prose.lead} domain={domain} />
        </div>
      )}
      {prose !== null && prose.intro !== "" && (
        <div>
          <Markdown source={prose.intro} domain={domain} />
        </div>
      )}
      {(whenToUse.length > 0 || missing("When to Use")) && (
        <ProseSection
          id={whenToUseId}
          domain={domain}
          heading={whenToUseHeading}
          hint={routingHint ?? undefined}
          body={whenToUse.map((s) => s.body).join("\n\n")}
        >
          {missing("When to Use") && sections !== null && (
            <EmptySection
              starter={starterFor(sections, "When to Use")}
              fallback="No When to Use section"
              onStart={onStart}
            />
          )}
        </ProseSection>
      )}
      {whenToUse.length === 0 &&
        !missing("When to Use") &&
        routingHint !== null &&
        !empty && (
          // A routing state with no When to Use heading to hang it on.
          <p className="text-sm text-muted">{routingHint}</p>
        )}
      {missing("Scope") && sections !== null && (
        <ProseSection id={scopeId} domain={domain} heading="Scope" body="">
          <EmptySection
            starter={starterFor(sections, "Scope")}
            fallback="No Scope section"
            onStart={onStart}
          />
        </ProseSection>
      )}
      {rest.map((section, index) => (
        <ProseSection
          // A heading may repeat; its place in the file does not.
          key={`${section.key}:${String(index)}`}
          id={restIds[index] ?? ""}
          domain={domain}
          heading={section.heading}
          body={section.body}
        />
      ))}
    </article>
  );
}

/**
 * A heading's text as it reads, for its slug: inline markup, a link's target
 * and a status label's syntax dropped, the way the rendered heading's text
 * has none of them.
 */
function headingText(heading: string): string {
  return statusLabelWords(heading)
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[*`~]/g, "");
}

/** One section of the MANIFEST, under its own heading as written. */
function ProseSection({
  id,
  domain,
  heading,
  hint,
  body,
  children,
}: {
  /** The heading's id, the slug the document renderer would give it. */
  id: string;
  domain: string;
  heading: string;
  hint?: string | undefined;
  body: string;
  children?: ReactNode;
}) {
  return (
    <section aria-labelledby={id} className="measured">
      <div className="mb-3 flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h2
          id={id}
          className="scroll-mt-20 text-[21px] leading-7 font-[620] text-ink"
        >
          <InlineMarkdown source={heading} />
        </h2>
        {hint !== undefined && <p className="text-[13px] text-muted">{hint}</p>}
      </div>
      {body !== "" && <Markdown source={body} domain={domain} />}
      {children}
    </section>
  );
}

/**
 * One folder: its path, its size, and its engrams.
 *
 * Nothing of the domain's furniture is here. A folder is browsed rather than
 * administered, so the archive round trip, unregistering, the team cards and
 * the MANIFEST all stay on the domain page, one link up the trail.
 */
function FolderPage({
  domain,
  path,
  folders,
}: {
  domain: string;
  path: string;
  folders: string[];
}) {
  const { canWrite } = useDomainWritable(domain);
  const { filters, browse, filtering, listingOrder, apply } = useListingState();
  const [creating, setCreating] = useState(false);
  const tags = useQuery({
    queryKey: vocabularyKey(domain),
    queryFn: () => fetchTags(domain),
  });
  // The folder's size is the listing's own total, which counts the subtree.
  // Read under the same key the list below pages under, so the two are one
  // cache entry and one request while the folder is what is listed; under a
  // filter the list below spans the domain and this is the one read of the
  // folder itself, because the count under the heading is a fact about the
  // folder whatever the list beneath it shows.
  const folderListing = useInfiniteQuery({
    queryKey: domainEngramsKey(domain, browse, listingOrder),
    queryFn: ({ pageParam }) =>
      fetchDomainEngrams(domain, browse, pageParam, listingOrder),
    initialPageParam: 1,
    getNextPageParam: (last: EngramPage) =>
      hasNextPage(last) ? last.page + 1 : undefined,
  });
  const total = folderListing.data?.pages[0]?.total ?? null;

  const commands = useMemo<readonly PaletteCommand[]>(
    () =>
      canWrite
        ? [
            {
              id: "create",
              title: "New engram",
              run: () => {
                setCreating(true);
              },
            },
          ]
        : NO_COMMANDS,
    [canWrite],
  );
  useRegisterCommands(commands);

  return (
    <div className="flex flex-col gap-8">
      <header>
        <FolderHeading domain={domain} path={path} />
        {total !== null && (
          <p className="mt-1 text-sm text-slate-500 tabular-nums dark:text-slate-400">
            {`${plural(total, "engram", "engrams")} in this folder`}
          </p>
        )}
      </header>
      <EngramsSection
        domain={domain}
        path={path}
        filters={filters}
        browse={browse}
        filtering={filtering}
        listingOrder={listingOrder}
        folders={folders}
        tags={tags.data ?? []}
        creating={creating}
        onCreatingChange={setCreating}
        onApply={apply}
      />
    </div>
  );
}

/**
 * The folder's path as the page's heading, every step of it a link but the
 * last: the domain and each parent folder go to their pages, and the folder
 * itself is where the reader is. The slashes are text rather than hidden
 * glyphs, so the heading reads "eng / notes / deep" to a screen reader too,
 * which is the folder's name in the only spelling this app has for one.
 *
 * Laid out inline rather than as a flex row, and the spaces around each
 * slash are text nodes of the heading itself: an accessible name is built by
 * trimming what each child element contributes, so a separator wrapped in an
 * element of its own comes out "eng/notes/deep" - the name and the screen
 * saying two different things about one heading.
 */
function FolderHeading({ domain, path }: { domain: string; path: string }) {
  const segments = path.split("/");
  const link = `rounded underline underline-offset-4 hover:no-underline ${FOCUS_RING}`;
  return (
    <h1 className="text-display">
      <Link to={engramsRoute(domain)} className={link}>
        {domain}
      </Link>
      {segments.map((segment, index) => {
        const upto = segments.slice(0, index + 1).join("/");
        const last = index === segments.length - 1;
        return (
          <Fragment key={upto}>
            {" "}
            <span className="text-slate-400">/</span>{" "}
            {last ? (
              <span>{segment}</span>
            ) : (
              <Link to={folderRoute(domain, upto)} className={link}>
                {segment}
              </Link>
            )}
          </Fragment>
        );
      })}
    </h1>
  );
}

/**
 * The engrams of a domain or of a folder: the heading with the count, Filter
 * and New engram, the folders as chips, and the list as rows with its own
 * row above it.
 *
 * Shared by both pages so a folder lists exactly the way its domain does. The
 * frontmatter form waits behind Filter (spec 5.3): it is open whenever a
 * filter is in force, so a filtered address never hides what filters it. The
 * order menu sits on the row directly above the list. The create dialog's
 * open state is the page's rather than this section's, because the page's
 * palette row opens the same dialog.
 */
function EngramsSection({
  domain,
  path,
  filters,
  browse,
  filtering,
  listingOrder,
  folders,
  tags,
  creating,
  onCreatingChange,
  onApply,
  renaming = false,
  total = null,
  manifestPermalink = null,
}: {
  domain: string;
  /** The folder being browsed, empty at the domain's root. */
  path: string;
  filters: EngramFilters;
  browse: EngramFilters;
  filtering: boolean;
  /** The same order as the listing request and the cache key carry it. */
  listingOrder: ListingOrder;
  /** The subfolders of `path`, from the tree. */
  folders: string[];
  tags: TagCount[];
  creating: boolean;
  onCreatingChange: (creating: boolean) => void;
  /** Change the URL, which is the whole of the listing's state. */
  onApply: (next: ListingChange) => void;
  /**
   * Whether a rename has this domain paused right now. A write into a
   * paused domain waits on the server rather than failing, but a reader who
   * pressed New engram while it is renaming would sit on that wait with no
   * idea why, so the control is shut here instead. Defaults to false: the
   * folder page reads no domain summary of its own to know otherwise.
   */
  renaming?: boolean;
  /** The domain's engram count beside the heading, at the root only. */
  total?: number | null;
  /**
   * The MANIFEST's permalink at the domain's root, which the browse list
   * leaves out: the page above it is that file. An explicit filter still
   * finds it.
   */
  manifestPermalink?: string | null;
}) {
  const { canWrite } = useDomainWritable(domain);
  // The same members read the settings and the danger zone make: whether
  // GitHub decides here, and whether the domain is shared.
  const members = useQuery({
    queryKey: membersKey(domain),
    queryFn: () => fetchMembers(domain),
  });
  const sharedGithub =
    members.data !== undefined &&
    governanceOf(members.data).github &&
    members.data.visibility !== "private";
  const unlinked = useUnlinkedFromGithub(sharedGithub && !canWrite);
  const newEngramReasonId = useId();
  const filterId = useId();
  const newEngramDisabledReason = renaming ? RENAMING_REASON : undefined;
  const [filterOpen, setFilterOpen] = useState(filtering);
  const skipManifest = useMemo(
    () =>
      manifestPermalink === null
        ? undefined
        : (row: EngramRow) => row.permalink === manifestPermalink,
    [manifestPermalink],
  );
  // A filter that arrives by the URL (a back button, a shared link) opens the
  // form, so what filters the list is in view; the reader may close it again.
  const [wasFiltering, setWasFiltering] = useState(filtering);
  if (filtering !== wasFiltering) {
    setWasFiltering(filtering);
    if (filtering) {
      setFilterOpen(true);
    }
  }

  return (
    <section aria-labelledby="page-domain-engrams">
      <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-baseline gap-2">
          <h2
            id="page-domain-engrams"
            className="text-[21px] leading-7 font-[620]"
          >
            Engrams
          </h2>
          {total !== null && (
            <span className="text-sm text-muted tabular-nums">{total}</span>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <EngramsOrderMenu />
          <button
            type="button"
            aria-expanded={filterOpen}
            aria-controls={filterId}
            onClick={() => {
              setFilterOpen(!filterOpen);
            }}
            className={SECONDARY_ACTION}
          >
            <ListFilter aria-hidden="true" size={14} strokeWidth={1.75} />
            Filter
          </button>
          {canWrite && (
            <>
              <button
                type="button"
                // `aria-disabled`, not `disabled`: a keyboard user still
                // reaches the button and still hears why it will not act.
                aria-disabled={newEngramDisabledReason !== undefined}
                aria-describedby={
                  newEngramDisabledReason !== undefined
                    ? newEngramReasonId
                    : undefined
                }
                onClick={() => {
                  if (newEngramDisabledReason !== undefined) {
                    return;
                  }
                  onCreatingChange(true);
                }}
                // Secondary (spec 3.1): the filled primary is kept for the
                // step that commits a flow, which is the dialog's.
                className={SECONDARY_ACTION}
              >
                <Plus aria-hidden="true" size={14} strokeWidth={1.75} />
                New engram
              </button>
              {newEngramDisabledReason !== undefined && (
                <span id={newEngramReasonId} className="sr-only">
                  {newEngramDisabledReason}
                </span>
              )}
            </>
          )}
        </div>
      </div>
      {!canWrite && unlinked && <GithubLinkHint />}
      {creating && (
        <CreateEngramDialog
          domain={domain}
          initialFolder={path}
          onClose={() => {
            onCreatingChange(false);
          }}
        />
      )}

      <FolderNav
        path={path}
        folders={folders}
        onOpen={(next) => {
          // Opening a folder is a browse, so it leaves the frontmatter view.
          onApply({ path: next, type: null, status: null, tags: [] });
        }}
      />

      <div id={filterId} hidden={!filterOpen}>
        {filterOpen && (
          <FilterBar
            // Keyed by the filters that are actually applied, so the two typed
            // fields reset to them whenever the URL moves under this screen: a
            // back button, a shared link, or the clear button.
            key={`${filters.type ?? ""}|${filters.status ?? ""}`}
            filters={filters}
            tags={tags}
            onChange={(next) => {
              onApply(next);
            }}
          />
        )}
      </div>

      {/*
        In a folder or under a filter the scope is named above the list,
        because it is a fact about the request rather than about any page it
        answers. The order sits in the heading row, beside Filter.
      */}
      {(filtering || path !== "") && (
        <p className="py-3 text-sm text-slate-500 dark:text-slate-400">
          {filtering
            ? "Filtered across the whole domain, every folder included."
            : `Browsing ${path}, subfolders included.`}
        </p>
      )}

      {filtering ? (
        <EngramList
          queryKey={domainEngramsKey(domain, filters, listingOrder)}
          loadPage={(page) =>
            fetchDomainEngrams(domain, filters, page, listingOrder)
          }
          label={`Engrams in ${domain}`}
          emptyMessage="No engram matches these filters."
          summary={() => null}
          plain
        />
      ) : (
        <EngramList
          // The same endpoint the filtered view pages, scoped to the folder
          // instead of filtered: the key carries the scope, so opening
          // another folder starts another list.
          queryKey={domainEngramsKey(domain, browse, listingOrder)}
          loadPage={(page) =>
            fetchDomainEngrams(domain, browse, page, listingOrder)
          }
          label={`Engrams in ${domain}`}
          summary={path === "" ? () => <div className="pt-3" /> : () => null}
          emptyMessage={
            path === ""
              ? "This domain has no engrams yet."
              : "This folder has no engrams."
          }
          plain
          skip={path === "" ? skipManifest : undefined}
        />
      )}
    </section>
  );
}

/** A secondary action (spec 3.1): 32px tall, a hairline border, an icon. */
const SECONDARY_ACTION = `inline-flex h-8 items-center gap-1.5 rounded border border-slate-300 bg-white px-3 text-sm font-medium hover:bg-slate-100 aria-disabled:cursor-default aria-disabled:opacity-50 aria-disabled:hover:bg-white aria-expanded:bg-sunken dark:border-slate-700 dark:bg-transparent dark:hover:bg-slate-800 dark:aria-disabled:hover:bg-transparent ${FOCUS_RING}`;

/**
 * The subfolders of the folder being browsed, each a step further in, as
 * chips. At the domain's root "All" heads them as the current view.
 */
function FolderNav({
  path,
  folders,
  onOpen,
}: {
  path: string;
  folders: string[];
  onOpen: (path: string) => void;
}) {
  if (folders.length === 0) {
    return null;
  }
  const chip = `rounded-full border px-3 py-1 text-sm ${FOCUS_RING}`;
  return (
    <nav aria-label="Folders" className="mb-1">
      <ul className="flex flex-wrap gap-2">
        {path === "" && (
          <li>
            <span
              aria-current="page"
              className={`${chip} inline-block border-ink bg-ink text-canvas`}
            >
              All
            </span>
          </li>
        )}
        {folders.map((folder) => (
          <li key={folder}>
            <button
              type="button"
              className={`${chip} border-hair-strong hover:bg-sunken`}
              onClick={() => {
                onOpen(path === "" ? folder : `${path}/${folder}`);
              }}
            >
              {folder}
            </button>
          </li>
        ))}
      </ul>
    </nav>
  );
}

/**
 * The frontmatter filters.
 *
 * The controls are the shared ones (`FilterControls`); what this adds is what a
 * change means here, which is a filter across the whole domain rather than
 * inside the folder being browsed. There is no timeframe field: this screen
 * asks what a domain holds, and the listing endpoint filters on frontmatter
 * alone.
 */
function FilterBar({
  filters,
  tags,
  onChange,
}: {
  filters: EngramFilters;
  tags: TagCount[];
  onChange: (next: {
    type?: string | null;
    status?: string | null;
    tags?: string[];
  }) => void;
}) {
  return (
    // Set off from the folder row above it: browsing and filtering are two
    // ways of asking, and stacked flush they read as one dense block of small
    // grey labels, which is worst in dark.
    <div className="mt-4 flex flex-col gap-3">
      <FilterFields
        type={filters.type}
        status={filters.status}
        clearable={hasFilters(filters)}
        onApply={({ type, status }) => {
          onChange({ type, status });
        }}
        onClear={() => {
          onChange({ type: null, status: null, tags: [] });
        }}
      />
      <TagChips
        tags={tags}
        chosen={filters.tags}
        onChange={(next) => {
          onChange({ tags: next });
        }}
      />
    </div>
  );
}

/**
 * Where a Reflection that lives on another instance is, said in words when
 * its address is not a web address: the bar's switch has nowhere safe to link
 * its Reflection half to then, so it draws nothing, and this line is what is
 * left of the way there.
 */
function ElsewhereWithoutLink({ domain }: { domain: string }) {
  const s = useReflectionStatus(domain).data;
  if (
    s?.state !== "elsewhere" ||
    s.right === "none" ||
    (s.homeUrl !== null && isWebAddress(s.homeUrl))
  ) {
    return null;
  }
  return <HomeElsewhere url={s.homeUrl} />;
}
