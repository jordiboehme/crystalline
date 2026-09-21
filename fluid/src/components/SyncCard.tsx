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
import { useEffect, useState } from "react";

import { fetchSyncStatus, syncDomain, syncStatusKey } from "../api/admin";
import { ApiProblem, problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { formatInstant, plural, relativeTime } from "../format";
import { ConflictDialog } from "./ConflictDialog";
import { BUTTON, FOCUS_RING } from "./primitives";

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

/**
 * How many conflicting paths this line draws before it starts counting the
 * rest in words.
 *
 * A copy that drifted for a week can come back with dozens, and every one of
 * them is a button: uncapped, the line becomes a wall of paths that pushes the
 * rest of the card off the screen, and nobody settles thirty conflicts by
 * reading them all at once anyway. Eight is enough to start, and the count in
 * the lead sentence still says how many there really are.
 */
const CONFLICTS_SHOWN = 8;

export function SyncCard({ domain }: { domain: string }): ReactElement | null {
  const queryClient = useQueryClient();
  const [problem, setProblem] = useState<string | null>(null);
  // The conflict being settled, by id, or null while none is. One at a time:
  // settling one is a decision that wants the whole screen.
  const [openConflict, setOpenConflict] = useState<string | null>(null);

  // The clock "Last checked" reads its relative phrase against. A minute
  // tick rather than a refetch: the check itself does not move, only how long
  // ago it reads does, so this redraws the sentence without asking the server
  // anything.
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => {
      setNow(new Date());
    }, 60_000);
    return () => {
      clearInterval(id);
    };
  }, []);

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
            <dd className="tabular-nums" title={sync.lastChecked ?? undefined}>
              {lastChecked(sync, now)}
            </dd>
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
              <span className="flex flex-wrap items-center gap-2">
                {/* The count is the sentence; the paths are what turns it
                    into somewhere to go. A report that carried a bare count,
                    or paths with no id to address a conflict by, still says
                    how many there are - it just has nothing to open, and the
                    colon goes with the list rather than promising one. */}
                {conflictLead(sync)}
                {sync.conflictList.slice(0, CONFLICTS_SHOWN).map((conflict) => (
                  <button
                    key={conflict.id}
                    type="button"
                    onClick={() => {
                      setOpenConflict(conflict.id);
                    }}
                    className={`font-mono underline ${FOCUS_RING}`}
                  >
                    {conflict.path}
                  </button>
                ))}
                {/* Plain text, deliberately: there is nothing behind it to
                    press, and the ones it stands for are settled by working
                    through the eight above until the list runs short. */}
                {sync.conflictList.length > CONFLICTS_SHOWN && (
                  <span className="text-slate-500 dark:text-slate-400">
                    {`and ${String(sync.conflictList.length - CONFLICTS_SHOWN)} more`}
                  </span>
                )}
              </span>
            )}
          </p>
          {/*
            What no share would pick up. In review mode a write joins its
            author's own draft and never reaches the folder the team shares, so
            the line above is true at zero over a pile of unshared work; this is
            the sentence that says so. Drawn only on a domain the server says
            reviews changes - on any other one there is nothing to draw, since
            nobody can draft there.
          */}
          {sync.reviewing && <p className="text-sm">{draftLine(sync)}</p>}
          {/*
            And the coordination half, on the presence of the key rather than on
            anything this side works out: the server sends it to whoever holds
            the domain and to nobody else, which is a per-domain answer a
            browser cannot make. Names and counts, because a draft is unshared
            by definition and what is in it is its author's until they share it.
          */}
          {sync.drafts !== null && sync.drafts.length > 0 && (
            <table className="text-sm">
              <caption className="text-left text-slate-500 dark:text-slate-400">
                Drafts held here
              </caption>
              <tbody>
                {sync.drafts.map((holder) => (
                  <tr key={holder.actor}>
                    <th scope="row" className="pr-4 text-left font-normal">
                      {holder.actor}
                    </th>
                    <td className="tabular-nums">
                      {plural(holder.entries, "draft", "drafts")}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
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

      {openConflict !== null && (
        <ConflictDialog
          domain={domain}
          conflictId={openConflict}
          onClose={() => {
            setOpenConflict(null);
          }}
        />
      )}
    </section>
  );
}

/**
 * What this session is holding in a reviewing domain, in words.
 *
 * Zero says so out loud here rather than staying silent the way the exceptional
 * counts above do, and the difference is what the reader is being told: a
 * reviewing domain is one where work can be under way that the sync numbers
 * cannot see, so "none of it is yours" is an answer somebody came here for. A
 * count the index could not answer says that instead, because it is neither.
 */
function draftLine(sync: { myDrafts: number | null }): string {
  if (sync.myDrafts === null) {
    return "Your drafts here could not be counted.";
  }
  if (sync.myDrafts === 0) {
    return "You are holding no drafts here.";
  }
  return sync.myDrafts === 1
    ? "1 draft change of yours awaits sharing."
    : `${String(sync.myDrafts)} draft changes of yours await sharing.`;
}

/**
 * The lead-in the conflict line wears: the count, and a colon exactly when
 * there are paths after it.
 *
 * The count and the list are read from the same field and can still disagree.
 * The status route embeds the conflicts themselves, but the poll overview
 * counts them and an older report lists bare paths with no id, and a conflict
 * with no id is one nothing on this side can open. So the sentence is drawn
 * from the count either way, and only the buttons depend on the list.
 */
function conflictLead(sync: {
  conflicts: number;
  conflictList: unknown[];
}): string {
  const line = plural(
    sync.conflicts,
    "conflict to settle",
    "conflicts to settle",
  );
  return sync.conflictList.length === 0 ? line : `${line}:`;
}

/**
 * When the origin was last checked, in words, and whether that check still
 * stands.
 *
 * The instant reads as this browser's own local date and time, with how long
 * ago it was alongside it - "2026-09-17 14:32, 13 minutes ago" - because a
 * bare timestamp asks a reader to do the subtraction themselves. A failed
 * probe leaves the timestamp untouched - it is when the check last SUCCEEDED
 * - so that pair alone would read as fresh on a copy that has not reached
 * GitHub since; `(stale)` after it says otherwise. Never checked at all says
 * so instead: a day that does not exist gets neither a time nor a staleness
 * marker.
 */
function lastChecked(
  sync: { lastChecked: string | null; probeError: string | null },
  now: Date,
): string {
  if (sync.lastChecked === null) {
    return "not yet";
  }
  const instant = formatInstant(sync.lastChecked);
  const relative = relativeTime(sync.lastChecked, now);
  const when = relative === null ? instant : `${instant}, ${relative}`;
  return sync.probeError === null ? when : `${when} (stale)`;
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
