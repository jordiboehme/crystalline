/**
 * One domain: what it is for, and what is in it.
 *
 * There are two ways of looking at what is in it, and exactly one is on screen
 * at a time with a line above the list saying which: a folder, or a
 * frontmatter filter across the whole domain. Blending them would mean filters
 * that quietly ignored the folder they sit under, or a folder that quietly
 * dropped what the filter did not match.
 *
 * Both are the same endpoint now. The listing pages a folder (`path`) exactly
 * as it pages a filter, so a folder holding thousands of engrams costs one
 * page rather than the folder, and the count above the rows is the server's
 * own. What the tree is still for is navigation: the subfolders of the folder
 * being browsed and the trail back out of it, which is a level rather than a
 * list.
 *
 * Both views live in the URL, so a folder or a filter is a link somebody can
 * send, and the back button moves between them.
 */

import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router";

import { ApiProblem, problemDetail } from "../api/client";
import { fetchManifest, manifestKey, treeQuery } from "../api/domain";
import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import type { EngramFilters } from "../api/engrams";
import {
  NO_FILTERS,
  domainEngramsKey,
  fetchDomainEngrams,
  hasFilters,
} from "../api/engrams";
import { fetchTags, vocabularyKey } from "../api/vocabulary";
import type { TagCount } from "../api/vocabulary";
import { useAuth } from "../auth/AuthContext";
import { NO_COMMANDS, useRegisterCommands } from "../commands";
import type { PaletteCommand } from "../commands";
import { CreateEngramDialog } from "../components/CreateEngramDialog";
import { EngramList } from "../components/EngramList";
import { FilterFields, TagChips } from "../components/FilterControls";
import { Skeleton } from "../components/Skeleton";
import { BUTTON, Chip, FOCUS_RING } from "../components/primitives";
import { plural } from "../format";
import { manifestRoute } from "../paths";
import { stripSnippetMarkup } from "../snippet";

