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

import { ApiProblem } from "../api/client";
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
import { EngramList } from "../components/EngramList";
import { Markdown } from "../components/Markdown";
import { plural } from "../format";
import { RETIRED_STATUSES } from "../lifecycle";

/**
 * Values offered as suggestions for the two free-form filters.
 *
 * `type` and `status` are free form by design and no endpoint enumerates the
 * values a domain actually uses, so these are the vocabulary the product
 * recommends rather than a claim about this domain. They are suggestions in a
 * datalist for that reason: anything can be typed, and nothing here says a
 * value not on the list is wrong.
 */
const SUGGESTED_TYPES = [
  "engram",
  "guide",
  "decision",
  "architecture",
  "runbook",
  "reference",
];
const SUGGESTED_STATUSES = [
  "stable",
  "current",
  "draft",
  "proposed",
  "idea",
  "poc",
  ...RETIRED_STATUSES,
];

export default function DomainHome() {
  const { domain = "" } = useParams();
  const [params, setParams] = useSearchParams();

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
        <h2 id="domain-engrams" className="mb-3 text-lg font-semibold">
          Engrams
        </h2>

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
          types={SUGGESTED_TYPES}
          statuses={SUGGESTED_STATUSES}
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
            {detailOf(tree.error)}
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
        {detailOf(error)}
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
 * Tags are chips because the vocabulary endpoint knows every tag in the domain
 * and how many engrams carry it, so the set on screen is the real one. Type and
 * status are typed rather than picked, because nothing enumerates them: a chip
 * row built from one page of results would look like the whole truth about a
 * domain while being whatever the first fifty rows happened to say.
 */
function FilterBar({
  filters,
  tags,
  types,
  statuses,
  onChange,
}: {
  filters: EngramFilters;
  tags: TagCount[];
  types: string[];
  statuses: string[];
  onChange: (next: {
    type?: string | null;
    status?: string | null;
    tags?: string[];
  }) => void;
}) {
  const [type, setType] = useState(filters.type ?? "");
  const [status, setStatus] = useState(filters.status ?? "");
  const chosen = new Set(filters.tags);

  return (
    <div className="flex flex-col gap-3">
      <form
        className="flex flex-wrap items-end gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          onChange({ type: type.trim(), status: status.trim() });
        }}
      >
        <label className="flex flex-col gap-1 text-xs text-slate-500 dark:text-slate-400">
          Type
          <input
            list="filter-types"
            value={type}
            onChange={(event) => {
              setType(event.target.value);
            }}
            className="w-40 rounded border border-slate-300 bg-white px-2 py-1 text-sm text-slate-900 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100"
          />
          <datalist id="filter-types">
            {types.map((value) => (
              <option key={value} value={value} />
            ))}
          </datalist>
        </label>
        <label className="flex flex-col gap-1 text-xs text-slate-500 dark:text-slate-400">
          Status
          <input
            list="filter-statuses"
            value={status}
            onChange={(event) => {
              setStatus(event.target.value);
            }}
            className="w-40 rounded border border-slate-300 bg-white px-2 py-1 text-sm text-slate-900 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100"
          />
          <datalist id="filter-statuses">
            {statuses.map((value) => (
              <option key={value} value={value} />
            ))}
          </datalist>
        </label>
        <button
          type="submit"
          className="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
        >
          Apply
        </button>
        {hasFilters(filters) && (
          <button
            type="button"
            className="rounded px-2 py-1 text-sm underline underline-offset-2 hover:no-underline"
            onClick={() => {
              setType("");
              setStatus("");
              onChange({ type: null, status: null, tags: [] });
            }}
          >
            Clear filters
          </button>
        )}
      </form>
      {tags.length > 0 && (
        <ul className="flex flex-wrap gap-1.5">
          {tags.map((tag) => {
            const on = chosen.has(tag.name);
            return (
              <li key={tag.name}>
                <button
                  type="button"
                  aria-pressed={on}
                  className={`flex items-baseline gap-1 rounded-full border px-2 py-0.5 text-xs focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none ${
                    on
                      ? "border-sky-600 bg-sky-50 text-sky-800 dark:bg-sky-950 dark:text-sky-200"
                      : "border-slate-200 hover:bg-slate-100 dark:border-slate-800 dark:hover:bg-slate-800"
                  }`}
                  onClick={() => {
                    onChange({
                      tags: on
                        ? filters.tags.filter((name) => name !== tag.name)
                        : [...filters.tags, tag.name],
                    });
                  }}
                >
                  <span>#{tag.name}</span>
                  <span className="text-slate-500 tabular-nums dark:text-slate-400">
                    {tag.engrams}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
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

function detailOf(error: Error): string {
  return error instanceof ApiProblem ? error.detail : error.message;
}
