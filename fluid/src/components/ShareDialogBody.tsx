/**
 * Sharing from the browser: what a share would do, read before anybody commits
 * to doing it, and what it did, read back in the same place afterwards.
 *
 * The plan is the reason this is a dialog rather than a button. The server's
 * own word for the action decides everything the dialog says and whether it
 * offers to act at all: only `create` and `update` are shareable, and the other
 * three - nothing to share, conflicts waiting, a proposal a reviewer moved -
 * are states where a share would do nothing or something surprising. Each of
 * them says so in a sentence instead of leaving a live button that fails.
 *
 * An untouched title is not sent. The field is prefilled with the title the
 * server would generate anyway, so echoing it back as an explicit title would
 * change nothing on a create and would rename an open proposal on an update -
 * a rename nobody asked for. Only a title somebody actually wrote travels, and
 * on an update it becomes both the proposal's title and the commit message;
 * leaving it alone keeps the proposal's title and lets the generated line be
 * the commit message.
 *
 * The outcome replaces the form rather than closing the dialog. A share is the
 * one write here whose answer is five different things, three of which mean
 * nothing happened, and closing on that would leave a reader guessing which of
 * them they got.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Dialog } from "radix-ui";
import type { ReactElement } from "react";
import { useId, useState } from "react";

import { fetchShareChanges, shareDomain, syncStatusKey } from "../api/admin";
import { problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { asNumber, asObject, asString } from "../api/json";
import type { ShareDialogProps } from "./ShareDialog";
import { BUTTON, Field } from "./primitives";

const FIELD_CLASSES =
  "w-full rounded border border-slate-300 bg-transparent px-2 py-1 text-sm focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700";

/** The refusal face, the same one every other screen announces a problem in. */
const ALERT_CLASSES =
  "rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200";

