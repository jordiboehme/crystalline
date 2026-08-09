/**
 * One domain: what it is for, and what is in it.
 *
 * The two ways of looking at what is in it come from two endpoints, and that
 * split is the server's rather than this screen's: the tree owns navigation by
 * folder and knows nothing about frontmatter, while the engram listing owns the
 * frontmatter filters and knows nothing about folders. So the list has two
 * sources, exactly one is on screen at a time, and a line above it says which -
 * blending them would mean showing a folder that quietly contained engrams from
 * elsewhere, or filters that quietly ignored the folder they sit under.
 *
 * Both views live in the URL, so a folder or a filter is a link somebody can
 * send.
 */

import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router";

import { ApiProblem, problemDetail } from "../api/client";
import { fetchManifest, fetchTree, manifestKey, treeKey } from "../api/domain";
import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import type { EngramFilters } from "../api/engrams";
import {
  domainEngramsKey,
  fetchDomainEngrams,
  hasFilters,
  singlePage,
} from "../api/engrams";
import { fetchTags, vocabularyKey } from "../api/vocabulary";
import type { TagCount } from "../api/vocabulary";
import { useAuth } from "../auth/AuthContext";
import { NO_COMMANDS, useRegisterCommands } from "../commands";
import type { PaletteCommand } from "../commands";
import { CreateEngramDialog } from "../components/CreateEngramDialog";
import { EngramList } from "../components/EngramList";
import { FilterFields, TagChips } from "../components/FilterControls";
import { Markdown } from "../components/Markdown";
import { plural } from "../format";

export default function DomainHome() {
  const { domain = "" } = useParams();
  const [params, setParams] = useSearchParams();
  const { capabilities } = useAuth();
  const [creating, setCreating] = useState(false);

  const path = params.get("path") ?? "";
  const filters: EngramFilters = useMemo(
    () => ({
      type: params.get("type"),
      status: params.get("status"),
      tags: (params.get("tags") ?? "").split(",").filter((tag) => tag !== ""),
    }),
    [params],
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
  const tree = useQuery({
    queryKey: treeKey(domain, path),
    queryFn: () => fetchTree(domain, path),
  });
  const tags = useQuery({
    queryKey: vocabularyKey(domain),
    queryFn: () => fetchTags(domain),
  });

  // The one write this screen offers, on the palette under the same gate as
  // the button. The dialog it opens picks its own folder from the URL, so the
  // keyboard route lands exactly where the pointer route does.
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

  /** The folder's own rows, once the tree has answered. */
  const folderRows = tree.data?.engrams;

  // A domain nobody registered is a wrong address, not an empty shelf. The
  // tree is what says so: a 404 from the manifest also means a domain that
  // simply has not been introduced yet.
  if (isMissing(tree.error)) {
    return <DomainNotFound domain={domain} />;
  }

  return (
    <div className="flex flex-col gap-8">
      <header>
        <h1 className="text-xl font-semibold">{domain}</h1>
        {summary && (
          <p className="mt-1 flex flex-wrap gap-x-3 text-sm text-slate-500 dark:text-slate-400">
            {summary.engrams !== null && (
              <span className="tabular-nums">
                {plural(summary.engrams, "engram", "engrams")}
              </span>
            )}
            {summary.kind !== null && <span>{summary.kind}</span>}
          </p>
        )}
      </header>

      <section aria-labelledby="domain-manifest">
        <h2 id="domain-manifest" className="mb-2 text-lg font-semibold">
          Manifest
        </h2>
        <ManifestPanel
          markdown={manifest.data}
          pending={manifest.isPending}
          error={manifest.error}
        />
      </section>

      <section aria-labelledby="domain-engrams">
        <div className="mb-3 flex flex-wrap items-baseline justify-between gap-3">
          <h2 id="domain-engrams" className="text-lg font-semibold">
            Engrams
          </h2>
          {capabilities.canWrite && (
            <button
              type="button"
              onClick={() => {
                setCreating(true);
              }}
              className="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
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
          {filtering
            ? "Filtered across the whole domain, every folder included."
            : path === ""
              ? "Browsing the root folder."
              : `Browsing ${path}.`}
        </p>

        {filtering ? (
          <EngramList
            queryKey={domainEngramsKey(domain, filters)}
            loadPage={(page) => fetchDomainEngrams(domain, filters, page)}
            label={`Engrams in ${domain}`}
            emptyMessage="No engram matches these filters."
          />
        ) : folderRows ? (
          <EngramList
            // The rows are already in hand from the tree, so this loader hands
            // them over rather than asking the server a second time. The key
            // carries the folder, so opening another one starts another list.
            queryKey={["folder-engrams", domain, path]}
            loadPage={() => Promise.resolve(singlePage(folderRows))}
            label={`Engrams in ${domain}`}
            emptyMessage={
              path === ""
                ? "This domain has no engrams yet."
                : "This folder has no engrams."
            }
          />
        ) : tree.error ? (
          <p
            role="alert"
            className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
          >
            {problemDetail(tree.error)}
          </p>
        ) : (
          <p className="text-sm text-slate-500 dark:text-slate-400">
            Loading engrams
          </p>
        )}
      </section>
    </div>
  );
}

/** The MANIFEST, or the fact that there is not one. */
function ManifestPanel({
  markdown,
  pending,
  error,
}: {
  markdown: string | undefined;
  pending: boolean;
  error: Error | null;
}) {
  if (pending) {
    return (
      <p className="text-sm text-slate-500 dark:text-slate-400">
        Loading the manifest
      </p>
    );
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
  return (
    <div className="rounded border border-slate-200 px-4 py-1 dark:border-slate-800">
      <Markdown source={markdown} />
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
                className="rounded border border-slate-200 px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-800 dark:hover:bg-slate-800"
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
    <div className="flex flex-col gap-3">
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
      <h1 className="text-xl font-semibold">Domain not found</h1>
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
