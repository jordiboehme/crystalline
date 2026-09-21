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

import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { Fragment, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";

import { archiveDownloadUrl } from "../api/admin";
import { ApiProblem, problemDetail } from "../api/client";
import { fetchManifest, manifestKey, treeQuery } from "../api/domain";
import type {
  ManifestProblem,
  ManifestSections,
  ManifestView,
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
import { fetchTags, vocabularyKey } from "../api/vocabulary";
import type { TagCount } from "../api/vocabulary";
import { useAuth } from "../auth/AuthContext";
import { NO_COMMANDS, useRegisterCommands } from "../commands";
import type { PaletteCommand } from "../commands";
import { BackupCard } from "../components/BackupCard";
import { CreateEngramDialog } from "../components/CreateEngramDialog";
import { DangerZoneCard } from "../components/DangerZoneCard";
import { EngramList } from "../components/EngramList";
import { EngramsOrderMenu } from "../components/EngramsOrderMenu";
import { FilterFields, TagChips } from "../components/FilterControls";
import { ImportArchiveDialog } from "../components/ImportArchiveDialog";
import { Markdown } from "../components/Markdown";
import { MembersCard } from "../components/MembersCard";
import { ProposalsCard } from "../components/ProposalsCard";
import { ReviewModeCard } from "../components/ReviewModeCard";
import { Skeleton } from "../components/Skeleton";
import { SyncCard } from "../components/SyncCard";
import { BUTTON, Chip, FOCUS_RING } from "../components/primitives";
import { orderQuery, useEngramsOrder } from "../engramsOrder";
import { frontmatterFilters } from "../filters";
import { plural } from "../format";
import { domainRoute, folderRoute, manifestEditRoute } from "../paths";
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
  const { capabilities } = useAuth();
  const navigate = useNavigate();
  const { path, filters, browse, filtering, listingOrder, apply } =
    useListingState();
  const [creating, setCreating] = useState(false);
  const [importing, setImporting] = useState(false);
  const [confirmingUnregister, setConfirmingUnregister] = useState(false);

  const listing = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });
  const summary = listing.data?.domains.find((entry) => entry.name === domain);
  const manifest = useQuery({
    queryKey: manifestKey(domain),
    queryFn: () => fetchManifest(domain),
  });
  const tags = useQuery({
    queryKey: vocabularyKey(domain),
    queryFn: () => fetchTags(domain),
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
  // does. Unregistering rides on the same gates, one role higher: the palette
  // row does what the button does, which is to ASK - the second press is the
  // point of the control and the keyboard route does not get to skip it.
  // Editing the MANIFEST is behind the same gate as its link, and under the
  // same second condition: a read that landed, or a domain that has no
  // MANIFEST yet, which is precisely what an admin opens the editor to fix.
  // Only a refused read is nothing to edit, and a read still in flight is
  // nothing to edit yet. Named once so the two doors cannot drift apart.
  const manifestLoaded = manifest.data !== undefined;
  const manifestEditable = manifestLoaded || isMissing(manifest.error);
  const commands = useMemo<readonly PaletteCommand[]>(() => {
    const rows: PaletteCommand[] = [];
    if (capabilities.canWrite) {
      rows.push({
        id: "create",
        title: "New engram",
        run: () => {
          setCreating(true);
        },
      });
    }
    if (capabilities.canAdminister) {
      if (manifestEditable) {
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
        // press a link that may not even be rendered.
        run: () => {
          window.location.assign(archiveDownloadUrl(domain));
        },
      });
      rows.push({
        id: "import-archive",
        title: "Import archive",
        run: () => {
          setImporting(true);
        },
      });
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
    capabilities.canAdminister,
    capabilities.canWrite,
    domain,
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
        </div>
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
            <Link
              to={manifestEditRoute(domain)}
              onPointerEnter={prefetchManifestEditor}
              onFocus={prefetchManifestEditor}
              className={`inline-flex items-center ${BUTTON.secondary}`}
            >
              Edit MANIFEST
            </Link>
          )}
        </div>
        <ManifestPanel
          domain={domain}
          manifest={manifest.data}
          pending={manifest.isPending}
          error={manifest.error}
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
      />

      {/*
        The team's furniture comes after the engrams: who may reach the
        domain and which way a write goes are settled once and read rarely,
        so they sit below what a reader opens the page for.

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
        Last on the page: a copy of the domain, and the two ways of taking it
        away from the people who read it. Both halves of the archive round
        trip are admin-only endpoints, so that card is gated here. The danger
        zone gates itself, because one of its two verbs is the owner's as well
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
        />
      )}
      <DangerZoneCard
        domain={domain}
        kind={summary?.kind ?? null}
        confirming={confirmingUnregister}
        onConfirmingChange={setConfirmingUnregister}
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
  const { filters, browse, filtering, listingOrder, apply } =
    useListingState();
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
}) {
  const { capabilities } = useAuth();

  return (
    <section aria-labelledby="domain-engrams">
      <div className="mb-3 flex flex-wrap items-baseline justify-between gap-3">
        <h2 id="domain-engrams" className="text-section">
          Engrams
        </h2>
        {capabilities.canWrite && (
          <button
            type="button"
            onClick={() => {
              onCreatingChange(true);
            }}
            // Primary: writing an engram is what a writer opens a domain to
            // do. The sidebar's launcher hides on these screens, so the two
            // never sit on one page competing for the same attention.
            className={BUTTON.primary}
          >
            New engram
          </button>
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
}: {
  domain: string;
  manifest: ManifestView | undefined;
  pending: boolean;
  error: Error | null;
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
  // said plainly rather than announced as an error.
  if (
    isMissing(error) ||
    manifest === undefined ||
    manifest.markdown.trim() === ""
  ) {
    return (
      <p className="text-sm text-slate-500 dark:text-slate-400">
        This domain has no MANIFEST yet, so nothing tells an agent what it is
        for.
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-4">
      {manifest.sections !== null && (
        <ManifestFacets sections={manifest.sections} />
      )}
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
    </div>
  );
}

/**
 * The four panels: what the core crate reads out of the MANIFEST, as it
 * reads it.
 *
 * Read-only on purpose. A switch here would be a second place to change the
 * document, and the document is the source: the editor is where a change
 * goes, and these say what it currently says. Whether provisioning was
 * allowed or denied on this machine is not here either; that decision lives
 * with the `provision` tool, not with the domain's own description.
 */
function ManifestFacets({ sections }: { sections: ManifestSections }) {
  const missingScope = sections.missing.includes("Scope");
  const missingWhenToUse = sections.missing.includes("When to Use");
  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <Facet title="Routing">
        <h4 className="text-caption font-semibold text-slate-500 dark:text-slate-400">
          When to Use
        </h4>
        {missingWhenToUse ? (
          <p>No When to Use section</p>
        ) : (
          <Bullets items={sections.whenToUse} />
        )}
        <h4 className="text-caption mt-2 font-semibold text-slate-500 dark:text-slate-400">
          Scope
        </h4>
        {missingScope ? (
          <p>No Scope section</p>
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
          <p>Nothing declared</p>
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
          <p>No aliases</p>
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
      <Facet title="Configuration">
        {/*
          The frontmatter switches. One today; any switch added later joins
          this panel rather than growing a fifth.
        */}
        <p className="font-mono">
          {`generated_indexes: ${sections.generatedIndexes.declared ?? "not declared"}`}
        </p>
        <p className="mt-1 text-slate-500 dark:text-slate-400">
          {sections.generatedIndexes.effective === "local"
            ? "Effective: local. The generated directory indexes stay on this machine."
            : "Effective: shared. The generated directory indexes travel with the domain."}
        </p>
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

/** A section's bullets, as the MANIFEST lists them. Nothing for none. */
function Bullets({ items }: { items: string[] }) {
  if (items.length === 0) {
    return null;
  }
  return (
    <ul className="list-disc pl-5">
      {items.map((item) => (
        <li key={item}>{item}</li>
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
