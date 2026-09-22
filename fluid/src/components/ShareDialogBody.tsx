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
 * button that fails. On a domain whose MANIFEST says `sharing: direct` the
 * same plan reads `commit`, the sentence says the change goes straight to the
 * branch with no review, the button says which branch, and `proposal_open` is
 * the one state a direct domain adds: a leftover proposal in the way.
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
 * The generated folder listings a share carries alongside them are not drawn
 * at all: an `index.md` is rebuilt from the engrams beside it and follows the
 * domain's own configuration, so it is never what somebody is deciding about.
 *
 * Deciding also means being able to look first, and to change your mind. A
 * press on a path replaces the form with that file's diff until Escape or
 * "Back to the list" brings the form back with every tick where it was; the
 * menu on a row discards that one file, and "Discard selected" discards the
 * ticked set. Both ask first, in a strip under the list naming what goes -
 * never a typed confirmation, because the files name themselves - and both
 * post the digest each file wore when the question was asked, so a file
 * somebody has edited since is refused instead of being thrown away. A refusal lands on its own row, and
 * the plan is read again afterwards, so the list says what is actually left.
 *
 * Which of those files travel is a choice too, and on a shared instance it is
 * the choice that matters most: the delta in front of somebody may be half
 * their colleague's afternoon. Every file carries a box, and the boxes open
 * where the guess is best - where the plan says this session's own account
 * last wrote something, exactly those files start ticked and a quiet line says
 * how much was left out; where it says nothing about anybody, everything
 * starts ticked, which is what this dialog has always done. It is a guess and
 * it is meant to be corrected: one press on a group heading takes the whole
 * group back in. A share of everything sends no file list at all, so the
 * common case is the request it always was.
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
import { useEffect, useId, useMemo, useState } from "react";

