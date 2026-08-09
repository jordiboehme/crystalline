/**
 * The MANIFEST, read on its own page.
 *
 * Everybody who can open the domain can read this screen; only
 * `capabilities.canAdminister` sees the Edit link, the same gate the editor
 * itself enforces if the address is typed directly. The detail read - the
 * markdown plus the checksum an edit needs - is cached under its own key
 * rather than `manifestKey`, which `DomainHome` already reads under a plainer
 * shape; the two never collide because neither ever writes into the other's
 * entry.
 */

import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "react-router";

import { problemDetail } from "../api/client";
import { fetchManifestDetail, manifestDetailKey } from "../api/domain";
import { useAuth } from "../auth/AuthContext";
import { Markdown } from "../components/Markdown";
import { manifestEditRoute } from "../paths";

/** Warm the editor chunk while the pointer is still on its way to the click. */
function prefetchEditor(): void {
  void import("./ManifestEditor");
}

export default function ManifestPage() {
  const { domain = "" } = useParams();
  const { capabilities } = useAuth();

  const detail = useQuery({
    queryKey: manifestDetailKey(domain),
    queryFn: () => fetchManifestDetail(domain),
  });

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
    return (
      <p className="text-sm text-slate-500 dark:text-slate-400">
        Loading the manifest
      </p>
    );
  }

  const empty = detail.data.markdown.trim() === "";

  return (
    <div className="flex flex-col gap-4">
      <header className="flex flex-wrap items-baseline justify-between gap-3">
        <h1 className="text-xl font-semibold">{domain} MANIFEST</h1>
        {/*
          Offered whether the MANIFEST is empty or not: an admin looking at
          nothing needs exactly this link to fix that, and an admin looking
          at prose needs it to change it. One link earns both jobs rather
          than a second one repeating itself inside the empty state below.
        */}
        {capabilities.canAdminister && (
          <Link
            to={manifestEditRoute(domain)}
            onPointerEnter={prefetchEditor}
            onFocus={prefetchEditor}
            className="rounded border border-slate-300 px-2 py-0.5 text-xs hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
          >
            Edit MANIFEST
          </Link>
        )}
      </header>
      {empty ? (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          This domain has no MANIFEST yet, so nothing tells an agent what it is
          for.
        </p>
      ) : (
        <article>
          <Markdown source={detail.data.markdown} />
        </article>
      )}
    </div>
  );
}
