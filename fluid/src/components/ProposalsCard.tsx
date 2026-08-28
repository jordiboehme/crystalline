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
 * Drawn once the status behind it has answered, and not before: a domain with
 * no origin answers 404, and neither that nor a status still in flight is a
 * state worth a heading. A team domain between proposals is, though, because
 * the way to make one is in this card's header - a card that vanished when the
 * list emptied would take the share button with it, exactly when somebody has
 * something to share.
 *
 * Feedback is folded away behind a press. A proposal under review carries a
 * thread, and spreading every thread over the card would bury the row below it;
 * what the collapsed row keeps is the verdict, which is the part somebody
 * scanning the card is looking for.
 *
 * Both dialogs are behind the house lazy seam, so a session that only reads
 * this card never loads a dialog's worth of code: this file draws rows, and
 * `WithdrawProposalDialog` and `ShareDialog` are what mount once somebody
 * presses Withdraw or Share changes.
 *
 * A chain of stacked proposals is drawn as what it is: the open layers come in
 * chain order, bottom first, each wearing where it sits, and the chain itself
 * is named once above them. Two rules decide that, and they are the same two
 * the CLI's own renderer follows. One: a lone proposal stands in no chain a
 * reader needs told about, so nothing says "layer 1 of 1". Two: the position is
 * the gate and the stack number is named only when there is one - a chain whose
 * linking call has not landed carries real positions with no number, and
 * "stack #null" would be worse than saying nothing about the number at all.
 */

import { useQuery } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useState } from "react";

import type { SyncProposal } from "../api/admin";
import { fetchSyncStatus, syncStatusKey } from "../api/admin";
import { ShareDialog } from "./ShareDialog";
import { WithdrawProposalDialog } from "./WithdrawProposalDialog";
import { BUTTON, Chip } from "./primitives";
import type { ChipVariant } from "./primitives";

export function ProposalsCard({
  domain,
}: {
  domain: string;
}): ReactElement | null {
  const [sharing, setSharing] = useState(false);

  // The sync card's own query, to the letter: react-query hands the second
  // subscriber the first one's result, so this is a cache read rather than a
  // second call. `retry: false` for the reason it gives - the two answers this
  // call has to distinguish are both immediate and final.
  const status = useQuery({
    queryKey: syncStatusKey(domain),
    queryFn: () => fetchSyncStatus(domain),
    retry: false,
  });

  // Both of the nothing-to-draw states in one condition: a status still in
  // flight, and a first read that was refused (404 for a domain with no
  // origin, and anything else the same way - the sync card beside this one is
  // where a refusal is reported). A refetch that failed after one succeeded
  // keeps its rows, the way that card keeps its numbers.
  const answered = status.data;
  if (!answered) {
    return null;
  }
  const proposals = answered.proposals;
  // The open layers in chain order, bottom first, which is the order the
  // report sends them in and the order reviewers merge them in.
  const open = proposals.filter((proposal) => proposal.status === "open");
  const chained = open.length > 1;
  // The number is named only when there is one. On the stacked path it is null
  // for as long as the call that groups the layers on the forge has not
  // landed, and the debt below says so instead.
  const stacked = chained && answered.stackNumber !== null;
  const linkPending =
    answered.stackLinkPending || (chained && answered.stackNumber === null);

  return (
    <section
      aria-labelledby="domain-proposals"
      className="flex flex-col gap-3 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <div className="flex flex-wrap items-baseline justify-between gap-3">
        <div className="flex flex-wrap items-baseline gap-2">
          <h2 id="domain-proposals" className="text-section">
            Proposals
          </h2>
          {/* The chain named once, beside its own heading, rather than
              repeated on every row that belongs to it. */}
          {stacked && (
            <Chip variant="accent">stack #{String(answered.stackNumber)}</Chip>
          )}
        </div>
        {/* Primary, and in the header rather than beside a row: sharing is
            what somebody opens this card to do, and it is about the domain
            rather than about any one proposal already on it. */}
        <button
          type="button"
          onClick={() => {
            setSharing(true);
          }}
          className={BUTTON.primary}
        >
          Share changes
        </button>
      </div>
      <ChainNotices
        wedged={answered.stackWedged}
        repairPending={answered.repairPending}
        linkPending={linkPending}
      />
      {proposals.length === 0 ? (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          No open proposals.
        </p>
      ) : (
        <ul className="flex flex-col gap-3">
          {proposals.map((proposal) => {
            // Where this one sits, worked out once from the open list rather
            // than carried per row: the report orders the chain and says how
            // it stands, and a row is a position in that order.
            const layer = open.indexOf(proposal);
            return (
              <ProposalRow
                key={proposal.number}
                domain={domain}
                proposal={proposal}
                position={
                  chained && layer >= 0 ? [layer + 1, open.length] : null
                }
                // How much a withdraw would rebuild. Only the open layers
                // above this one, and only for a layer that is itself open: a
                // declined layer's place in the chain is not something this
                // report says, so nothing is claimed about it.
                layersAbove={layer >= 0 ? open.length - 1 - layer : 0}
              />
            );
          })}
        </ul>
      )}
      {sharing && (
        <ShareDialog
          domain={domain}
          onClose={() => {
            setSharing(false);
          }}
        />
      )}
    </section>
  );
}