import type { DiscardReceipt, SharePlan } from "../api/admin";
import {
  SYNC_SUMMARY_KEY,
  discardChanges,
  fetchShareChanges,
  fetchSyncStatus,
  localChangeKey,
  localChangesKey,
  readStackPlacement,
  refusalSentence,
  shareDomain,
  sharePlanKey,
  syncStatusKey,
} from "../api/admin";
import { problemDetail } from "../api/client";
import { domainTreeKey } from "../api/domain";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { domainEngramsRoot } from "../api/engrams";
import { asNumber, asObject, asString } from "../api/json";
import { useAuth } from "../auth/AuthContext";
import { isWebAddress, plural } from "../format";
import { ChangeList } from "./ChangeList";
import { DiffPane } from "./DiffPaneLazy";
import { DiscardConfirm } from "./DiscardConfirm";
import type { ShareDialogProps } from "./ShareDialog";
import { ConnectToShare, SharingAs } from "./ShareIdentityAction";
import { preselect, substantive } from "./changes";
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
  only,
}: ShareDialogProps): ReactElement {
  const queryClient = useQueryClient();
  // Whose work the boxes open ticked for. The session's own account, which is
  // what the engine records as `human:<name>` when this person writes an
  // engram through it; an anonymous reader has none, and everything opens
  // ticked for them exactly as it always did.
  const { user, capabilities } = useAuth();
  const account = user?.name ?? null;
  /**
   * Whether this session may put a file back the way the team has it. The
   * capability the provider already resolved, never re-derived here: a
   * read-only instance refuses every content write whoever is asking, and a
   * reader who may not write is offered no control that would be refused.
   */
  const mayDiscard = capabilities.canWrite && !capabilities.readOnly;
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
  const [outcome, setOutcome] = useState<OutcomeSentence | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  // `null` is "nobody has touched a box", which is what lets the preselection
  // below stay in charge while the plan is still arriving and re-arriving. A
  // set - empty included - is a choice somebody made.
  // A caller that opened this about one file hands its path in, and that is a
  // choice rather than an absence: `only` seeds the ticks, so the preselection
  // below never gets a say. An empty list is still a choice - exactly these,
  // and there are none - which is what leaves the Share button disabled.
  const [picked, setPicked] = useState<ReadonlySet<string> | null>(() =>
    only === undefined ? null : new Set(only),
  );
  /** The path whose diff is up, or null while the form is. */
  const [pane, setPane] = useState<string | null>(null);
  /**
   * What a confirmed discard would take, and where to put the keyboard back.
   *
   * The digests are captured here, when the question is asked, rather than
   * read off the plan when it is answered. The plan is refetched behind this
   * strip - by a window coming back, by the card behind it, by this dialog's
   * own invalidation - and a file edited while the question was on the screen
   * would otherwise be discarded against its new digest, which is exactly the
   * edit the guard exists to refuse.
   */
  const [arming, setArming] = useState<{
    targets: { path: string; sha: string | null }[];
    from: HTMLElement | null;
  } | null>(null);
  /** Why the server refused a path last time, drawn on that path's own row. */
  const [refusals, setRefusals] = useState<ReadonlyMap<string, string>>(
    new Map(),
  );
  /** What the last discard did, said in one line above the list. */
  const [receipt, setReceipt] = useState<DiscardReceipt | null>(null);
  /** A discard that failed outright, said inside the strip that asked. */
  const [discardProblem, setDiscardProblem] = useState<string | null>(null);
  const paneHeading = useId();

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

  // What would travel, and which of it is ticked. The plan lists every
  // unshared change; the boxes decide which of them this share carries, and
  // the generated folder listings are never among them - the engine carries
  // the listing of any folder a chosen file lives in, whoever asked.
  const changes = plan.data?.changes ?? [];
  const real = substantive(changes);
  // Keyed on the plan itself rather than on its change array, which is a
  // fresh array on every render while the plan is still arriving.
  const preset = useMemo(
    () => preselect(plan.data?.changes ?? [], account),
    [plan.data, account],
  );
  const selected: ReadonlySet<string> = picked ?? new Set(preset.paths);
  /** Everything a reader could choose is chosen: the share it always was. */
  const allPicked = real.every((change) => selected.has(change.path));
  /** Nothing is: there is a decision to make before there is a share. */
  const nothingPicked =
    real.length > 0 && !real.some((change) => selected.has(change.path));

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
        // And only when the ticks are a subset. A share of everything sends
        // no file list at all, so the ordinary case is byte for byte the
        // request it has always been - and the folder listings the engine
        // carries along stay the engine's business either way.
        ...(allPicked
          ? {}
          : {
              files: real
                .filter((change) => selected.has(change.path))
                .map((change) => change.path),
            }),
      }),
    onSuccess: (result) => {
      setOutcome(describeOutcome(result));
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  /**
   * Putting the ticked files back the way the team has them.
   *
   * Every target carries the digest its row wore when the strip armed, so a
   * file somebody has edited since is refused by name rather than having that
   * edit thrown away. The rows leave the held plan the moment the receipt
   * says they are gone, and the refetch the effect below fires is what confirms
   * it against the engine.
   */
  const discard = useMutation({
    mutationFn: (targets: { path: string; sha: string | null }[]) =>
      discardChanges(domain, targets),
    onSuccess: (result) => {
      const gone = new Set([
        ...result.restored,
        ...result.deleted,
        ...result.cleared.map((entry) => entry.path),
      ]);
      // The rows leave the held plan at once; the refetch below confirms it.
      queryClient.setQueryData<SharePlan>(sharePlanKey(domain), (held) =>
        held
          ? { ...held, changes: held.changes.filter((c) => !gone.has(c.path)) }
          : held,
      );
      setPicked((current) => {
        if (current === null) return current;
        const now = new Set(current);
        for (const path of gone) now.delete(path);
        return now;
      });
      setRefusals(
        new Map(result.refused.map((r) => [r.path, refusalSentence(r.reason)])),
      );
      // A file the open pane is showing was refused because it moved under
      // this reader: the two sides they are looking at are the old ones.
      if (
        result.refused.some(
          (r) => r.path === pane && r.reason === "changed_since",
        )
      ) {
        void queryClient.invalidateQueries({
          queryKey: localChangeKey(domain, pane ?? ""),
        });
      }
      setArming(null);
      setReceipt(result);
    },
    onError: (error: Error) => {
      setDiscardProblem(problemDetail(error));
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
    // The plan too, which nothing refetches while this dialog is up - the
    // query above is switched off by the outcome - and which is exactly why
    // it is invalidated here: the next opening reads a fresh plan rather than
    // the one describing the share that just landed.
    void queryClient.invalidateQueries({ queryKey: sharePlanKey(domain) });
    // A direct commit in a reviewing domain folds the caller's own drafts
    // into the branch on its way out, which moves files under every list on
    // the screen. The same pair a withdraw's revert invalidates, for the
    // same reason: this is the working tree moving, not a count.
    if (outcome.folded) {
      void queryClient.invalidateQueries({ queryKey: domainTreeKey(domain) });
      void queryClient.invalidateQueries({
        queryKey: domainEngramsRoot(domain),
      });
    }
  }, [outcome, domain, queryClient]);

  // What a discard can have changed, keyed on its receipt for the reason the
  // share's is keyed on the outcome: the plan this dialog is drawn from, the
  // sync status the card behind it reads, the listing every surface counts
  // engrams in, the instance-wide summary the frame's share action is drawn
  // from, and the offline change list a page outside this dialog reads. Fired
  // from here rather than from the mutation's own handler, so the plan query
  // answers the invalidation on a render that already knows what is gone.
  useEffect(() => {
    if (receipt === null) {
      return;
    }
    void queryClient.invalidateQueries({ queryKey: sharePlanKey(domain) });
    void queryClient.invalidateQueries({ queryKey: syncStatusKey(domain) });
    void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
    void queryClient.invalidateQueries({ queryKey: SYNC_SUMMARY_KEY });
    void queryClient.invalidateQueries({ queryKey: localChangesKey(domain) });
  }, [receipt, domain, queryClient]);

  const action = plan.data?.action ?? null;
  const shareable =
    action === "create" ||
    action === "update" ||
    action === "stack" ||
    action === "amend" ||
    action === "commit";
  const planProblem = plan.error === null ? null : problemDetail(plan.error);

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
        {/* Wider while a diff is up, because a diff is two columns of text
            and the form is a column of fields; back to the form's own width
            the moment the pane closes. */}
        <Dialog.Content
          className={`fixed top-1/2 left-1/2 z-50 ${
            pane === null
              ? "w-[min(28rem,calc(100vw-2rem))]"
              : "w-[min(64rem,calc(100vw-2rem))]"
          } -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900`}
          onEscapeKeyDown={(event) => {
            // Escape closes the one thing the reader opened last, and only
            // leaves the dialog when that is all there is. Answered here
            // rather than inside the pane or the strip, because Radix listens
            // for Escape on the document in the capture phase: an event
            // stopped further in has already been seen and acted on.
            if (pane !== null) {
              event.preventDefault();
              setPane(null);
              return;
            }
            if (arming !== null) {
              event.preventDefault();
              const from = arming.from;
              setArming(null);
              from?.focus();
            }
          }}
        >
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
          {outcome !== null ? (
            <div className="mt-3 flex flex-col gap-3">
              <p className="text-sm">
                {outcome.before}
                {outcome.link !== null && (
                  <a
                    href={outcome.link.href}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="font-medium underline underline-offset-2 hover:no-underline"
                  >
                    {outcome.link.label}
                  </a>
                )}
                {outcome.after}
              </p>
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
          ) : pane !== null ? (
            // One file, both sides, in place of the form: the ticks and the
            // fields are held in state, so coming back costs nothing and
            // changes nothing.
            <div className="mt-3 flex flex-col gap-3">
              <h2 id={paneHeading} className="font-mono text-sm break-all">
                {pane}
              </h2>
              <DiffPane domain={domain} path={pane} headingId={paneHeading} />
              <div className="flex justify-end">
                <button
                  type="button"
                  autoFocus
                  onClick={() => {
                    setPane(null);
                  }}
                  className={BUTTON.secondary}
                >
                  Back to the list
                </button>
              </div>
            </div>
          ) : (
            <form
              className="mt-3 flex flex-col gap-3"
              onSubmit={(event) => {
                event.preventDefault();
                // The button's own conditions, because a form submits on
                // Enter as well as on a press - including from a field, in a
                // dialog whose primary action is a link rather than a submit.
                if (
                  shareable &&
                  !nothingPicked &&
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
              {/* What the last discard did, once, above the list it changed.
                  Announced rather than merely drawn: the rows it took away
                  are gone from under the reader's pointer, and the count is
                  what says so. Why a path stayed is said on that path's own
                  row instead, where the reader is already looking. */}
              {receipt !== null && (
                <p
                  role="status"
                  className="text-caption text-slate-500 dark:text-slate-400"
                >
                  {receiptLine(receipt)}
                </p>
              )}
              {/* No layer to choose on a direct domain: a proposal is never
                  the target. */}
              {openLayers.length > 0 && plan.data?.sharing !== "direct" && (
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
                  grouping is what keeps a sweep's worth of files readable,
                  and the boxes are what make it a choice. The hint is shown
                  only while nobody has touched them - once somebody has, it
                  describes a state that is no longer on the screen. */}
              <ChangeList
                changes={changes}
                selected={selected}
                hint={picked === null ? preset.hint : null}
                onToggle={(path, next) => {
                  setPicked((current) => {
                    const now = new Set(current ?? selected);
                    if (next) {
                      now.add(path);
                    } else {
                      now.delete(path);
                    }
                    return now;
                  });
                }}
                onToggleGroup={(paths, next) => {
                  setPicked((current) => {
                    const now = new Set(current ?? selected);
                    for (const path of paths) {
                      if (next) {
                        now.add(path);
                      } else {
                        now.delete(path);
                      }
                    }
                    return now;
                  });
                }}
                onOpen={setPane}
                refusals={refusals}
                onDiscard={
                  mayDiscard
                    ? (path, from) => {
                        setDiscardProblem(null);
                        setArming({
                          targets: [
                            {
                              path,
                              sha:
                                changes.find((c) => c.path === path)?.sha ??
                                null,
                            },
                          ],
                          from,
                        });
                      }
                    : undefined
                }
              />
              {arming !== null && (
                <DiscardConfirm
                  question={
                    arming.targets.length === 1
                      ? `Discard ${arming.targets[0]?.path ?? ""}?`
                      : `Discard ${plural(arming.targets.length, "file", "files")}?`
                  }
                  pending={discard.isPending}
                  problem={discardProblem}
                  onConfirm={() => {
                    setDiscardProblem(null);
                    discard.mutate(arming.targets);
                  }}
                  onCancel={() => {
                    const from = arming.from;
                    setArming(null);
                    from?.focus();
                  }}
                />
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
                helper={
                  plan.data?.sharing === "direct"
                    ? "Optional. Becomes the body of the commit message."
                    : "Optional. The engine writes a summary when this is empty; on an update it replaces the proposal's previous description either way."
                }
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
                {/* Destructive and secondary at once: it is not what this
                    dialog is for, and it is the only way back out of work
                    somebody does not want to share at all. */}
                {mayDiscard && (
                  <button
                    type="button"
                    disabled={
                      nothingPicked || real.length === 0 || discard.isPending
                    }
                    onClick={(event) => {
                      setDiscardProblem(null);
                      setArming({
                        targets: real
                          .filter((change) => selected.has(change.path))
                          .map((change) => ({
                            path: change.path,
                            sha: change.sha,
                          })),
                        from: event.currentTarget,
                      });
                    }}
                    className={`${BUTTON.secondary} text-red-700 dark:text-red-300`}
                  >
                    Discard selected
                  </button>
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
                    disabled={
                      !shareable ||
                      nothingPicked ||
                      share.isPending ||
                      identity.asking
                    }
                    className={BUTTON.primary}
                  >
                    {action === "commit"
                      ? `Commit to ${plan.data?.branch ?? "the branch"}`
                      : "Share"}
                  </button>
                )}
              </div>
            </form>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * What a discard did, in one line.
 *
 * The count rather than the paths: the rows those paths were on have just left
 * the list, and naming them again would be a second list of things that are no
 * longer there. A path that stayed says why on its own row, so a discard where
 * everything was refused has nothing left to report but that.
 */
function receiptLine(receipt: DiscardReceipt): string {
  const gone =
    receipt.restored.length + receipt.deleted.length + receipt.cleared.length;
  return gone === 0
    ? "Nothing was discarded."
    : `Discarded ${plural(gone, "file", "files")}.`;
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
    branch = null,
    title = null,
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
    case "commit":
      return `Sharing commits straight to ${branch ?? "the branch"}, with no review.`;
    case "proposal_open":
      // The one state a direct domain adds: the branch this share would
      // commit onto is the one a proposal is still waiting to land on.
      return number === null
        ? "A proposal is still open; merge or withdraw it before sharing directly."
        : `Proposal #${String(number)} (${title ?? "untitled"}) is still open; merge or withdraw it before sharing directly.`;
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
 * The outcome sentence, split around the one thing in it worth a click.
 *
 * A plain string can't carry a link, and the outcome names a proposal that
 * has one on every outcome that lands: `before` and `after` are the sentence
 * with a hole in the middle, `link` is what fills it - `{ label, href }` for
 * an anchor, or `null` when there is nothing to link, in which case `before`
 * already carries the whole sentence and `after` is empty.
 */
interface OutcomeSentence {
  before: string;
  link: { label: string; href: string } | null;
  after: string;
  /**
   * Whether the share folded the caller's own drafts into the domain, which
   * is a direct commit in a reviewing domain and nothing else. It moves files
   * on disk rather than counts, so the caller invalidates the lists that draw
   * them; every other outcome leaves the working tree where it was.
   */
  folded?: boolean;
}

/** A sentence with nothing to link. */
function plainSentence(text: string): OutcomeSentence {
  return { before: text, link: null, after: "" };
}

/**
 * A sentence with one segment worth a click: `${before}${label}${after}`, the
 * label an anchor when `url` passes the same address screen the proposals
 * card links a title through, and plain text otherwise.
 */
function linkedSentence(
  before: string,
  label: string,
  after: string,
  url: string | null,
): OutcomeSentence {
  if (url !== null && isWebAddress(url)) {
    return { before, link: { label, href: url }, after };
  }
  return plainSentence(`${before}${label}${after}`);
}

/**
 * A sentence naming a proposal by number, with the number itself as the one
 * segment a click on it should go anywhere: `${prefix}proposal #N${suffix}`,
 * the `#N` a link when `url` passes the same address screen the proposals
 * card links a title through, plain text otherwise or when there is no
 * number to name at all.
 */
function numberSentence(
  prefix: string,
  number: number | null,
  suffix: string,
  url: string | null,
): OutcomeSentence {
  if (number === null) {
    return plainSentence(`${prefix}the proposal${suffix}`);
  }
  return linkedSentence(
    `${prefix}proposal `,
    `#${String(number)}`,
    suffix,
    url,
  );
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
 *
 * The number itself is the link, on every outcome that names one and whose
 * url passes {@link isWebAddress} - the same screen the proposals card links
 * a title through. The placement clause after it (`, layer 2 of 3 on stack
 * #12`) is never a link: the report carries no address for a stack, only its
 * number, so there is nothing there to point a reader at.
 */
function describeOutcome(result: unknown): OutcomeSentence {
  const record = asObject(result);
  const outcome = asString(record?.outcome) ?? "";
  // A `proposed` report carries its placement and url at the top level and
  // an `updated` one inside `proposal`, the same split the number follows.
  const proposal = asObject(record?.proposal);
  const number = asNumber(record?.number) ?? asNumber(proposal?.number);
  switch (outcome) {
    case "updated":
      return numberSentence(
        "Updated ",
        number,
        `${placementLine(proposal ?? record)}.`,
        asString(proposal?.url),
      );
    case "proposed":
      return numberSentence(
        "Opened ",
        number,
        `${placementLine(proposal ?? record)}.`,
        asString(record?.url),
      );
    case "nothing_to_share":
      return plainSentence(
        "Nothing to share: the team already has all of this.",
      );
    case "conflicts_pending":
      return plainSentence(
        "Conflicts need settling before sharing. Nothing was shared.",
      );
    case "committed": {
      // The commit itself is what a reader follows, by the short sha every
      // forge names one by; a report that carried none says what it did
      // without inventing an address for it.
      const sha = asString(record?.sha) ?? "";
      const short = sha.slice(0, 7);
      const branch = asString(record?.branch) ?? "the branch";
      const folded = asNumber(record?.drafts_folded) !== null;
      const sentence =
        short === ""
          ? plainSentence(`Committed to ${branch}.`)
          : linkedSentence(
              "Committed ",
              short,
              ` to ${branch}.`,
              asString(record?.url),
            );
      return { ...sentence, folded };
    }
    // The three a direct domain refuses with. Each carries the server's own
    // guidance, which names the verb that settles it, so nothing is
    // paraphrased here.
    case "proposal_open":
    case "branch_protected":
    case "branch_moved":
      return plainSentence(asString(record?.guidance) ?? "Nothing was shared.");
    case "proposal_diverged":
      return number === null
        ? plainSentence(
            "A reviewer amended the proposal branch, so nothing was shared. Withdraw it or let the review finish.",
          )
        : numberSentence(
            "A reviewer amended ",
            number,
            "'s branch, so nothing was shared. Withdraw it or let the review finish.",
            asString(proposal?.url),
          );
    default:
      return plainSentence("Shared.");
  }
}