export default function DomainHome() {
  const { domain = "" } = useParams();
  const [params, setParams] = useSearchParams();
  const { capabilities } = useAuth();
  const [creating, setCreating] = useState(false);

  const path = params.get("path") ?? "";
  // The frontmatter view, which is the whole domain: no `path`, deliberately.
  // Scoping a filter to the folder being browsed is a different feature - the
  // line above the list says "every folder included" and means it - so the
  // scope is left empty here rather than picked up from the URL by accident.
  const filters: EngramFilters = useMemo(
    () => ({
      type: params.get("type"),
      status: params.get("status"),
      tags: (params.get("tags") ?? "").split(",").filter((tag) => tag !== ""),
      path: "",
    }),
    [params],
  );
  // The browse view: one folder, no frontmatter filter, paged by the server.
  const browse: EngramFilters = useMemo(
    () => ({ ...NO_FILTERS, path }),
    [path],
  );
  const filtering = hasFilters(filters);

  const listing = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });
  const summary = listing.data?.domains.find((entry) => entry.name === domain);

  const manifest = useQuery({
    queryKey: manifestKey(domain),
    queryFn: () => fetchManifest(domain),
  });
  // The tree is what the folder navigation above the list is drawn from - the
  // subfolders of the folder being browsed, and the trail back out of it. It
  // no longer carries the listing: a level of it is capped by the server, and
  // a folder of any size is what the paged listing below is for.
  const tree = useQuery(treeQuery(domain, path));
  const tags = useQuery({
    queryKey: vocabularyKey(domain),
    queryFn: () => fetchTags(domain),
  });

  // A domain nobody registered is a wrong address, not an empty shelf. The
  // tree is what says so: a 404 from the manifest also means a domain that
  // simply has not been introduced yet.
  const unknownDomain = isMissing(tree.error);

  // The one write this screen offers, on the palette under both of the gates
  // the button is under: what this session may do, and whether this screen is
  // showing a domain at all. The not-found branch below draws no button and
  // mounts no dialog, so a row there would set a flag nothing reads.
  //
  // The dialog it opens picks its own folder from the URL, so the keyboard
  // route lands exactly where the pointer route does.
  const commands = useMemo<readonly PaletteCommand[]>(
    () =>
      capabilities.canWrite && !unknownDomain
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
    [capabilities.canWrite, unknownDomain],
  );
  useRegisterCommands(commands);

  /** Change the URL, which is the whole of this screen's state. */
  function apply(next: {
    path?: string;
    type?: string | null;
    status?: string | null;
    tags?: string[];
  }) {
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

  if (unknownDomain) {
    return <DomainNotFound domain={domain} />;
  }

  return (
    <div className="flex flex-col gap-8">
      <header>
        <h1 className="text-display">{domain}</h1>
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
      </header>

      <section aria-labelledby="domain-manifest">
        <h2 id="domain-manifest" className="mb-2 text-section">
          Manifest
        </h2>
        <ManifestPanel
          domain={domain}
          markdown={manifest.data}
          pending={manifest.isPending}
          error={manifest.error}
        />
      </section>

      <section aria-labelledby="domain-engrams">
        <div className="mb-3 flex flex-wrap items-baseline justify-between gap-3">
          <h2 id="domain-engrams" className="text-section">
            Engrams
          </h2>
          {capabilities.canWrite && (
            <button
              type="button"
              onClick={() => {
                setCreating(true);
              }}
              // Primary: writing an engram is what a writer opens a domain to
              // do. The sidebar's launcher hides on this screen, so the two
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
              setCreating(false);
            }}
          />
        )}

        <FolderNav
          domain={domain}
          path={path}
          folders={tree.data?.folders ?? []}
          onOpen={(next) => {
            // Opening a folder is a browse, so it leaves the frontmatter view.
            apply({ path: next, type: null, status: null, tags: [] });
          }}
        />

        <FilterBar
          // Keyed by the filters that are actually applied, so the two typed
          // fields reset to them whenever the URL moves under this screen: a
          // back button, a shared link, or the clear button. Without it they
          // would keep showing a filter that is no longer in force.
          key={`${filters.type ?? ""}|${filters.status ?? ""}`}
          filters={filters}
          tags={tags.data ?? []}
          onChange={(next) => {
            apply(next);
          }}
        />

        <p className="py-3 text-sm text-slate-500 dark:text-slate-400">
          {/*
            What the list below is a list of, in one line. The browse view is
            a folder and everything under it, which is what the endpoint's
            `path` means, so the line says so rather than letting a reader read
            "Browsing notes" as the four files sitting directly in it.
          */}
          {filtering
            ? "Filtered across the whole domain, every folder included."
            : path === ""
              ? "Browsing this domain, every folder included."
              : `Browsing ${path}, subfolders included.`}
        </p>

        {filtering ? (
          <EngramList
            queryKey={domainEngramsKey(domain, filters)}
            loadPage={(page) => fetchDomainEngrams(domain, filters, page)}
            label={`Engrams in ${domain}`}
            emptyMessage="No engram matches these filters."
          />
        ) : (
          <EngramList
            // The same endpoint the filtered view pages, scoped to the folder
            // instead of filtered: a folder holding thousands of engrams costs
            // one page here rather than the whole folder, and the key carries
            // the scope, so opening another folder starts another list.
            queryKey={domainEngramsKey(domain, browse)}
            loadPage={(page) => fetchDomainEngrams(domain, browse, page)}
            label={`Engrams in ${domain}`}
            // The count this list would draw on its own is "50 of 620 shown",
            // which says nothing about where those 620 are, and reads as a
            // contradiction of the "4 engrams" under the domain's name, which
            // counts the whole domain. Naming the scope is what settles it.
            summary={(page) => (
              <p className="text-caption pb-2 text-slate-500 tabular-nums dark:text-slate-400">
                {plural(page.total, "engram", "engrams")}{" "}
                {path === "" ? "in this domain" : "in this folder"}
              </p>
            )}
            emptyMessage={
              path === ""
                ? "This domain has no engrams yet."
                : "This folder has no engrams."
            }
          />
        )}
      </section>
    </div>
  );
}

