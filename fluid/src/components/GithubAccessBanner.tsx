/**
 * What an instance admin needs to know, on every page, about access on the
 * domains connected through the GitHub App: that GitHub cannot be reached
 * (access then uses the last known roles), and which domains are closed
 * because their repository lacks the App, each with what to do.
 *
 * It reads `GET /github/access`, which asks GitHub nothing, once a minute.
 * Nothing is drawn while everything is fine, for anybody but an admin, or
 * when the read fails: a banner about a problem must not become one.
 */

import { useQuery } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useId } from "react";

import { api } from "../api/client";
import type { GithubAccessResponse } from "../api/model";
import { formatInstant } from "../format";

/** The cache key of the GitHub access read. */
const GITHUB_ACCESS_KEY = ["github-access"] as const;

export function GithubAccessBanner(): ReactElement | null {
  const headingId = useId();
  const access = useQuery({
    queryKey: GITHUB_ACCESS_KEY,
    queryFn: () => api<GithubAccessResponse>("/github/access"),
    refetchInterval: 60_000,
    retry: false,
  });
  const data = access.data;
  if (!data) {
    return null;
  }
  const stale = data.stale_since ?? null;
  if (stale === null && data.closed.length === 0) {
    return null;
  }
  return (
    <div
      role="status"
      aria-labelledby={headingId}
      className="mb-4 flex flex-col gap-1 rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-200"
    >
      <p id={headingId} className="sr-only">
        GitHub access
      </p>
      {stale !== null && (
        <p>
          {`GitHub cannot be reached; access uses the last known roles since ${formatInstant(stale)}.`}
        </p>
      )}
      {data.closed.map((closed) => (
        <p key={closed.domain}>{`${closed.domain}: ${closed.reason}`}</p>
      ))}
    </div>
  );
}
