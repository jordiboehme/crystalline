/**
 * One route, two pages: the domain, and one folder inside it.
 *
 * The domain page is where a domain is introduced - its MANIFEST, whole,
 * with what the core crate reads out of it; the team's sync, proposals,
 * members and review mode; and its engrams. The folder page (`?path=`) is
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
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
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
  renameReportKey,
  syncStatusKey,
} from "../api/admin";
import type { RenameReport } from "../api/admin";
import { ApiProblem, problemDetail } from "../api/client";
import { fetchManifest, manifestKey, treeQuery } from "../api/domain";
import type {
  ManifestProblem,
  ManifestSections,
  ManifestView,
  StarterStanza,
} from "../api/domain";
import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import type { EngramFilters, EngramPage, ListingOrder } from "../api/engrams";
import {
  NO_FILTERS,
  domainEngramsKey,
  fetchDomainEngrams,
  hasFilters,
  hasNextPage,
} from "../api/engrams";
import { fetchMembers, membersKey, sameAccount } from "../api/members";
import { fetchTags, vocabularyKey } from "../api/vocabulary";
import type { TagCount } from "../api/vocabulary";
import { useAuth } from "../auth/AuthContext";
import { NO_COMMANDS, useRegisterCommands } from "../commands";
import type { PaletteCommand } from "../commands";
import { BackupCard } from "../components/BackupCard";
import { CreateEngramDialog } from "../components/CreateEngramDialog";
import { DangerZoneCard } from "../components/DangerZoneCard";
import {
  READ_ONLY_REASON,
  RENAMING_REASON,
} from "../components/DestructiveAction";
import { DomainPoliciesCard } from "../components/DomainPoliciesCard";
import { EngramList } from "../components/EngramList";
import { EngramsOrderMenu } from "../components/EngramsOrderMenu";
import { FilterFields, TagChips } from "../components/FilterControls";
import { ImportArchiveDialog } from "../components/ImportArchiveDialog";
import { InlineMarkdown } from "../components/InlineMarkdown";
import { Markdown } from "../components/Markdown";
import { MembersCard } from "../components/MembersCard";
import { ProposalsCard } from "../components/ProposalsCard";
import { RenameDomainDialog } from "../components/RenameDomainDialog";
import { ReviewModeCard } from "../components/ReviewModeCard";
import { Skeleton } from "../components/Skeleton";
import { SyncCard } from "../components/SyncCard";
import { BUTTON, Chip, FOCUS_RING } from "../components/primitives";
import { orderQuery, useEngramsOrder } from "../engramsOrder";
import { frontmatterFilters } from "../filters";
import { plural } from "../format";
import {
  WHOLE_MANIFEST,
  domainRoute,
  folderRoute,
  manifestEditRoute,
} from "../paths";
import { prefetchManifestEditor } from "../prefetch";

export default function DomainHome() {
  const { domain = "" } = useParams();
  const [params] = useSearchParams();
  const path = params.get("path") ?? "";
  // The tree is what says whether the domain exists at all - a 404 from it
  // is a wrong address, not an empty shelf - and what the folder buttons on
  // both pages are drawn from. Read here, once, so neither page asks twice.
  const tree = useQuery(treeQuery(domain, path));
  if (isMissing(tree.error)) {
    return <DomainNotFound domain={domain} />;
  }
  const folders = tree.data?.folders ?? [];
  return path === "" ? (
    <DomainPage domain={domain} folders={folders} />
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
    setParams(updated);
  }
  return { path, filters, browse, filtering, listingOrder, apply };
}

/**
 * The domain: what it is for, who shares it, and what is in it.
 */
