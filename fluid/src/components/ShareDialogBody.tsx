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
 * The form is ordered the way the decision is actually made: which layer the
 * share lands on, then the sentence saying what landing there would do, then
 * the files that would travel, and last the wording somebody writes for them.
 * The picker comes first because it is what rewrites that sentence - asked
 * after it, it would leave a line describing a share nobody is making any more.
 * The files are grouped by kind rather than listed flat, in {@link ChangeList}:
 * an evolve pass or an ingest shares hundreds at once, and the shape of that -
 * three added, a hundred and twenty-one modified - is what a reader decides on.
 * The generated folder listings a share carries alongside them are counted into
 * one line there rather than grouped, for the same reason: they are what keeps
 * the team repository browsable, never what somebody is deciding about.
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
import { useEffect, useId, useState } from "react";

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
import { ChangeList } from "./ChangeList";
import type { ShareDialogProps } from "./ShareDialog";
import { ConnectToShare, SharingAs } from "./ShareIdentityAction";
import { BUTTON, Field } from "./primitives";
import { useShareIdentity } from "./useShareIdentity";

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
  // refetched by every bulk domain invalidation in the app - this component's
  // own included, and `enabled` alone would not save it, since an invalidation
  // fired in the same tick as the state above reaches an observer React has
  // not re-rendered yet. This is not a cache of domain content: reading it
  // pulls the origin, and re-reading it as a side effect of somebody else's
  // write is a write nobody asked for.
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
  //
  // Switched off the moment a share lands, for the reason the plan above is
  // and one of its own. This component invalidates the status key and the
  // `["domains"]` prefix that also covers it, and an observer still live would
  // answer that by pulling the origin again to redraw a select that is no
  // longer on screen - the outcome pane has replaced the form by then. The
  // card behind the dialog is the reader that wants the fresh status, and it
  // gets it: what is switched off here is this dialog's second copy of it.
  // This flag only holds because those invalidations are fired from an effect
  // rather than from the mutation's handler; the note on them says why.
  const status = useQuery({
    queryKey: syncStatusKey(domain),
    queryFn: () => fetchSyncStatus(domain),
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    retry: false,
    enabled: outcome === null,
  });
  // Whose credential the share would go out on, off the same status: in the
  // mode where that is the acting person's own, a session without one is
  // offered the way to get one instead of a button the engine would refuse.
  // Switched off with the two queries above, and for the same reason.
  const identity = useShareIdentity(domain, outcome === null);

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
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  // All three of the things a share can have changed: the status the card that
  // opened this is drawn from, the listing every sidebar, card and switcher
  // counts engrams in - a share pulls the origin first, and a pull that applied
  // files moves those counts - and the instance-wide summary, which is what the
  // frame's share action reads to decide whether there is anything left to
  // share and what to fill its picker with. That one is the reason this list is
  // not two keys: the work just left this domain, and a button still offering
  // to share it would be offering a dialog that opens to say there is nothing
  // to do.
  //
  // Fired from an effect keyed on the outcome rather than from the mutation's
  // own success handler, and that is load bearing rather than tidy. The status
  // query above is one of the keys being invalidated - `syncStatusKey` sits
  // under the `["domains"]` prefix, so both of the first two reach it - and
  // firing them inside `onSuccess` would reach an observer that is still
  // enabled, because React has not re-rendered with the outcome yet. The dialog
  // would answer its own invalidation by pulling the origin again to redraw a
  // select that the outcome pane has already replaced. By the time this runs
  // the component has re-rendered, this observer is off, and the reader that
  // actually wants the fresh status - the proposals card behind the dialog -
  // is the one left to answer.
  useEffect(() => {
    if (outcome === null) {
      return;
    }
    void queryClient.invalidateQueries({ queryKey: syncStatusKey(domain) });
    void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
    void queryClient.invalidateQueries({ queryKey: SYNC_SUMMARY_KEY });
  }, [outcome, domain, queryClient]);

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
          {/*
            Once there is an outcome the header says nothing of its own. The
            plan's line is written in the future tense - "Sharing updates
            proposal #4." - and left standing it would sit directly above a
            sentence saying that share already happened, which reads as the
            dialog contradicting itself about the one thing it is for. The
            outcome below is the whole answer, so this steps out of its way
            rather than paraphrasing it in a second voice.

            While the form is up the description lives inside it instead, under
            the layer picker: which layer the share lands on is what decides
            what the sentence says, so the choice is asked before the sentence
            that answers it. Exactly one of the two is ever mounted, so the
            dialog is described once either way.
          */}
          {outcome !== null && (
            <Dialog.Description className="mt-1 text-sm text-slate-500 dark:text-slate-400">
              Done.
            </Dialog.Description>
          )}
          {outcome === null ? (
            <form
              className="mt-3 flex flex-col gap-3"
              onSubmit={(event) => {
                event.preventDefault();
                // The button's own conditions, because a form submits on
                // Enter as well as on a press - including from a field, in a
                // dialog whose primary action is a link rather than a submit.
                if (
                  shareable &&
                  !share.isPending &&
                  !identity.mustConnect &&
                  !identity.asking
                ) {
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
                    {/* Newest layer first, which is the reverse of the order
                        the report sends and the same order the card draws.
                        The chain is reviewed bottom up, but the layer somebody
                        is most likely to amend is the one they last shared -
                        the one at the top - and a picker that buried it at the
                        far end of a long chain would put the likely choice
                        furthest from the default. */}
                    {[...openLayers].reverse().map((layer) => (
                      <option key={layer.number} value={String(layer.number)}>
                        Amend #{String(layer.number)} - {layer.title}
                      </option>
                    ))}
                  </select>
                </Field>
              )}
              {/* The sentence the picker above decides: a chosen layer is the
                  plan now, and saying the server's line over a choice somebody
                  just made would describe a different share from the one the
                  button would send. */}
              <Dialog.Description className="text-sm text-slate-500 dark:text-slate-400">
                {planProblem === null
                  ? amending === null
                    ? actionLine(plan.data ?? null)
                    : amendLine(amending, chosenLayersAbove)
                  : "This share could not be planned."}
              </Dialog.Description>
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
              {/* What would travel, after what it would travel into: the
                  grouping is what keeps a sweep's worth of files readable. */}
              <ChangeList changes={changes} />
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
              <div className="flex flex-wrap items-center justify-end gap-2">
                {identity.sharingAs !== null && (
                  <SharingAs login={identity.sharingAs} />
                )}
                <button
                  type="button"
                  onClick={onClose}
                  className={BUTTON.secondary}
                >
                  Cancel
                </button>
                {/* The primary tier, and its disabled face is what makes an
                    unshareable plan legible: a filled button gone grey reads
                    as "not now", which is what the sentence above it says.
                    Where the engine would refuse this session's share for
                    want of an identity, the fix takes the same place: the
                    plan above stays exactly as it is, because reading it
                    needed nobody's credential. */}
                {identity.mustConnect ? (
                  <ConnectToShare />
                ) : (
                  <button
                    type="submit"
                    disabled={!shareable || share.isPending || identity.asking}
                    className={BUTTON.primary}
                  >
                    Share
                  </button>
                )}
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