export default function ShareDialogBody({
  domain,
  onClose,
}: ShareDialogProps): ReactElement {
  const queryClient = useQueryClient();
  const titleField = useId();
  const descriptionField = useId();
  // `null` is "nobody has typed here", which is what keeps the prefill out of
  // the request; an empty string is a title somebody deliberately cleared.
  const [title, setTitle] = useState<string | null>(null);
  const [description, setDescription] = useState("");
  const [outcome, setOutcome] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  // Always fresh, and never retried: the plan is the whole point of opening
  // this, a cached one would describe a share somebody else's session already
  // made, and the refusals this call can carry - read-only, GitHub off - are
  // immediate and final.
  const plan = useQuery({
    queryKey: ["domains", domain, "share-plan"],
    queryFn: () => fetchShareChanges(domain),
    staleTime: 0,
    retry: false,
  });

  const effectiveTitle = plan.data?.effectiveTitle ?? "";
  const typed = title?.trim() ?? "";
  /** A title of the author's own, as opposed to the prefill handed back. */
  const ownTitle = typed !== "" && typed !== effectiveTitle.trim();

  const share = useMutation({
    mutationFn: () =>
      shareDomain(domain, {
        ...(ownTitle ? { title: typed } : {}),
        ...(description.trim() !== ""
          ? { description: description.trim() }
          : {}),
      }),
    onSuccess: (result) => {
      setOutcome(describeOutcome(result));
      // Both of the things a share can have changed: the status the card that
      // opened this is drawn from, and the listing every sidebar, card and
      // switcher counts engrams in - a share pulls the origin first, and a
      // pull that applied files moves those counts.
      void queryClient.invalidateQueries({ queryKey: syncStatusKey(domain) });
      void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  const action = plan.data?.action ?? null;
  const shareable = action === "create" || action === "update";
  const planProblem = plan.error === null ? null : problemDetail(plan.error);
  const changes = plan.data?.changes ?? [];

  return (
    <Dialog.Root
      open
      onOpenChange={(next) => {
        // Escape and the overlay mean what Cancel means. After a share that
        // landed they mean what Close means, which is the same thing: the
        // outcome has been read and the card behind is already refreshing.
        if (!next) {
          onClose();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-slate-900/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 w-[min(28rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-lg font-semibold">
            Share changes
          </Dialog.Title>
          <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
            {planProblem === null
              ? actionLine(action, plan.data?.number ?? null)
              : "This share could not be planned."}
          </Dialog.Description>
          {outcome === null ? (
            <form
              className="mt-3 flex flex-col gap-3"
              onSubmit={(event) => {
                event.preventDefault();
                if (shareable && !share.isPending) {
                  setProblem(null);
                  share.mutate();
                }
              }}
            >
              {(problem ?? planProblem) !== null && (
                <p role="alert" className={ALERT_CLASSES}>
                  {problem ?? planProblem}
                </p>
              )}
              {changes.length > 0 && (
                <ul className="max-h-40 overflow-y-auto text-sm">
                  {changes.map((change) => (
                    <li key={change.path} className="flex items-baseline gap-2">
                      {/* The verb in a fixed column so the paths line up: a
                          list of files is read down the names, not across. */}
                      <span className="w-16 shrink-0 text-caption text-slate-500 dark:text-slate-400">
                        {change.kind}
                      </span>
                      <span className="font-mono text-xs break-all">
                        {change.path}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              <Field
                id={titleField}
                label="Title"
                {...(action === "update"
                  ? {
                      helper:
                        "Rewriting this renames the proposal; left alone, the proposal keeps its title.",
                    }
                  : {})}
              >
                <input
                  id={titleField}
                  {...(action === "update"
                    ? { "aria-describedby": `${titleField}-help` }
                    : {})}
                  className={FIELD_CLASSES}
                  value={title ?? effectiveTitle}
                  onChange={(event) => {
                    setTitle(event.target.value);
                  }}
                />
              </Field>
              <Field
                id={descriptionField}
                label="Description"
                helper="Optional. The engine writes a summary when this is empty."
              >
                <textarea
                  id={descriptionField}
                  aria-describedby={`${descriptionField}-help`}
                  className={FIELD_CLASSES}
                  rows={3}
                  value={description}
                  onChange={(event) => {
                    setDescription(event.target.value);
                  }}
                />
              </Field>
              <div className="flex justify-end gap-2">
                <button
                  type="button"
                  onClick={onClose}
                  className={BUTTON.secondary}
                >
                  Cancel
                </button>
                {/* The primary tier, and its disabled face is what makes an
                    unshareable plan legible: a filled button gone grey reads
                    as "not now", which is what the sentence above it says. */}
                <button
                  type="submit"
                  disabled={!shareable || share.isPending}
                  className={BUTTON.primary}
                >
                  Share
                </button>
              </div>
            </form>
          ) : (
            <div className="mt-3 flex flex-col gap-3">
              <p className="text-sm">{outcome}</p>
              <div className="flex justify-end">
                <button
                  type="button"
                  autoFocus
                  onClick={onClose}
                  className={BUTTON.primary}
                >
                  Close
                </button>
              </div>
            </div>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * The one sentence the plan earns: what pressing Share would do, or why there
 * is nothing for it to do.
 *
 * A word this side has not been taught reads as the plan still arriving rather
 * than as a verdict invented for it: the button stays disabled either way, and
 * an unknown action is not grounds for telling somebody their work cannot be
 * shared. A plan that was refused never reaches here - the caller says so in
 * the server's own words instead.
 */
function actionLine(action: string | null, number: number | null): string {
  const named =
    number === null ? "the proposal" : `proposal #${String(number)}`;
  switch (action) {
    case "update":
      return `Sharing updates ${named}.`;
    case "create":
      return "Sharing opens a new proposal.";
    case "nothing_to_share":
      return "Nothing to share: the team already has all of this.";
    case "conflicts_pending":
      return "Conflicts need settling before sharing.";
    case "proposal_diverged":
      return `A reviewer amended ${named}; withdraw it or let the review finish.`;
    default:
      return "Working out what a share would do...";
  }
}

/**
 * The one sentence the outcome earns.
 *
 * Read off the engine's own report rather than through a parsed shape, because
 * that is what `shareDomain` hands back: five answers, and the number sits at
 * the top level on a create and inside `proposal` on the two that already have
 * one. Read with the same primitives every `api/` reader uses, so a report
 * that arrives without a number says so in words instead of printing a gap.
 */
function describeOutcome(result: unknown): string {
  const record = asObject(result);
  const outcome = asString(record?.outcome) ?? "";
  const number =
    asNumber(record?.number) ?? asNumber(asObject(record?.proposal)?.number);
  const named =
    number === null ? "the proposal" : `proposal #${String(number)}`;
  switch (outcome) {
    case "updated":
      return `Updated ${named}.`;
    case "proposed":
      return `Opened ${named}.`;
    case "nothing_to_share":
      return "Nothing to share: the team already has all of this.";
    case "conflicts_pending":
      return "Conflicts need settling before sharing. Nothing was shared.";
    case "proposal_diverged":
      return "A reviewer amended the proposal branch, so nothing was shared. Withdraw it or let the review finish.";
    default:
      return "Shared.";
  }
}