function DomainPage({
  domain,
  folders,
}: {
  domain: string;
  folders: string[];
}) {
  const { user, capabilities } = useAuth();
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
  const [confirmingUnregister, setConfirmingUnregister] = useState(false);
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
   * cache on every render - an earlier version of this did the
   * latter (a plain `getQueryData` call in the render body, cleared from an
   * effect keyed on a domain change) and that has a real gap: `DomainPage`
   * is not the only screen a reader reaches a renamed domain's engrams
   * through. Browsing into a folder swaps this component out for
   * `FolderPage` (`DomainHome`'s own conditional return, above), and
   * opening an engram or its editor lands on an entirely different
   * `<Route>` (`routes.tsx`) - both perfectly ordinary ways to read a
   * domain, and both unmount `DomainPage` outright. An effect keyed on "the
   * domain changed while THIS instance stayed mounted" never fires for any
   * of that, so the report to a domain a reader had already left through
   * one of those sat in the cache untouched - and a later, unrelated visit
   * to that same domain's home page (a fresh `DomainPage` mount, with no
   * memory of the earlier one) would read it right back out and show a
   * rename that did not just happen, for as long as the query client's
   * default `gcTime` (five minutes) kept the entry alive.
   *
   * So the state below is what actually decides what shows, and the cache
   * read only ever seeds it: the `useState` initializer runs once, at this
   * component instance's own first render, and takes whatever the cache
   * says for the domain that render shows. From then on the cache is never
   * consulted again by this instance - a later effect (below) empties its
   * slot outright, unconditionally, on every commit that shows a new domain
   * value: its own dependency array reruns it exactly then, whether that
   * commit is a fresh mount or this very instance's own `domain` prop
   * changing in place, so no OTHER mount - a fresh one after a
   * folder/engram/editor detour, or a plain revisit - can find anything left
   * to replay.
   *
   * `renamedFor` carries the domain the state was captured for alongside
   * the report, because ONE thing an initializer cannot do is fire again
   * when `domain` changes under a continuously-mounted instance - which is
   * exactly what happens the moment a rename itself lands (the dialog
   * navigates to the SAME `/d/:domain` route, just a new param, so React
   * reuses this very instance rather than remounting it). Comparing
   * `renamedFor.domain` against the render's own `domain` and calling
   * `setState` right here, during render, when they disagree is a
   * documented, StrictMode-safe pattern for exactly this ("adjusting state
   * when a prop changes" - React's own docs use the identical shape): the
   * mismatched render is never committed, React immediately re-runs this
   * function with the corrected state, and calling it a second time with
   * the same computed values (StrictMode's own extra render-phase
   * invocation) is a no-op. This is what makes two renames in a row show
   * only the second summary: the first `setRenamedFor` call already
   * captured the first report, and the second domain change repeats the
   * same adjustment for the second one, discarding the first from state
   * entirely rather than appending to it.
   *
   * The effect that empties the cache slot is deliberately unconditional -
   * it does not check whether a report was there, and it runs the same way
   * whether this mount's domain came with one or not - because its only job
   * is to guarantee nothing is left for a LATER, different mount to find.
   * StrictMode's double-invoke (setup, cleanup, setup, right after this
   * component's very first mount) calls it twice in a row; removing an
   * already-removed entry a second time does nothing.
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
  useEffect(() => {
    queryClient.removeQueries({ queryKey: renameReportKey(domain) });
  }, [domain, queryClient]);

  const listing = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
    // A rename in progress on THIS domain - this session's own, or another
    // live instance's sharing the same index - pauses it until it lands, and
    // the page has no other way to learn that it cleared: nothing here
    // pushes, so it polls. Keyed to this domain by name, not to "some domain
    // in the listing": a rename elsewhere is somebody else's page to poll.
    // Stopped rather than backed off once a fetch fails: a failing poll
    // every second would hammer a server that is already in trouble, and the
    // listing's own error state is what the rest of this screen already
    // falls back to.
    refetchInterval: (query) => {
      if (query.state.status === "error") {
        return false;
      }
      const mine = query.state.data?.domains.find(
        (entry) => entry.name === domain,
      );
      return mine?.renaming === true ? 1000 : false;
    },
  });
  const summary = listing.data?.domains.find((entry) => entry.name === domain);
  const manifest = useQuery({
    queryKey: manifestKey(domain),
    queryFn: () => fetchManifest(domain),
  });
  // The members read the danger zone makes, under the same key, so one
  // request serves both: the palette's unregister row follows the card's
  // own rule - an admin, or the owner of a private domain.
  const members = useQuery({
    queryKey: membersKey(domain),
    queryFn: () => fetchMembers(domain),
  });
  const owner = members.data?.owner ?? null;
  const canUnregister =
    capabilities.canAdminister ||
    (user !== null && owner !== null && sameAccount(owner, user.name));
  // The rename controls share this exact rule with the danger zone: an
  // instance admin, or a private domain's owner, which is the rule
  // `POST /rename` itself enforces.
  const own = canUnregister;
  const isRenaming = summary?.renaming === true;
  const renameDisabledReason = capabilities.readOnly
    ? READ_ONLY_REASON
    : isRenaming
      ? RENAMING_REASON
      : undefined;
  /**
   * Every OTHER write control on this page - New engram, Edit MANIFEST,
   * Import archive - shares this one reason rather than the rename button's
   * own: `renaming` is the one thing this task adds disabling for on those
   * controls, so this stays scoped to it rather than also taking on a
   * pre-existing read-only gap this task did not introduce (the danger zone
   * is the one card here that already handles read-only itself).
   */
  const writeDisabledReason = isRenaming ? RENAMING_REASON : undefined;
  const tags = useQuery({
    queryKey: vocabularyKey(domain),
    queryFn: () => fetchTags(domain),
  });
  // The same query the sync card makes, under the same key, and under the
  // same gate: a session that may not share must knock on nothing the server
  // would refuse it. One fact off it is what the policies card needs - the
  // branch a direct share would commit onto - and a domain with no origin,
  // or a session that may not ask, simply names no branch.
  const syncStatus = useQuery({
    queryKey: syncStatusKey(domain),
    queryFn: () => fetchSyncStatus(domain),
    retry: false,
    enabled: capabilities.canShare,
  });
  // Off the listing every screen already reads, and deliberately not off the
  // domain's sync status, which carries the same count: that route is gated
  // with the share verbs, so a plain member of a reviewing domain could not
  // reach it, and their own count is exactly what this line is for. A domain
  // that takes changes directly carries no count at all, which is null here.
  const myDrafts = summary?.review == null ? null : summary.myDrafts;

  // The writes this screen offers, on the palette under the gates the
  // buttons are under. The dialog the first opens picks its own folder from
  // the URL, so the keyboard route lands exactly where the pointer route
  // does. Unregistering follows the danger zone's own gate (an admin, or a
  // private domain's owner): the palette row does what the button does,
  // which is to ASK - the second press is the point of the control and the
  // keyboard route does not get to skip it.
  // Editing the MANIFEST is behind the same gate as its link, and under the
  // same second condition: a read that landed, or a domain that has no
  // MANIFEST yet, which is precisely what an admin opens the editor to fix.
  // Only a refused read is nothing to edit, and a read still in flight is
  // nothing to edit yet. Named once so the two doors cannot drift apart.
  const manifestLoaded = manifest.data !== undefined;
  const manifestEditable = manifestLoaded || isMissing(manifest.error);
  const commands = useMemo<readonly PaletteCommand[]>(() => {
    const rows: PaletteCommand[] = [];
    // Every write row here drops out while renaming: the keyboard route
    // does not get to reach a control the page itself shows disabled.
    if (capabilities.canWrite && !isRenaming) {
      rows.push({
        id: "create",
        title: "New engram",
        run: () => {
          setCreating(true);
        },
      });
    }
    if (capabilities.canAdminister) {
      if (manifestEditable && !isRenaming) {
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
        // that the browser saves on its own, so the keyboard route goes to the
        // same URL the anchor carries rather than reaching into the DOM to
        // press a link that may not even be rendered. Not a write, so it is
        // not withheld while renaming.
        run: () => {
          window.location.assign(archiveDownloadUrl(domain));
        },
      });
      if (!isRenaming) {
        rows.push({
          id: "import-archive",
          title: "Import archive",
          run: () => {
            setImporting(true);
          },
        });
      }
    }
    if (canUnregister && !isRenaming) {
      rows.push({
        id: "unregister-domain",
        title: "Unregister domain",
        run: () => {
          setConfirmingUnregister(true);
        },
      });
    }
    return rows.length === 0 ? NO_COMMANDS : rows;
  }, [
    canUnregister,
    capabilities.canAdminister,
    capabilities.canWrite,
    domain,
    isRenaming,
    manifestEditable,
    navigate,
  ]);
  useRegisterCommands(commands);

  return (
    <div className="flex flex-col gap-8">
      <header>
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-display">{domain}</h1>
          {/*
            A sibling of the heading rather than inside it: the heading's own
            accessible name stays exactly the domain's name, and the badge is
            a separate piece of content beside it rather than text silently
            appended to what a screen reader announces as the page's title.
            Off the listing every other chip here draws from, which is the
            same read the sidebar and the home cards badge from: one fact,
            one source, and a header that cannot disagree with the two places
            that named this domain on the way here.
          */}
          {summary?.private === true && <Chip variant="accent">private</Chip>}
          {/*
            Reached only by the domain's owner or an instance admin - the
            exact rule the danger zone's own controls follow, since it is the
            rule `POST /rename` enforces server side too. Shown disabled
            rather than withheld on a read-only instance or while a rename is
            already running, the way the danger zone shows its own controls:
            `aria-disabled` rather than `disabled`, so a keyboard user still
            reaches the control and still hears why it will not act, the same
            trade `DestructiveAction` documents.
          */}
          {own && (
            <button
              type="button"
              aria-describedby={
                renameDisabledReason !== undefined ? renameReasonId : undefined
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
              Rename domain
            </button>
          )}
        </div>
        {own && renameDisabledReason !== undefined && (
          <span id={renameReasonId} className="sr-only">
            {renameDisabledReason}
          </span>
        )}
        {/*
          The name this domain's content declares, when it reads differently
          from the local name the heading above already said - the two agree
          most of the time, and this line is silent then. Former names follow
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
          What the page somebody came from did before it stopped existing. A
          discarded addition or a discarded draft takes its own page with it,
          so the engram screen sends the reader here and hands the sentence
          over in the navigation itself; there is nothing to read back from
          the server about a file that is gone.
        */}
        {arrived !== null && (
          <p
            role="status"
            className="text-sm text-slate-600 dark:text-slate-300"
          >
            {`Discarded ${arrived}.`}
          </p>
        )}
        {summary && (
          <p className="mt-1 flex flex-wrap items-center gap-2 text-sm text-slate-500 dark:text-slate-400">
            {summary.engrams !== null && (
              <span className="tabular-nums">
                {plural(summary.engrams, "engram", "engrams")}
              </span>
            )}
            {/* The same fact wears the same chip the home card gives it. */}
            {summary.kind !== null && <Chip>{summary.kind}</Chip>}
          </p>
        )}
        {/*
          What the reader is holding here that no share would pick up. A
          reviewing domain takes every write into its author's own draft, so
          somebody arriving at this screen can have work waiting that nothing
          else on it mentions. Drawn only where there is something to say:
          zero is the ordinary state and the card below says it in full.
        */}
        {myDrafts !== null && myDrafts > 0 && (
          <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            {`You have ${plural(myDrafts, "draft change", "draft changes")} here, waiting to be shared.`}
          </p>
        )}
      </header>

      {/*
        A rename this reader just made, on the address it landed on: the
        summary in `renamedFor.report`, seeded above from the query cache the
        rename dialog filled (see this component's own module doc), since
        there is nothing left to read it back from - the address that
        answered it is gone the moment the hop lands.
      */}
      {renamed !== null && (
        <div
          role="status"
          className="flex flex-col gap-1 rounded bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:bg-slate-900 dark:text-slate-300"
        >
          <p>{`Renamed from ${renamed.previous}.`}</p>
          {renamed.manifestDraft && (
            <p>The MANIFEST change waits for review.</p>
          )}
          {renamed.rewritten.map((row) => (
            <p key={row.domain}>
              {`${row.domain}: ${plural(row.engrams, "engram", "engrams")} rewritten, ${plural(row.references, "reference", "references")}.`}
            </p>
          ))}
          {renamed.leftBehind.length > 0 && (
            <div>
              {/*
                Two different reasons share this list: a domain the caller
                could only read, where the link was never this rename's to
                touch, and one whose rewrite failed, where `reason` says why -
                so the heading names neither on its own.
              */}
              <p>Links left as they were:</p>
              <ul className="list-disc pl-5">
                {renamed.leftBehind.map((row) => (
                  <li key={`${row.domain}:${row.path}`}>
                    {`${row.domain}/${row.path} (${plural(row.references, "reference", "references")})${row.reason !== null ? `: ${row.reason}` : ""}`}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      {/*
        Another domain here already answers to this one's own canonical
        name, so a link spelled with it reaches that domain instead. Shown to
        every reader - it is a fact about how links resolve, not a secret -
        with the rename action itself withheld from anybody who is not the
        owner or an admin, exactly as the button above it is.
      */}
      {summary !== undefined && summary.shadowed && (
        <div
          role="status"
          // The chip's own caution pair (amber-800 on amber-100, amber-300 on
          // amber-950), already vetted for `CHIP_VARIANTS.caution`, lightened
          // to amber-50 for the light background: a banner's box is larger
          // than a chip and the paler wash only raises the ratio further.
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

      {renamingOpen && (
        <RenameDomainDialog
          domain={domain}
          onClose={() => {
            setRenamingOpen(false);
          }}
        />
      )}

      {/*
        Only for a session that may share, because the endpoints behind it are
        gated the same way: a screen must knock on nothing it would be refused.
        Which accounts those are is the server's answer rather than this side's
        arithmetic. The card draws nothing at all on a domain with no origin,
        which is most of them, so this is the whole of the gate the screen
        owns; an unregistered domain never reaches here.
      */}
      {capabilities.canShare && <SyncCard domain={domain} />}
      {/*
        The same gate, and the same query behind it: the card beside this one
        says how many proposals there are and this one says which they are, off
        one fetch.
      */}
      {capabilities.canShare && <ProposalsCard domain={domain} />}

      <section aria-labelledby="domain-manifest">
        <div className="mb-2 flex flex-wrap items-baseline justify-between gap-3">
          <h2 id="domain-manifest" className="text-section">
            Manifest
          </h2>
          {/*
            Offered whether the MANIFEST loaded empty or full, and offered on
            a domain that has none yet: an admin looking at nothing needs
            exactly this link to fix that, and an admin looking at prose
            needs it to change it. The same gate the editor itself enforces
            if the address is typed directly. It is withheld in two states
            only, both of them states where the panel below is not showing a
            document: while the read is in flight, and after one the server
            refused.
          */}
          {capabilities.canAdminister && manifestEditable && (
            <>
              <Link
                to={manifestEditRoute(domain)}
                aria-disabled={writeDisabledReason !== undefined}
                aria-describedby={
                  writeDisabledReason !== undefined ? writeReasonId : undefined
                }
                onClick={(event) => {
                  if (writeDisabledReason !== undefined) {
                    event.preventDefault();
                  }
                }}
                onPointerEnter={prefetchManifestEditor}
                onFocus={prefetchManifestEditor}
                className={`inline-flex items-center ${BUTTON.secondary} aria-disabled:cursor-default aria-disabled:opacity-50 aria-disabled:hover:bg-transparent dark:aria-disabled:hover:bg-transparent`}
              >
                Edit MANIFEST
              </Link>
              {writeDisabledReason !== undefined && (
                <span id={writeReasonId} className="sr-only">
                  {writeDisabledReason}
                </span>
              )}
            </>
          )}
        </div>
        {/*
          The same door the Edit MANIFEST link above is behind, and behind the
          same second condition: seeding a section IS opening that editor, so
          a reader who may not open it is not offered a button that would.
          Withheld the same way while renaming, rather than shown disabled:
          these starter buttons are a second path to the editor the link
          above already names as disabled and why, right beside them.
        */}
        <ManifestPanel
          domain={domain}
          manifest={manifest.data}
          pending={manifest.isPending}
          error={manifest.error}
          onStart={
            capabilities.canAdminister && manifestEditable && !isRenaming
              ? (section) => {
                  void navigate(manifestEditRoute(domain), {
                    state: { seedSection: section },
                  });
                }
              : null
          }
        />
      </section>

      {importing && (
        <ImportArchiveDialog
          domain={domain}
          onClose={() => {
            setImporting(false);
          }}
        />
      )}

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
      />

      {/*
        The team's furniture comes after the engrams: who may reach the
        domain, which way a write goes and the MANIFEST's own switches are
        settled once and read rarely, so they sit below what a reader opens
        the page for.

        No capability gate here: `GET /members` is served to any account that
        may see the domain at all, and the card itself decides what it may
        offer from what that read says rather than from an instance-wide
        capability; see its own module doc.
      */}
      <MembersCard domain={domain} />

      {/*
        Which way a write in this domain goes, and the control that changes it.
        Under the same instance-wide capability the two share cards above are
        under, because review mode is about proposing changes to a team and an
        instance that shares with nobody has no use for it. It is NOT a
        per-domain gate: the card is drawn on a virtual or origin-less domain
        too, where the button answers 409 in the server's own words.
      */}
      {capabilities.canShare && summary !== undefined && (
        <ReviewModeCard domain={domain} reviewing={summary.review !== null} />
      )}

      {/*
        The MANIFEST's switches, as controls rather than as a panel of prose,
        right beside review mode: `sharing` decides the same thing review mode's
        own button does, and `generated_indexes` sits next to it because both
        are registry rows rather than document text. Drawn off the server's
        registry and only when it sent rows: a MANIFEST that did not parse
        declares nothing and can declare nothing until it is repaired, which
        the "Edit MANIFEST" link above the panel is where somebody does.
      */}
      {manifest.data?.sections !== null &&
        manifest.data?.sections !== undefined &&
        manifest.data.sections.policies.length > 0 && (
          <DomainPoliciesCard
            domain={domain}
            policies={manifest.data.sections.policies}
            branch={syncStatus.data?.branch ?? null}
            onRename={() => {
              setRenamingOpen(true);
            }}
            renaming={isRenaming}
          />
        )}

      {/*
        Last on the page: a copy of the domain, and the two ways of taking it
        away from the people who read it. Both halves of the archive round
        trip are admin-only endpoints, so that card is gated here. The danger
        zone gates itself, because both of its verbs are the owner's as well
        as an admin's and only the members read says who the owner is; it
        draws nothing for a caller who may reach neither. The unregister
        confirmation is the screen's state rather than the card's, because the
        palette row above asks the same question and must arm that exact
        control.
      */}
      {capabilities.canAdminister && (
        <BackupCard
          domain={domain}
          onImport={() => {
            setImporting(true);
          }}
          renaming={isRenaming}
        />
      )}
      <DangerZoneCard
        domain={domain}
        kind={summary?.kind ?? null}
        confirming={confirmingUnregister}
        onConfirmingChange={setConfirmingUnregister}
        renaming={isRenaming}
      />
    </div>
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
  const { capabilities } = useAuth();
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
      capabilities.canWrite
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
    [capabilities.canWrite],
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
      <Link to={domainRoute(domain)} className={link}>
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
 * The engrams of a domain or of a folder: the heading, the subfolders, the
 * filters, and the list with its own row above it.
 *
 * Shared by both pages so a folder lists exactly the way its domain does.
 * Nothing administrative is drawn here any more: the archive round trip and
 * unregistering have cards of their own at the foot of the domain page, so
 * this heading carries New engram only. The order menu sits on the row
 * directly above the list instead, beside whatever that row says the list
 * is a list of - the count at the root, the scope in a folder or under a
 * filter - so it reads as the list's own header rather than the heading's.
 * The create dialog's open state is the page's rather than this section's,
 * because the page's palette row opens the same dialog.
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
   * idea why, so the control is withheld here instead. Defaults to false:
   * the folder page reads no domain summary of its own to know otherwise.
   */
  renaming?: boolean;
}) {
  const { capabilities } = useAuth();
  const newEngramReasonId = useId();
  const newEngramDisabledReason = renaming ? RENAMING_REASON : undefined;

  return (
    <section aria-labelledby="domain-engrams">
      <div className="mb-3 flex flex-wrap items-baseline justify-between gap-3">
        <h2 id="domain-engrams" className="text-section">
          Engrams
        </h2>
        {capabilities.canWrite && (
          <>
            <button
              type="button"
              // `aria-disabled`, not `disabled`: the same trade every other
              // control on this page makes, so a keyboard user still reaches
              // the button and still hears why it will not act.
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
              // Primary: writing an engram is what a writer opens a domain to
              // do. The sidebar's launcher hides on these screens, so the two
              // never sit on one page competing for the same attention.
              className={`${BUTTON.primary} aria-disabled:bg-slate-200 aria-disabled:text-slate-500 dark:aria-disabled:bg-slate-800 dark:aria-disabled:text-slate-500`}
            >
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

      <FilterBar
        // Keyed by the filters that are actually applied, so the two typed
        // fields reset to them whenever the URL moves under this screen: a
        // back button, a shared link, or the clear button. Without it they
        // would keep showing a filter that is no longer in force.
        key={`${filters.type ?? ""}|${filters.status ?? ""}`}
        filters={filters}
        tags={tags}
        onChange={(next) => {
          onApply(next);
        }}
      />

      {/*
        The row directly above the list, carrying what the list below is a
        list of and the order it is in. At the root that is the count, read
        off the list's own first page through `summary` below, so the total
        has exactly one source. In a folder or under a filter the scope is
        named here instead, because a folder or a filter is a fact about the
        request rather than about any page it answers - it holds even on an
        empty first page - and the list is handed a `summary` that draws
        nothing, so the row is said once.
      */}
      {(filtering || path !== "") && (
        <div className="flex flex-wrap items-center justify-between gap-3 py-3">
          <p className="text-sm text-slate-500 dark:text-slate-400">
            {filtering
              ? "Filtered across the whole domain, every folder included."
              : `Browsing ${path}, subfolders included.`}
          </p>
          <EngramsOrderMenu />
        </div>
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
        />
      ) : (
        <EngramList
          // The same endpoint the filtered view pages, scoped to the folder
          // instead of filtered: a folder holding thousands of engrams costs
          // one page here rather than the whole folder, and the key carries
          // the scope, so opening another folder starts another list.
          queryKey={domainEngramsKey(domain, browse, listingOrder)}
          loadPage={(page) =>
            fetchDomainEngrams(domain, browse, page, listingOrder)
          }
          label={`Engrams in ${domain}`}
          // At the root this list draws the whole row above itself: the
          // count left, the order menu right. In a folder the row above is
          // already drawn by this section, so the list says nothing.
          summary={
            path === ""
              ? (page: EngramPage) => (
                  <div className="flex flex-wrap items-center justify-between gap-3 pb-2">
                    <p className="text-caption text-slate-500 tabular-nums dark:text-slate-400">
                      {plural(page.total, "engram", "engrams")} in this domain
                    </p>
                    <EngramsOrderMenu />
                  </div>
                )
              : () => null
          }
          emptyMessage={
            path === ""
              ? "This domain has no engrams yet."
              : "This folder has no engrams."
          }
        />
      )}
    </section>
  );
}

/**
 * The MANIFEST, whole, where the domain is introduced.
 *
 * Rendered by the document renderer inside the reading measure, so diagrams,
 * links, attachments and large text behave exactly as in any engram. The
 * document's own opening title folds away where it repeats the domain's
 * name, which the page's heading has already said.
 */
function ManifestPanel({
  domain,
  manifest,
  pending,
  error,
  onStart,
}: {
  domain: string;
  manifest: ManifestView | undefined;
  pending: boolean;
  error: Error | null;
  /**
   * Seed a section into the editor and hand over, or null for a reader who
   * may not edit this MANIFEST. Nothing here writes: the button navigates,
   * the editor appends, and the person saves - which is the rule the facets
   * below were built on and the reason they stay read-only.
   */
  onStart: ((section: string) => void) | null;
}) {
  if (pending) {
    return <Skeleton label="Loading the manifest" />;
  }
  // A refusal is announced before the gap below is, because an errored read
  // carries no markdown either and would otherwise be reported as a domain
  // that never wrote one. Only a 404 is that gap; anything else is the
  // server declining to answer and says so in its own words.
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
  // A missing MANIFEST is a gap in the domain rather than a failure of the
  // screen, and it is the one thing every domain is supposed to have, so it is
  // said plainly rather than announced as an error. The boxes below it are
  // drawn all the same where the server answered: a domain with nothing
  // written is the moment the page has most to explain, not least.
  const empty =
    isMissing(error) ||
    manifest === undefined ||
    manifest.markdown.trim() === "";
  // A read that 404ed carries no sections and so no starters either: the
  // registry travels with the manifest payload, and there is no payload. That
  // reader gets the sentence and the editor link above it, which is what they
  // got before.
  const sections = manifest?.sections ?? null;
  return (
    <div className="flex flex-col gap-4">
      {empty && (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          This domain has no MANIFEST yet, so nothing tells an agent what it is
          for.
        </p>
      )}
      {sections !== null && (
        <ManifestFacets sections={sections} onStart={onStart} />
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
      {!empty && manifest !== undefined && (
        <article className="measured">
          {/*
            The domain's own attachments live at its root, so a MANIFEST that
            references one resolves it exactly the way an engram does.
          */}
          <Markdown
            source={manifest.markdown}
            domain={domain}
            foldTitle={domain}
          />
        </article>
      )}
    </div>
  );
}

/**
 * The stanza the server sent for `section`, or null when it sent none.
 *
 * Null is the older daemon: it answered sections without the registry of
 * startable ones, and a facet then draws the bare line it drew before rather
 * than an explanation this side invented.
 */
function starterFor(
  sections: ManifestSections,
  section: string,
): StarterStanza | null {
  return sections.starters.find((row) => row.section === section) ?? null;
}

/**
 * What an empty facet says instead of a bare "nothing here".
 *
 * The line and the example both come from the server's registry, never from a
 * list kept here, for the reason the policies card draws its rows from the
 * server: a section added to the registry in core shows up with its meaning
 * and its syntax without a line of this file changing, and the core-side
 * guard test parses every example back into the declaration it advertises, so
 * what a person is shown is what the parser accepts.
 *
 * The action does not write anything. It seeds the editor and hands over,
 * which keeps the rule this panel was built on: the document is the source
 * and the editor is where a change goes.
 */
function EmptyFacet({
  starter,
  fallback,
  onStart,
}: {
  starter: StarterStanza | null;
  /** What to say when the server sent no registry to say anything from. */
  fallback: string;
  onStart: ((section: string) => void) | null;
}) {
  if (starter === null) {
    return <p>{fallback}</p>;
  }
  return (
    <div className="flex flex-col gap-2">
      <p className="text-slate-600 dark:text-slate-400">{starter.meaning}</p>
      <pre className="text-caption overflow-x-auto rounded bg-slate-100 p-2 font-mono whitespace-pre dark:bg-slate-900">
        {starter.example}
      </pre>
      {onStart !== null && (
        <button
          type="button"
          className={`self-start ${BUTTON.secondary}`}
          onClick={() => {
            onStart(starter.section);
          }}
        >
          {`Add ${starter.section}`}
        </button>
      )}
    </div>
  );
}

/**
 * The three panels: what the core crate reads out of the MANIFEST, as it
 * reads it.
 *
 * Read-only on purpose. A switch here would be a second place to change the
 * document, and the document is the source: the editor is where a change
 * goes, and these say what it currently says. The frontmatter switches are
 * the exception, and they are not here: they are controls, so they have a
 * card of their own, down with the domain's other team furniture rather
 * than in this section. Whether provisioning was allowed or denied on this
 * machine is not here either; that decision lives with the `provision`
 * tool, not with the domain's own description.
 */
function ManifestFacets({
  sections,
  onStart,
}: {
  sections: ManifestSections;
  onStart: ((section: string) => void) | null;
}) {
  const missingScope = sections.missing.includes("Scope");
  const missingWhenToUse = sections.missing.includes("When to Use");
  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <Facet title="Routing">
        <h4 className="text-caption font-semibold text-slate-500 dark:text-slate-400">
          When to Use
        </h4>
        {missingWhenToUse ? (
          <EmptyFacet
            starter={starterFor(sections, "When to Use")}
            fallback="No When to Use section"
            onStart={onStart}
          />
        ) : (
          <Bullets items={sections.whenToUse} />
        )}
        <h4 className="text-caption mt-2 font-semibold text-slate-500 dark:text-slate-400">
          Scope
        </h4>
        {missingScope ? (
          <EmptyFacet
            starter={starterFor(sections, "Scope")}
            fallback="No Scope section"
            onStart={onStart}
          />
        ) : (
          <Bullets items={sections.scope} />
        )}
        <p className="mt-2 text-slate-500 dark:text-slate-400">
          {sections.routing === "when_to_use"
            ? "Agents route by When to Use."
            : sections.routing === "scope"
              ? "Agents route by Scope, because When to Use is absent or empty."
              : "Agents cannot route here until When to Use has a bullet."}
        </p>
      </Facet>
      <Facet title="Provisioning">
        {sections.provisioning === null ||
        sections.provisioning.decls.length === 0 ? (
          <EmptyFacet
            starter={starterFor(sections, "Provisioning")}
            fallback="Nothing declared"
            onStart={onStart}
          />
        ) : (
          <ul className="flex flex-col gap-1">
            {sections.provisioning.decls.map((decl) => (
              <li key={decl.kind} className="font-mono">
                {`${decl.kind}: ${decl.path}`}
              </li>
            ))}
          </ul>
        )}
        {sections.provisioning !== null && (
          <Problems items={sections.provisioning.problems} />
        )}
      </Facet>
      <Facet title="Tag aliases">
        {sections.tagAliases === null ||
        sections.tagAliases.decls.length === 0 ? (
          <EmptyFacet
            starter={starterFor(sections, "Tag Aliases")}
            fallback="No aliases"
            onStart={onStart}
          />
        ) : (
          <ul className="flex flex-col gap-1">
            {sections.tagAliases.decls.map((decl) => (
              <li key={decl.alias} className="font-mono">
                {`${decl.alias} -> ${decl.canonical}`}
              </li>
            ))}
          </ul>
        )}
        {sections.tagAliases !== null && (
          <Problems items={sections.tagAliases.problems} />
        )}
      </Facet>
    </div>
  );
}

/** One panel: a labelled region, so each is reachable by its name. */
function Facet({ title, children }: { title: string; children: ReactNode }) {
  const id = `manifest-facet-${title.toLowerCase().replace(/\s+/g, "-")}`;
  return (
    <section
      aria-labelledby={id}
      className="rounded border border-slate-200 p-3 text-sm dark:border-slate-800"
    >
      <h3 id={id} className="mb-2 font-medium">
        {title}
      </h3>
      {children}
    </section>
  );
}

/**
 * A section's bullets, as the MANIFEST lists them. Nothing for none.
 *
 * Each bullet is drawn as inline markdown rather than as plain text, so
 * `**Search `project2030` first**` reads as bold text around a code span
 * instead of showing its own asterisks and backticks.
 */
function Bullets({ items }: { items: string[] }) {
  if (items.length === 0) {
    return null;
  }
  return (
    <ul className="list-disc pl-5">
      {items.map((item) => (
        <li key={item}>
          <InlineMarkdown source={item} />
        </li>
      ))}
    </ul>
  );
}

/**
 * The bullets the core crate flagged, each with its reason in the crate's
 * own words. The bullet in monospace, because it is quoted from the file.
 */
function Problems({ items }: { items: ManifestProblem[] }) {
  if (items.length === 0) {
    return null;
  }
  return (
    <div className="mt-2">
      <h4 className="text-caption font-semibold text-red-700 dark:text-red-300">
        Problems
      </h4>
      <ul className="flex flex-col gap-1">
        {items.map((item) => (
          <li key={`${item.kind}:${item.bullet}`}>
            <span className="font-mono">{item.bullet}</span>
            <span className="block text-slate-500 dark:text-slate-400">
              {item.reason}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

/** The subfolders of the folder being browsed, each a step further in. */
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
  return (
    <nav aria-label="Folders">
      <ul className="flex flex-wrap gap-2">
        {folders.map((folder) => (
          <li key={folder}>
            <button
              type="button"
              className="rounded border border-slate-200 px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-800 dark:hover:bg-slate-800"
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

/** The wrong-address screen, which is not the same thing as an empty domain. */
function DomainNotFound({ domain }: { domain: string }) {
  return (
    <div className="flex flex-col items-start gap-3">
      <h1 className="text-display">Domain not found</h1>
      <p className="text-sm">
        No domain named {`"${domain}"`} is registered on this instance.
      </p>
      <Link
        to="/"
        className="text-sm text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
      >
        See the domains that are
      </Link>
    </div>
  );
}

/** Whether this failure is the server saying there is nothing at that address. */
function isMissing(error: unknown): boolean {
  return error instanceof ApiProblem && error.status === 404;
}
