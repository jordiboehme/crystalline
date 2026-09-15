/**
 * Whether this domain reviews changes before they land, and the one control
 * that changes it.
 *
 * In review mode every write joins its author's own draft, and the folder the
 * team shares changes only through a proposal somebody reviewed. That is a
 * promise about everybody's writing, so turning it on is the domain owner's
 * decision and turning it off ends every private draft in the domain - which is
 * why the way out is a plan before it is a button.
 *
 * The card draws its state from the domain listing every screen already makes,
 * not from a read of its own: one fact, one source, and a card that cannot
 * disagree with the header beside it about which mode this domain is in.
 *
 * **What it does not do is guess who may press.** The route is gated by the
 * same rule unregistering a domain is - an instance admin, or a private
 * domain's owner - and that is a per-domain fact this side cannot compute
 * (`MembersCard`'s module doc makes the same argument about the same thing). So
 * the control is offered and a refusal renders in the server's own words, which
 * is the pattern this app uses wherever the answer is the server's: ask, and
 * show what happened.
 *
 * `capabilities.readOnly` is the one certainty this side does hold, so the
 * buttons are disabled rather than offered-then-refused on a read-only
 * instance, each carrying the reason as its accessible description.
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useId, useState } from "react";

import { problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import type { FoldChoice, PlannedActor, ReviewPlan } from "../api/review";
import { enableReview, fetchReviewPlan, leaveReview } from "../api/review";
import { useAuth } from "../auth/AuthContext";
import { BUTTON } from "./primitives";

/** Why a control is disabled on an instance that serves reads only. */
const READ_ONLY_REASON =
  "This instance is read only, so nothing here can be changed.";

/** The answer each actor starts at: fold, because ending somebody's unshared work is the choice that should be deliberate. */
const DEFAULT_CHOICE: FoldChoice = "fold";

/**
 * What one actor is holding, as the legend says it.
 *
 * A plain count while every change is a page, which is what it always said.
 * With files among them it says how many, because what folding does to a file
 * is not what it does to a page: the bytes become the team's file rather than
 * a page anybody reads, and somebody answering the plan should not find that
 * out afterwards.
 */
function countOf(row: PlannedActor): string {
  const files = row.drafts.filter((draft) => draft.kind === "file").length;
  if (files === 0) {
    return `${row.entries}`;
  }
  const what = files === 1 ? "1 of them a file" : `${files} of them files`;
  return `${row.entries} draft changes, ${what}`;
}

