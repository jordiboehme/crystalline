/**
 * A team domain's proposals: one row each, with the title linking to the review
 * on the forge, where it stands, what the review said and the way to take one
 * back.
 *
 * The sync card beside it says how many there are; this one says which they
 * are, which is the difference between knowing there is work waiting and being
 * able to act on it. Both read the same query - same key, same fetcher - so
 * mounting the pair costs one request rather than two.
 *
 * Drawn only when there is something to draw. A domain with no origin answers
 * 404 and a team domain between proposals answers an empty pair of lists, and
 * neither of those is a state worth a heading and an empty box: the card
 * appears when it has rows, exactly as the sync card appears when it has a
 * status.
 *
 * Feedback is folded away behind a press. A proposal under review carries a
 * thread, and spreading every thread over the card would bury the row below it;
 * what the collapsed row keeps is the verdict, which is the part somebody
 * scanning the card is looking for.
 *
 * The withdraw confirm is behind the house lazy seam, so a session that only
 * reads this card never loads a dialog's worth of code: this file draws rows,
 * and `WithdrawProposalDialog` is what mounts once somebody presses Withdraw.
 */

import { useQuery } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useState } from "react";

import type { SyncProposal } from "../api/admin";
import { fetchSyncStatus, syncStatusKey } from "../api/admin";
import { WithdrawProposalDialog } from "./WithdrawProposalDialog";
import { BUTTON, Chip } from "./primitives";
import type { ChipVariant } from "./primitives";

export function ProposalsCard({
  domain,
}: {
  domain: string;
}): ReactElement | null {
  // The sync card's own query, to the letter: react-query hands the second
  // subscriber the first one's result, so this is a cache read rather than a
  // second call. `retry: false` for the reason it gives - the two answers this
  // call has to distinguish are both immediate and final.
  const status = useQuery({
    queryKey: syncStatusKey(domain),
    queryFn: () => fetchSyncStatus(domain),
    retry: false,
  });

  // One condition covers all three of the nothing-to-draw states: a status
  // still in flight, a first read that was refused (404 or otherwise) and a
  // team domain with no proposals. A refetch that failed after one succeeded
  // keeps its rows, the way the sync card keeps its numbers.
  const proposals = status.data?.proposals ?? [];
  if (proposals.length === 0) {
    return null;
  }

  return (
    <section
      aria-labelledby="domain-proposals"
      className="flex flex-col gap-3 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <h2 id="domain-proposals" className="text-section">
        Proposals
      </h2>
      <ul className="flex flex-col gap-3">
        {proposals.map((proposal) => (
          <ProposalRow
            key={proposal.number}
            domain={domain}
            proposal={proposal}
          />
        ))}
      </ul>
    </section>
  );
}

/**
 * Which face a proposal's standing wears. Anything unrecognized stays plain.
 *
 * Named apart from `primitives.tsx`'s exported `statusVariant`, which this file
 * imports from: that one maps an engram's lifecycle status and these are a
 * proposal's four states, and two different tables under one name in one module
 * is a trap for whoever edits this next.
 */
function proposalStatusVariant(status: string): ChipVariant {
  if (status === "merged") {
    return "positive";
  }
  if (status === "declined" || status === "withdrawn") {
    return "retired";
  }
  return "neutral";
}

/**
 * Which face a review's verdict wears.
 *
 * Changes requested is the caution amber rather than the alert red: it is the
 * reviewer asking for something, which is how review works, not a failure.
 */
function reviewVariant(state: string): ChipVariant {
  if (state === "approved") {
    return "positive";
  }
  return state === "changes_requested" ? "caution" : "neutral";
}

function ProposalRow({
  domain,
  proposal,
}: {
  domain: string;
  proposal: SyncProposal;
}): ReactElement {
  const [confirming, setConfirming] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  return (
    <li className="flex flex-col gap-1 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <a
          href={proposal.url}
          target="_blank"
          rel="noreferrer"
          className="font-medium underline underline-offset-2 hover:no-underline"
        >
          {proposal.title}
        </a>
        <Chip variant={proposalStatusVariant(proposal.status)}>
          {proposal.status}
        </Chip>
        {proposal.reviewState !== null && (
          <Chip variant={reviewVariant(proposal.reviewState)}>
            {/* The wire's underscores are a key, not a word somebody reads. */}
            {proposal.reviewState.replaceAll("_", " ")}
          </Chip>
        )}
        {/* Only on the open list, and only when a reviewer actually moved the
            branch: it is the one fact that has to reach somebody before they
            share into this proposal again. */}
        {proposal.amendedUpstream && (
          <Chip variant="caution">amended upstream</Chip>
        )}
        {/* Secondary: withdrawing is the exception, and the row's own subject
            is the proposal rather than the button. */}
        <button
          type="button"
          onClick={() => {
            setProblem(null);
            setConfirming(true);
          }}
          className={BUTTON.secondary}
        >
          Withdraw
        </button>
      </div>

      {proposal.feedback.length > 0 && (
        <button
          type="button"
          aria-expanded={expanded}
          onClick={() => {
            setExpanded((open) => !open);
          }}
          className="self-start text-caption text-slate-500 underline underline-offset-2 hover:no-underline dark:text-slate-400"
        >
          {expanded
            ? "Hide feedback"
            : `Show feedback (${String(proposal.feedback.length)})`}
        </button>
      )}
      {expanded && (
        <ul className="flex flex-col gap-2 border-l-2 border-slate-200 pl-3 dark:border-slate-700">
          {proposal.feedback.map((item, index) => (
            // Keyed by position as well as instant: two comments can share a
            // timestamp, and the list arrives whole and is never reordered.
            <li key={`${String(index)}-${item.submittedAt ?? ""}`}>
              <p className="flex flex-wrap items-baseline gap-2">
                <span className="font-medium">{item.author}</span>
                {item.path !== null && (
                  <span className="font-mono text-caption text-slate-500 dark:text-slate-400">
                    {item.line === null
                      ? item.path
                      : `${item.path}:${String(item.line)}`}
                  </span>
                )}
              </p>
              {/* Verbatim, wrapping preserved: a review body is somebody's
                  own paragraphs, and reflowing them loses the code and the
                  lists people write reviews in. */}
              <p className="whitespace-pre-wrap">{item.body}</p>
            </li>
          ))}
        </ul>
      )}

      {problem !== null && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problem}
        </p>
      )}

      {confirming && (
        <WithdrawProposalDialog
          domain={domain}
          proposal={proposal}
          onClose={() => {
            setConfirming(false);
          }}
          onProblem={(detail) => {
            // The dialog closes with the refusal, so the message lands on the
            // row: an open dialog `aria-hidden`s the page behind it, and an
            // alert under one reaches nothing that reads the screen.
            setConfirming(false);
            setProblem(detail);
          }}
        />
      )}
    </li>
  );
}
