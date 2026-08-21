/**
 * Where a team domain stands relative to its GitHub origin, and the one button
 * that closes the gap.
 *
 * Self-contained: it owns its query, so the domain screen mounts it and says
 * nothing else about sync. What it renders is decided by what the server
 * answers rather than by what the screen knows:
 *
 * - a 404 is a domain with no origin, which is most domains, and draws nothing
 *   at all - no card, no notice, no empty state. The status resource does not
 *   exist there, and a local domain has no sync story to tell;
 * - a status that has not landed yet also draws nothing, so the card appears
 *   once when it is known rather than reserving a box that jumps;
 * - any other refusal (GitHub switched off, and so on) keeps the card chrome
 *   and puts the server's own sentence where the numbers would be. The fix for
 *   those lives on the settings screen the message names, so the card quotes it
 *   rather than inventing its own advice.
 *
 * The one thing this card must never do is show stale numbers as fresh. The
 * engine answers a status call rather than failing it when the live check
 * cannot reach GitHub, retrying with no probe at all and reporting why as
 * `probe_error`; every number beside it is then local state alone. So the rows
 * still render - they are true about this copy - under a warning that says the
 * check failed in the server's words, and the checked day wears its staleness.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useState } from "react";

import { fetchSyncStatus, syncDomain, syncStatusKey } from "../api/admin";
import { ApiProblem, problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { formatDay, plural } from "../format";
import { BUTTON } from "./primitives";

/**
 * The warning face, for the one thing here that is neither fine nor a failure:
 * a report that arrived without the check behind it.
 *
 * The caution pair the chips already wear, at body size: amber-800 on
 * amber-100 is 6.41:1 and amber-300 on amber-950 is 10.37:1, both clear of the
 * 4.5:1 floor for text this size. Red is reserved for what actually failed to
 * answer, which is the branch below it.
 */
const STALE_CLASSES =
  "rounded bg-amber-100 px-3 py-2 text-sm text-amber-800 dark:bg-amber-950 dark:text-amber-300";

/** The refusal face, the same one every other screen announces a problem in. */
const ALERT_CLASSES =
  "rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200";