export function ReviewModeCard({
  domain,
  reviewing,
}: {
  domain: string;
  /** Whether this domain reviews changes, off the listing. */
  reviewing: boolean;
}): ReactElement {
  const { capabilities } = useAuth();
  const queryClient = useQueryClient();
  const headingId = useId();
  const [problem, setProblem] = useState<string | null>(null);
  const [plan, setPlan] = useState<ReviewPlan | null>(null);
  const [choices, setChoices] = useState<Record<string, FoldChoice>>({});
  const readOnly = capabilities.readOnly;

  function settled() {
    setPlan(null);
    setChoices({});
    setProblem(null);
    void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
  }

  const enable = useMutation({
    mutationFn: () => enableReview(domain),
    onSuccess: settled,
    onError: (error: Error) => setProblem(problemDetail(error)),
  });

  const ask = useMutation({
    mutationFn: () => fetchReviewPlan(domain),
    onSuccess: (answer) => {
      setProblem(null);
      setPlan(answer);
      setChoices(
        Object.fromEntries(
          answer.actors.map((row) => [row.actor, DEFAULT_CHOICE]),
        ),
      );
    },
    onError: (error: Error) => setProblem(problemDetail(error)),
  });

  const leave = useMutation({
    mutationFn: () => leaveReview(domain, choices),
    onSuccess: settled,
    onError: (error: Error) => setProblem(problemDetail(error)),
  });

  return (
    <section aria-labelledby={headingId}>
      <h2 id={headingId} className="mb-2 text-section">
        Review mode
      </h2>
      <div className="rounded border border-slate-200 p-4 dark:border-slate-800">
        <p className="text-sm text-slate-600 dark:text-slate-400">
          {reviewing
            ? "Every write in this domain joins its author's own draft. The folder the team shares changes only through a proposal somebody reviewed."
            : "Every write in this domain lands in the folder straight away. Turn review mode on to have each one join its author's own draft until the team has reviewed it."}
        </p>

        {!reviewing && (
          <button
            type="button"
            className={`${BUTTON.primary} mt-3`}
            disabled={readOnly || enable.isPending}
            aria-describedby={readOnly ? `${headingId}-read-only` : undefined}
            onClick={() => enable.mutate()}
          >
            Review changes before they land
          </button>
        )}

        {reviewing && plan === null && (
          <button
            type="button"
            className={`${BUTTON.secondary} mt-3`}
            disabled={readOnly || ask.isPending}
            aria-describedby={readOnly ? `${headingId}-read-only` : undefined}
            onClick={() => ask.mutate()}
          >
            Take review mode off
          </button>
        )}

        {plan !== null && (
          <div className="mt-3">
            {plan.actors.length === 0 ? (
              <p className="text-sm">
                Nobody is drafting in this domain, so taking review mode off
                ends nothing.
              </p>
            ) : (
              <>
                <p className="text-sm">
                  Taking review mode off ends every private draft here. Say what
                  happens to each person&apos;s:
                </p>
                <ul className="mt-2 flex flex-col gap-3">
                  {plan.actors.map((row) => (
                    <li key={row.actor}>
                      <fieldset>
                        <legend className="text-sm font-medium">
                          {row.actor} ({countOf(row)})
                        </legend>
                        <ul className="ml-4 list-disc text-sm text-slate-600 dark:text-slate-400">
                          {row.drafts.map((draft) => (
                            <li key={draft.path}>
                              {draft.tombstone ? "deleted " : "drafted "}
                              {draft.kind === "file" ? "the file " : ""}
                              {draft.path}
                              {draft.conflict != null && (
                                <span className="text-red-700 dark:text-red-300">
                                  {" "}
                                  - {draft.conflict}
                                </span>
                              )}
                            </li>
                          ))}
                        </ul>
                        <div className="mt-1 flex gap-4">
                          {(["fold", "discard"] as FoldChoice[]).map(
                            (choice) => (
                              <label
                                key={choice}
                                className="flex items-center gap-1 text-sm"
                              >
                                <input
                                  type="radio"
                                  name={`${headingId}-${row.actor}`}
                                  value={choice}
                                  checked={
                                    (choices[row.actor] ?? DEFAULT_CHOICE) ===
                                    choice
                                  }
                                  onChange={() =>
                                    setChoices((held) => ({
                                      ...held,
                                      [row.actor]: choice,
                                    }))
                                  }
                                />
                                {choice === "fold"
                                  ? "Write them into the folder"
                                  : "End them"}
                              </label>
                            ),
                          )}
                        </div>
                      </fieldset>
                    </li>
                  ))}
                </ul>
                {plan.contested_paths.map((contested) => (
                  <p
                    key={contested.path}
                    className="mt-2 text-sm text-red-700 dark:text-red-300"
                  >
                    {contested.path} is drafted by{" "}
                    {contested.actors.join(" and ")}, so at most one of them can
                    be written into the folder.
                  </p>
                ))}
                {/*
                  The other kind of trouble, drawn beside the first because it
                  is the one a reader would otherwise meet as a refusal after
                  answering: two people whose different paths answer to one
                  address. See `ContestedAddress`.
                */}
                {plan.contested_addresses.map((contested) => (
                  <p
                    key={contested.permalink}
                    className="mt-2 text-sm text-red-700 dark:text-red-300"
                  >
                    {contested.paths.join(" and ")} answer to the address{" "}
                    {contested.permalink}, drafted by{" "}
                    {contested.actors.join(" and ")}, so at most one of them can
                    be written into the folder: one engram answers to one
                    address.
                  </p>
                ))}
              </>
            )}
            <div className="mt-3 flex gap-2">
              <button
                type="button"
                className={BUTTON.destructive}
                disabled={readOnly || leave.isPending}
                aria-describedby={
                  readOnly ? `${headingId}-read-only` : undefined
                }
                onClick={() => leave.mutate()}
              >
                Take review mode off
              </button>
              <button
                type="button"
                className={BUTTON.ghost}
                onClick={() => {
                  setPlan(null);
                  setChoices({});
                }}
              >
                Keep reviewing
              </button>
            </div>
          </div>
        )}

        {readOnly && (
          <p
            id={`${headingId}-read-only`}
            className="mt-2 text-sm text-slate-500 dark:text-slate-400"
          >
            {READ_ONLY_REASON}
          </p>
        )}
        {problem !== null && (
          <p
            role="alert"
            className="mt-2 text-sm text-red-700 dark:text-red-300"
          >
            {problem}
          </p>
        )}
      </div>
    </section>
  );
}