/**
 * What the chain itself is owed, said in the words of the verbs that settle it.
 *
 * Three lines, none of which prints while the chain is sound. A wedged layer is
 * named by number, one line each, because that number is what a reader
 * withdraws or shares against and a chain cannot grow until one of them does.
 * The two debts below it are not blockages at all: they are work the next write
 * finishes by itself, so they read as notes rather than as warnings.
 */
function ChainNotices({
  wedged,
  repairPending,
  linkPending,
}: {
  wedged: number[];
  repairPending: boolean;
  linkPending: boolean;
}): ReactElement | null {
  if (wedged.length === 0 && !repairPending && !linkPending) {
    return null;
  }
  return (
    <div className="flex flex-col gap-1 text-sm">
      {wedged.map((number) => (
        <p
          key={number}
          className="rounded bg-amber-50 px-3 py-2 text-amber-900 dark:bg-amber-950 dark:text-amber-200"
        >
          Stack wedged by #{String(number)} - withdraw it or share again to
          repair the chain.
        </p>
      ))}
      {repairPending && (
        <p className="text-caption text-slate-500 dark:text-slate-400">
          Repair pending - the next share or withdraw finishes it.
        </p>
      )}
      {linkPending && (
        <p className="text-caption text-slate-500 dark:text-slate-400">
          Stack link pending - the next share or status check finishes it.
        </p>
      )}
    </div>
  );
}

/**
 * Whether a proposal's url is an address this card will hand a reader.
 *
 * The url is the forge's own word, and on an enterprise install the forge is a
 * machine somebody else administers. Nothing about a review needs a scheme
 * other than http or https, so anything else - `javascript:` first among them -
 * is drawn as text instead of as a link that runs on press. Defence in depth
 * rather than a known hole: the engine builds these from the API's own fields,
 * and this is the line that holds if one day it does not.
 */
function isWebAddress(url: string): boolean {
  return url.startsWith("https://") || url.startsWith("http://");
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
  position,
  layersAbove,
}: {
  domain: string;
  proposal: SyncProposal;
  /** `[layer, open layers]`, 1-based, or null when there is no chain to name. */
  position: [number, number] | null;
  /** How many open layers a withdraw here would re-base. */
  layersAbove: number;
}): ReactElement {
  const [confirming, setConfirming] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  return (
    <li className="flex flex-col gap-1 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        {/* Where this layer sits, before what it is called: the chain is read
            bottom-up, and the position is what makes the order legible. */}
        {position !== null && (
          <Chip>
            layer {String(position[0])} of {String(position[1])}
          </Chip>
        )}
        {isWebAddress(proposal.url) ? (
          <a
            href={proposal.url}
            target="_blank"
            rel="noreferrer"
            className="font-medium underline underline-offset-2 hover:no-underline"
          >
            {proposal.title}
          </a>
        ) : (
          // The title is still worth reading; the link is not worth having.
          <span className="font-medium">{proposal.title}</span>
        )}
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
            setNotice(null);
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

      {/* What a withdraw that landed could not do. Not a refusal - the
          proposal is closed - so it is announced politely rather than as an
          alert, and it lands here for the same reason a refusal does: the
          dialog is gone by the time there is anything to say. */}
      {notice !== null && (
        <p
          role="status"
          className="rounded bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:bg-amber-950 dark:text-amber-200"
        >
          {notice}
        </p>
      )}

      {confirming && (
        <WithdrawProposalDialog
          domain={domain}
          proposal={proposal}
          layersAbove={layersAbove}
          onClose={() => {
            setConfirming(false);
          }}
          onNotice={setNotice}
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