export function SyncCard({ domain }: { domain: string }): ReactElement | null {
  const queryClient = useQueryClient();
  const [problem, setProblem] = useState<string | null>(null);

  // No retry: the two answers this call has to distinguish are both immediate
  // and final - a domain with no origin, and an instance with GitHub off - and
  // retrying either would only delay the card by the backoff.
  const status = useQuery({
    queryKey: syncStatusKey(domain),
    queryFn: () => fetchSyncStatus(domain),
    retry: false,
  });

  const pull = useMutation({
    mutationFn: () => syncDomain(domain),
    onSuccess: () => {
      setProblem(null);
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
    onSettled: () => {
      // Both of the things a pull can have changed: this card's own status,
      // and the listing every sidebar, card and switcher draws from - a pull
      // that applied files moves a domain's engram count and its last sync.
      void queryClient.invalidateQueries({ queryKey: syncStatusKey(domain) });
      void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
    },
  });

  if (status.isPending || isMissing(status.error)) {
    return null;
  }

  // The two halves of an answered call, each named once: a refusal the card
  // quotes, or a report the card reads.
  const failure = status.error;
  const sync = status.data ?? null;
  const refusal = failure === null ? null : problemDetail(failure);
  return (
    // The home card's chrome, on the tag the screen's other blocks use: a
    // labelled `section` is a region somebody navigating by landmark can reach
    // as "Team sync", which an `article` is not, and this is one of the two
    // named blocks of the domain screen rather than a card in a list.
    <section
      aria-labelledby="domain-sync"
      className="flex flex-col gap-3 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <div className="flex flex-wrap items-baseline justify-between gap-3">
        <h2 id="domain-sync" className="text-section">
          Team sync
        </h2>
        {/* Secondary: keeping up with the origin is maintenance, not the act
            this screen is about, and the poller does it unattended anyway. */}
        <button
          type="button"
          disabled={pull.isPending}
          onClick={() => {
            setProblem(null);
            pull.mutate();
          }}
          className={BUTTON.secondary}
        >
          Sync now
        </button>
      </div>

      {/*
        The refusal and the report are not alternatives. A first read that is
        refused leaves nothing to show and this is the whole card; but a read
        that is refused AFTER one succeeded - a "Sync now" against an instance
        whose GitHub was switched off in the meantime - is a card that could
        not be updated, not a card whose facts were withdrawn, so the rows it
        already showed stay under the refusal rather than vanishing from under
        the reader.
      */}
      {refusal !== null && (
        <p role="alert" className={ALERT_CLASSES}>
          {refusal}
        </p>
      )}
      {sync !== null && (
        <>
          {/*
            Only at a literal false. The status route answers with the
            connection rather than refusing over it, so this is the one place
            a disconnected instance is ever told why its report is thin - and
            the probe error above it says the check failed without ever
            naming the cause. A report that carries no connection block says
            nothing, which is not the same as saying no.
          */}
          {sync.connected === false && (
            <p className="text-sm text-slate-500 dark:text-slate-400">
              Not connected - connect GitHub under Settings to sync.
            </p>
          )}
          {sync.probeError !== null && (
            <p role="alert" className={STALE_CLASSES}>
              {`The last origin check failed, so these numbers are this copy's own: ${sync.probeError}`}
            </p>
          )}
          <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
            <dt className="text-slate-500 dark:text-slate-400">Repository</dt>
            <dd className="font-mono">{sync.repo}</dd>
            {sync.branch !== null && (
              <>
                <dt className="text-slate-500 dark:text-slate-400">Branch</dt>
                <dd className="font-mono">{sync.branch}</dd>
              </>
            )}
            <dt className="text-slate-500 dark:text-slate-400">Last checked</dt>
            <dd className="tabular-nums">{lastChecked(sync)}</dd>
          </dl>
          <p className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
            <span>
              {plural(
                sync.localChanges,
                "pending local change",
                "pending local changes",
              )}
            </span>
            <span>
              {plural(sync.openProposals, "open proposal", "open proposals")}
            </span>
            {/* The two exceptional counts, each shown only when it is not
                zero: "0 declined proposals" and "0 conflicts to settle" are
                the normal state of every team domain, and a card that recites
                them teaches a reader to skim past the line that one day says
                something. Declined work is informational, a conflict is
                somebody's next task, and the wording is what says which. */}
            {sync.declinedProposals > 0 && (
              <span>
                {plural(
                  sync.declinedProposals,
                  "declined proposal",
                  "declined proposals",
                )}
              </span>
            )}
            {sync.conflicts > 0 && (
              <span>
                {plural(
                  sync.conflicts,
                  "conflict to settle",
                  "conflicts to settle",
                )}
              </span>
            )}
          </p>
          {/* Only when the origin is actually ahead: `behind` is null when
                nothing probed it, and "not behind" is not a fact then. */}
          {sync.behind === true && (
            <p className="text-sm">
              Behind upstream: the origin has work this copy does not.
            </p>
          )}
        </>
      )}

      {/*
        The pull's own refusal, unless the status is already saying the same
        sentence: an instance with GitHub switched off refuses both calls with
        one message, and two byte-identical alerts read as two problems. A
        pull that failed for its OWN reason still gets its own line - what is
        suppressed is the repetition, not the second cause.
      */}
      {problem !== null && problem !== refusal && (
        <p role="alert" className={ALERT_CLASSES}>
          {problem}
        </p>
      )}
    </section>
  );
}

/**
 * The day the origin was last checked, and whether that day still stands.
 *
 * A failed probe leaves the timestamp untouched - it is when the check last
 * SUCCEEDED - so the day alone would read as "checked this morning" on a copy
 * that has not reached GitHub since. Never checked at all says so instead: a
 * day that does not exist gets no staleness marker.
 */
function lastChecked(sync: {
  lastChecked: string | null;
  probeError: string | null;
}): string {
  if (sync.lastChecked === null) {
    return "not yet";
  }
  const day = formatDay(sync.lastChecked);
  return sync.probeError === null ? day : `${day} (stale)`;
}

/**
 * Whether this failure is the server saying there is nothing at that address.
 *
 * The domain screen's own copy of the same two lines: exporting a helper
 * beside a component is what fast refresh gives up a module over, and one
 * `instanceof` is cheaper than that.
 */
function isMissing(error: unknown): boolean {
  return error instanceof ApiProblem && error.status === 404;
}
