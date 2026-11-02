/**
 * The end of a printed engram: the Related section. Its own module, read by
 * the engram page only, so its queries stay out of the other pages' chunks.
 * Like every print-only block it carries `hidden` and `data-print` (see
 * `PrintSheet.tsx`).
 */

import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useMemo } from "react";

import type { AssetHome, AttachmentRow } from "../api/files";
import { inboundTitlesQuery } from "../api/inboundTitles";
import { assetRefsIn } from "../editor/imageFormat";
import { formatBytes, plural } from "../format";
import { useEngramAssetHome } from "./useEngramAssetHome";

/** A printed list's small heading. */
const LIST_LABEL =
  "mb-1.5 text-caption font-semibold tracking-[0.08em] text-muted uppercase";

/**
 * The end of a printed engram: what links here, by title and relation, and
 * the files its text references, by name and size. The first page of links
 * prints; a longer list says how many more there are. Nothing prints when
 * there is neither.
 */
export function PrintRelated({
  domain,
  permalink,
  inboundCount,
  body,
}: {
  domain: string;
  permalink: string;
  inboundCount: number;
  body: string;
}): ReactElement | null {
  // The Linked from block's own query: on the reading column the rows are
  // already in the cache, and at full width this asks for them itself.
  const inbound = useInfiniteQuery(
    inboundTitlesQuery(domain, permalink, "", inboundCount > 0),
  );
  const home = useEngramAssetHome(domain, body) as AssetHome;
  const referenced = useMemo(
    () => assetRefsIn(body, home.resolve),
    [body, home],
  );
  const listing = useQuery({
    queryKey: home.key,
    queryFn: () => home.list(),
    enabled: referenced.length > 0,
  });
  const first = inbound.data?.pages[0];
  const seen = new Set<string>();
  const links = (first?.hits ?? []).filter((hit) => {
    const key = `${hit.rel}\u0000${hit.domain}\u0000${hit.permalink}`;
    if (seen.has(key)) {
      return false;
    }
    seen.add(key);
    return true;
  });
  const more = Math.max(0, (first?.total ?? 0) - (first?.hits.length ?? 0));
  const held = new Map<string, AttachmentRow>(
    (listing.data ?? []).map((row) => [row.path, row]),
  );
  if (links.length === 0 && referenced.length === 0) {
    return null;
  }
  const both = links.length > 0 && referenced.length > 0;
  return (
    <section
      hidden
      data-print=""
      aria-label="Related"
      className="print-related mt-10 border-t border-hair-strong pt-3"
    >
      <h2 className="mb-3 text-prose-h3">Related</h2>
      <div className={both ? "grid grid-cols-2 gap-8" : ""}>
        {links.length > 0 && (
          <div>
            <h3 className={LIST_LABEL}>Linked from</h3>
            <ul className="text-sm">
              {links.map((hit) => (
                <li
                  key={`${hit.rel}:${hit.domain}/${hit.permalink}`}
                  className="flex items-baseline justify-between gap-3 border-b border-hair py-1"
                >
                  <span className="min-w-0">
                    {hit.title}
                    {hit.domain !== domain && (
                      <span className="text-muted">{` (${hit.domain})`}</span>
                    )}
                  </span>
                  <span className="shrink-0 font-mono text-caption text-muted">
                    {hit.rel}
                  </span>
                </li>
              ))}
            </ul>
            {more > 0 && (
              <p className="mt-1 text-caption text-muted">
                {`And ${plural(more, "more engram", "more engrams")}.`}
              </p>
            )}
          </div>
        )}
        {referenced.length > 0 && (
          <div>
            <h3 className={LIST_LABEL}>Attachments</h3>
            <ul className="text-sm">
              {referenced.map((path) => {
                const row = held.get(path);
                return (
                  <li
                    key={path}
                    className="flex items-baseline justify-between gap-3 border-b border-hair py-1"
                  >
                    <span className="min-w-0 break-all">
                      {path.slice(path.lastIndexOf("/") + 1)}
                    </span>
                    <span className="shrink-0 text-caption text-muted">
                      {row !== undefined
                        ? formatBytes(row.size)
                        : listing.isSuccess
                          ? "missing"
                          : ""}
                    </span>
                  </li>
                );
              })}
            </ul>
          </div>
        )}
      </div>
    </section>
  );
}