/**
 * The first prose paragraph of the manifest, after frontmatter and headings.
 *
 * Blocks are split on the blank line rather than parsed: the lede feeds a plain
 * paragraph, so what comes back has to be prose and never markdown syntax
 * rendered as text. A manifest that opens with a heading and a list has no
 * lede, and says so by answering null.
 */
function manifestLede(markdown: string): string | null {
  const body = markdown.replace(/^---\r?\n[\s\S]*?\r?\n---[ \t]*\r?\n?/, "");
  for (const block of body.split(/\r?\n\s*\r?\n/)) {
    const line = block.trim();
    if (line === "" || line.startsWith("#") || PROSE_EXCLUDED.test(line)) {
      continue;
    }
    // The same stripper the search snippets use, for the same reason: this
    // feeds a plain paragraph, and a lede wearing its own asterisks would be
    // the raw-markdown bug one screen over.
    const lede = stripSnippetMarkup(line).replace(/\s+/g, " ").trim();
    if (lede !== "") {
      return lede;
    }
  }
  return null;
}

/** Blocks that are structure rather than prose: lists, quotes, rules, fences. */
const PROSE_EXCLUDED = /^([-*+>|]|\d+\.|```|---)/;

/** The MANIFEST in one line, and the way to the whole of it. */
function ManifestPanel({
  domain,
  markdown,
  pending,
  error,
}: {
  domain: string;
  markdown: string | undefined;
  pending: boolean;
  error: Error | null;
}) {
  if (pending) {
    return <Skeleton label="Loading the manifest" />;
  }
  // A missing MANIFEST is a gap in the domain rather than a failure of the
  // screen, and it is the one thing every domain is supposed to have, so it is
  // said plainly rather than announced as an error.
  if (isMissing(error) || markdown === undefined || markdown.trim() === "") {
    return (
      <p className="text-sm text-slate-500 dark:text-slate-400">
        This domain has no MANIFEST yet, so nothing tells an agent what it is
        for.
      </p>
    );
  }
  if (error) {
    return (
      <p
        role="alert"
        className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
      >
        {problemDetail(error)}
      </p>
    );
  }
  const lede = manifestLede(markdown);
  return (
    // The one measure, from the one class: a lede that ran the width of a
    // wide monitor would be the reading problem this app fixed elsewhere.
    <div className="measured flex flex-col items-start gap-2">
      {lede !== null && <p className="text-sm">{lede}</p>}
      <Link
        to={manifestRoute(domain)}
        className={`text-sm text-accent-700 underline underline-offset-2 hover:no-underline dark:text-accent-400 ${FOCUS_RING}`}
      >
        Read the MANIFEST
      </Link>
    </div>
  );
}

/** Where in the domain the browse view is, and where it can go from here. */
function FolderNav({
  domain,
  path,
  folders,
  onOpen,
}: {
  domain: string;
  path: string;
  folders: string[];
  onOpen: (path: string) => void;
}) {
  const segments = path === "" ? [] : path.split("/");
  return (
    <nav aria-label="Folders" className="flex flex-col gap-2">
      <p className="flex flex-wrap items-center gap-1 text-sm">
        {segments.length === 0 ? (
          <span className="font-medium">{domain}</span>
        ) : (
          <button
            type="button"
            className="rounded px-1 underline underline-offset-2 hover:no-underline"
            onClick={() => {
              onOpen("");
            }}
          >
            {domain}
          </button>
        )}
        {segments.map((segment, index) => {
          const upto = segments.slice(0, index + 1).join("/");
          const here = index === segments.length - 1;
          return (
            <span key={upto} className="flex items-center gap-1">
              <span aria-hidden="true" className="text-slate-400">
                /
              </span>
              {here ? (
                <span className="font-medium">{segment}</span>
              ) : (
                <button
                  type="button"
                  className="rounded px-1 underline underline-offset-2 hover:no-underline"
                  onClick={() => {
                    onOpen(upto);
                  }}
                >
                  {segment}
                </button>
              )}
            </span>
          );
        })}
      </p>
      {folders.length > 0 && (
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
      )}
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
