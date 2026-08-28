/**
 * Sharing from the browser: what a share would do, read before anybody commits
 * to doing it, and what it did, read back in the same place afterwards.
 *
 * The plan is the reason this is a dialog rather than a button. The server's
 * own word for the action decides everything the dialog says and whether it
 * offers to act at all: `create`, `update`, `stack` and `amend` are shareable,
 * and the other three - nothing to share, conflicts waiting, a proposal a
 * reviewer moved - are states where a share would do nothing or something
 * surprising. Each of them says so in a sentence instead of leaving a live
 * button that fails.
 *
 * Where a chain is open, which layer the share lands on is a choice rather than
 * a verdict, and it is the one choice this dialog adds. Stacking a new layer on
 * top is the default because that is what the engine would do unasked and what
 * keeps each review focused; naming an open layer amends it instead, which is
 * how somebody acts on that layer's review feedback. The layers themselves come
 * off the status the proposals card already read, under the same key: opening
 * this dialog from the card costs nothing, and opening it from the top bar's
 * picker costs one read of a domain the reader is not standing in.
 *
 * An untouched title is not sent. The field is prefilled with the title the
 * server would generate anyway, so echoing it back as an explicit title would
 * change nothing on a create and would rename an open proposal on an update -
 * a rename nobody asked for. Only a title somebody actually wrote travels, and
 * on an update it becomes both the proposal's title and the commit message;
 * leaving it alone keeps the proposal's title and lets the generated line be
 * the commit message. The description does not work that way and the field
 * says so: the engine rebuilds the proposal's body on every update, so an
 * empty description replaces the previous one with a generated summary rather
 * than leaving it standing.
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

import type { SharePlan } from "../api/admin";
import {
  SYNC_SUMMARY_KEY,
  fetchShareChanges,
  fetchSyncStatus,
  readStackPlacement,
  shareDomain,
  sharePlanKey,
  syncStatusKey,
} from "../api/admin";
import { problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { asNumber, asObject, asString } from "../api/json";
import { plural } from "../format";
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
  const proposalField = useId();
  // "" is "stack a new layer on top", which is what the engine does unasked;
  // a number is the open layer somebody chose to amend instead.
  const [target, setTarget] = useState("");
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
  //
  // Switched off the moment a share lands, and that is load bearing rather
  // than tidy: nothing should re-plan a share that already happened, and a
  // refetch that failed would put the planning-error line above an outcome
  // saying the share succeeded.
  //
  // The key deliberately sits outside the `["domains", ...]` family every
  // other read of a domain is filed under, which is the one place this app
  // breaks that pattern. `DOMAINS_QUERY_KEY` is the bare `["domains"]` prefix
  // and TanStack invalidates by prefix, so a plan filed in there would be
  // refetched by every bulk domain invalidation in the app - including this
  // component's own success handler, which fires it in the same tick as the
  // state above and so beats `enabled` to the punch. This is not a cache of
  // domain content: reading it pulls the origin, and re-reading it as a side
  // effect of somebody else's write is a write nobody asked for.
  const plan = useQuery({
    queryKey: sharePlanKey(domain),
    queryFn: () => fetchShareChanges(domain),
    staleTime: 0,
    retry: false,
    enabled: outcome === null,
  });

  // The open layers, off the status the proposals card is drawn from: same
  // key, same fetcher, so mounting this over that card is a cache read. Held
  // rather than refetched while the dialog is open - re-reading it pulls the
  // origin, and nothing about a list of open layers changes because a field
  // was typed in - and never retried, since the refusals it can carry (a
  // domain with no origin, GitHub off) are immediate and final. A domain this
  // read cannot answer for simply offers no layer to amend.
  const status = useQuery({
    queryKey: syncStatusKey(domain),
    queryFn: () => fetchSyncStatus(domain),
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    retry: false,
  });
  const openLayers = (status.data?.proposals ?? []).filter(
    (proposal) => proposal.status === "open",
  );
  const amending = target === "" ? null : Number(target);
  // How many layers the chosen amend would rebuild, counted off the same
  // bottom-first order the chain is reviewed in.
  const chosenIndex = openLayers.findIndex(
    (proposal) => proposal.number === amending,
  );
  const chosenLayersAbove =
    chosenIndex < 0 ? null : openLayers.length - 1 - chosenIndex;

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
        // Only when somebody chose a layer: the engine picks its own target
        // otherwise, and sending the one it would have picked would turn a
        // stack into an amend of the layer under it.
        ...(amending === null ? {} : { proposal: amending }),
      }),
    onSuccess: (result) => {
      setOutcome(describeOutcome(result));
      // All three of the things a share can have changed: the status the card
      // that opened this is drawn from, the listing every sidebar, card and
      // switcher counts engrams in - a share pulls the origin first, and a
      // pull that applied files moves those counts - and the instance-wide
      // summary, which is what the frame's share action reads to decide
      // whether there is anything left to share and what to fill its picker
      // with. That one is the reason this list is not two keys: the work just
      // left this domain, and a button still offering to share it would be
      // offering a dialog that opens to say there is nothing to do.
      void queryClient.invalidateQueries({ queryKey: syncStatusKey(domain) });
      void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
      void queryClient.invalidateQueries({ queryKey: SYNC_SUMMARY_KEY });
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  const action = plan.data?.action ?? null;
  const shareable =
    action === "create" ||
    action === "update" ||
    action === "stack" ||
    action === "amend";
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
            {/*
              Once there is an outcome the header says nothing of its own. The
              plan's line is written in the future tense - "Sharing updates
              proposal #4." - and left standing it would sit directly above a
              sentence saying that share already happened, which reads as the
              dialog contradicting itself about the one thing it is for. The
              outcome below is the whole answer, so this steps out of its way
              rather than paraphrasing it in a second voice.
            */}
            {outcome !== null
              ? "Done."
              : planProblem === null
                ? // A chosen layer is the plan now: the server planned the
                  // target it would have picked, and saying that line over a
                  // choice somebody just made would describe a different
                  // share from the one the button would send.
                  amending === null
                  ? actionLine(plan.data ?? null)
                  : amendLine(amending, chosenLayersAbove)
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
              {openLayers.length > 0 && (
                <Field id={proposalField} label="Proposal">
                  <select
                    id={proposalField}
                    className={FIELD_CLASSES}
                    value={target}
                    onChange={(event) => {
                      setTarget(event.target.value);
                    }}
                  >
                    {/* The engine's own default, first and selected: each
                        share gets its own focused review, and reviewers land
                        the chain by merging the top. */}
                    <option value="">New proposal (stack on top)</option>
                    {openLayers.map((layer) => (
                      <option key={layer.number} value={String(layer.number)}>
                        Amend #{String(layer.number)} - {layer.title}
                      </option>
                    ))}
                  </select>
                </Field>
              )}
              {amending !== null && (
                // The one thing somebody amending a layer has to know, and it
                // is general rather than a list of paths: the engine knows
                // which files the layers above claim, and a change to one of
                // them belongs in the layer that claimed it - put lower, it
                // is simply overwritten by the layer above.
                <p className="text-caption text-slate-500 dark:text-slate-400">
                  Changes to files a higher layer already touched belong in that
                  layer instead.
                </p>
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
                // The clause matters next to the title's: the body is
                // rewritten on every update whether or not anybody typed
                // here, so the title's "left alone, it keeps what it has" must
                // not be generalized into a description that survives.
                helper="Optional. The engine writes a summary when this is empty; on an update it replaces the proposal's previous description either way."
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
 * What amending a named layer would do, in the same voice the plan speaks in.
 *
 * Written here rather than left to the plan, because the plan is about the
 * target the server picked and this is about the one somebody chose instead.
 * The layers above are named when there are any: amending under them rebuilds
 * work that is already in front of reviewers, which is the whole difference
 * between amending the top layer and amending one below it.
 */
function amendLine(number: number, layersAbove: number | null): string {
  const named = `proposal #${String(number)}`;
  return layersAbove === null || layersAbove === 0
    ? `Sharing amends ${named}.`
    : `Sharing amends ${named} and re-bases ${plural(layersAbove, "layer", "layers")} above it.`;
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
function actionLine(plan: SharePlan | null): string {
  const {
    action = null,
    number = null,
    count = null,
    topNumber = null,
    layersAbove = null,
  } = plan ?? {};
  const named =
    number === null ? "the proposal" : `proposal #${String(number)}`;
  switch (action) {
    case "update":
      return `Sharing updates ${named}.`;
    case "create":
      return "Sharing opens a new proposal.";
    case "stack":
      // The layer it lands on is the whole difference between a stack and a
      // lone proposal, so it is named rather than implied.
      return topNumber === null
        ? "Will stack a new proposal on top of the open one."
        : `Will stack a new proposal on top of #${String(topNumber)}.`;
    case "amend":
      return number === null
        ? "Sharing amends the open proposal."
        : amendLine(number, layersAbove);
    case "nothing_to_share":
      return "Nothing to share: the team already has all of this.";
    case "conflicts_pending":
      // With the number when the report carried one: how much is waiting is
      // the difference between settling it now and coming back later.
      return count === null
        ? "Conflicts need settling before sharing."
        : `${plural(count, "conflict needs", "conflicts need")} settling before sharing.`;
    case "proposal_diverged":
      return `A reviewer amended ${named}; withdraw it or let the review finish.`;
    default:
      return "Working out what a share would do...";
  }
}

/**
 * Where the proposal that just landed sits in its chain, as a clause to hang
 * off its name, or the empty string when there is no chain worth naming.
 *
 * Two rules, and they are the CLI's own to the letter. The position is what
 * decides whether this is a layer at all, never the stack number: on the
 * stacked path a chain whose linking call has not landed carries real
 * positions with no number, and "stack #null" would be worse than saying
 * nothing about the number. And a chain of one open layer is not a chain a
 * reader needs told about, so a lone proposal reads exactly as it always did.
 */
function placementLine(payload: unknown): string {
  const { stackNumber, stackPosition } = readStackPlacement(payload);
  if (stackPosition === null) {
    return "";
  }
  const [layer, open] = stackPosition;
  if (open < 2) {
    return "";
  }
  const where = `, layer ${String(layer)} of ${String(open)}`;
  return stackNumber === null
    ? `${where} (stack link pending)`
    : `${where} on stack #${String(stackNumber)}`;
}

/**
 * The one sentence the outcome earns.
 *
 * Read off the engine's own report rather than through a parsed shape, because
 * that is what `shareDomain` hands back: five answers, and the number sits at
 * the top level on a create and inside `proposal` on the two that already have
 * one. Read with the same primitives every `api/` reader uses, so a report
 * that arrives without a number says so in words instead of printing a gap.
 *
 * The two answers that landed also say where in the chain they landed, and the
 * two rules for saying it are the ones {@link readStackPlacement} carries: the
 * position is the gate, and a chain of one open layer is not a chain anybody
 * needs told about.
 */
function describeOutcome(result: unknown): string {
  const record = asObject(result);
  const outcome = asString(record?.outcome) ?? "";
  // A `proposed` report carries its placement at the top level and an
  // `updated` one inside `proposal`, the same split the number follows.
  const proposal = asObject(record?.proposal);
  const number = asNumber(record?.number) ?? asNumber(proposal?.number);
  const named =
    number === null ? "the proposal" : `proposal #${String(number)}`;
  const placed = `${named}${placementLine(proposal ?? record)}`;
  switch (outcome) {
    case "updated":
      return `Updated ${placed}.`;
    case "proposed":
      return `Opened ${placed}.`;
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
