/**
 * The screen the app opens on: what this instance knows about, and what it has
 * learned lately.
 *
 * The cards come from the same listing the sidebar reads, under the same cache
 * key, so opening the app is one request rather than two answers that can
 * disagree. What a card dates itself by is chosen rather than assumed: the
 * feed knows when an engram was last recorded in a domain, the listing only
 * knows when the domain was last synced, and those are different facts - so
 * whichever is shown says which one it is, and a domain with neither shows no
 * date at all.
 */

import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { Link } from "react-router";

import { problemDetail } from "../api/client";
import { ACTIVITY_QUERY_KEY, fetchActivity } from "../api/activity";
import type { Activity } from "../api/activity";
import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import type { DomainSummary } from "../api/domains";
import { formatDay, plural } from "../format";
import { RETIRED_CLASS, isRetired } from "../lifecycle";
import { domainRoute, engramRoute } from "../paths";

export default function Home() {
  const listing = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });
  const activity = useQuery({
    queryKey: ACTIVITY_QUERY_KEY,
    queryFn: fetchActivity,
  });

  const recorded = useMemo(
    () => lastRecordedByDomain(activity.data),
    [activity.data],
  );

  return (
    <div className="flex flex-col gap-8">
      <header>
        <h1 className="text-xl font-semibold">Home</h1>
        <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
          Crystalline stores what was learned; Fluid is where you think with it.
        </p>
      </header>

      <section aria-labelledby="home-domains">
        <h2 id="home-domains" className="mb-3 text-lg font-semibold">
          Domains
        </h2>
        {listing.isPending && (
          <p className="text-sm text-slate-500 dark:text-slate-400">
            Loading domains
          </p>
        )}
        {/*
          A failed listing is announced once, by the sidebar, which reads the
          same query. Saying it twice would have a screen reader hear the same
          failure twice over.
        */}
        {listing.error && (
          <p className="text-sm text-red-800 dark:text-red-200">
            {problemDetail(listing.error)}
          </p>
        )}
        {listing.data?.domains.length === 0 && (
          <p className="text-sm text-slate-500 dark:text-slate-400">
            No domains are registered on this instance yet.
          </p>
        )}
        <ul className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
          {listing.data?.domains.map((domain) => (
            <li key={domain.name}>
              <DomainCard
                domain={domain}
                lastActivity={recorded.get(domain.name) ?? null}
              />
            </li>
          ))}
        </ul>
      </section>

      <section aria-labelledby="home-activity">
        <h2 id="home-activity" className="mb-1 text-lg font-semibold">
          Recent activity
        </h2>
        <ActivityFeed
          activity={activity.data}
          pending={activity.isPending}
          error={activity.error}
        />
      </section>
    </div>
  );
}

/** One domain, as much as can be said about it from the listing and the feed. */
function DomainCard({
  domain,
  lastActivity,
}: {
  domain: DomainSummary;
  lastActivity: string | null;
}) {
  return (
    <article className="flex h-full flex-col gap-2 rounded border border-slate-200 p-4 dark:border-slate-800">
      <h3 className="text-base font-semibold">
        <Link
          to={domainRoute(domain.name)}
          className="hover:underline focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none"
        >
          {domain.name}
        </Link>
      </h3>
      <p className="flex flex-wrap items-baseline gap-x-3 text-xs text-slate-500 dark:text-slate-400">
        {domain.engrams !== null && (
          <span className="tabular-nums">
            {plural(domain.engrams, "engram", "engrams")}
          </span>
        )}
        {domain.kind !== null && <span>{domain.kind}</span>}
      </p>
      {domain.whenToUse.length > 0 ? (
        <p className="line-clamp-3 text-sm">{domain.whenToUse[0]}</p>
      ) : (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          Its MANIFEST carries no routing line yet.
        </p>
      )}
      <DomainDate domain={domain} lastActivity={lastActivity} />
    </article>
  );
}

/**
 * The one date a card carries, named for the fact it actually is. Nothing is
 * shown when neither fact exists, rather than a placeholder standing in for a
 * date nobody wrote.
 */
function DomainDate({
  domain,
  lastActivity,
}: {
  domain: DomainSummary;
  lastActivity: string | null;
}) {
  if (lastActivity !== null) {
    return (
      <p className="mt-auto text-xs text-slate-500 dark:text-slate-400">
        Last activity {formatDay(lastActivity)}
      </p>
    );
  }
  if (domain.lastSync !== null) {
    return (
      <p className="mt-auto text-xs text-slate-500 dark:text-slate-400">
        Synced {formatDay(domain.lastSync)}
      </p>
    );
  }
  return null;
}

/** What was recorded lately, newest first, as the engine ordered it. */
function ActivityFeed({
  activity,
  pending,
  error,
}: {
  activity: Activity | undefined;
  pending: boolean;
  error: Error | null;
}) {
  if (pending) {
    return (
      <p className="text-sm text-slate-500 dark:text-slate-400">
        Loading recent activity
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
  // The window is the engine's choice, so it is quoted rather than restated.
  const covered = activity?.timeframe ?? null;
  if (!activity || activity.items.length === 0) {
    return (
      <p className="text-sm text-slate-500 dark:text-slate-400">
        {covered === null
          ? "Nothing was recorded in this window."
          : `Nothing was recorded in the last ${covered}.`}
      </p>
    );
  }
  return (
    <>
      <p className="mb-3 text-sm text-slate-500 dark:text-slate-400">
        {covered === null
          ? "The most recent engrams."
          : `Recorded in the last ${covered}.`}
      </p>
      <ul className="flex flex-col divide-y divide-slate-200 dark:divide-slate-800">
        {activity.items.map((item) => (
          <li
            key={`${item.domain}/${item.permalink}`}
            className={`py-2 ${isRetired(item.status) ? RETIRED_CLASS : ""}`}
          >
            <Link
              to={engramRoute(item.domain, item.permalink)}
              className="flex flex-wrap items-baseline gap-x-3 rounded hover:underline focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none"
            >
              <span className="font-medium">{item.title}</span>
              <span className="text-xs text-slate-500 dark:text-slate-400">
                {item.domain}
              </span>
              {item.status !== null && (
                <span className="text-xs text-slate-500 dark:text-slate-400">
                  {item.status}
                </span>
              )}
              {item.recordedAt !== null && (
                <span className="ml-auto text-xs text-slate-500 tabular-nums dark:text-slate-400">
                  {formatDay(item.recordedAt)}
                </span>
              )}
            </Link>
          </li>
        ))}
      </ul>
    </>
  );
}

/** The newest recorded day per domain, out of the feed. */
function lastRecordedByDomain(
  activity: Activity | undefined,
): Map<string, string> {
  const newest = new Map<string, string>();
  for (const item of activity?.items ?? []) {
    if (item.recordedAt === null) {
      continue;
    }
    const held = newest.get(item.domain);
    // ISO days sort as strings, so this is a comparison rather than a parse.
    if (held === undefined || held < item.recordedAt) {
      newest.set(item.domain, item.recordedAt);
    }
  }
  return newest;
}
