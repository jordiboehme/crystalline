/**
 * Maintenance: what the knowledge needs next.
 *
 * A report, not a workbench. The sweep behind it reads every registered domain
 * and changes nothing, and neither does looking at it: there is no button here
 * that edits an engram, because the work a finding names is work somebody does
 * in the engram itself, having read it. So every finding links to the engram it
 * fired on, and that link is the whole of what this screen asks of a reader.
 *
 * Three buttons are the exception, and each of them is about the QUEUE rather
 * than about the knowledge. Acknowledging rules a finding intentional so
 * future sweeps stop raising it; taking that back un-rules it; and deleting an
 * orphaned attachment settles the one finding whose whole resolution is that
 * the file should not exist. None of them fires on arrival, on a refetch or on
 * a toggle - only on a press.
 *
 * What an acknowledgment silences is never hidden. The count of it rides every
 * sweep, it is said under the queue whether or not anybody asks, and the toggle
 * beside it fetches the silenced rows and draws them muted with the note that
 * silenced each one. An acknowledgment given for evidence that has since moved
 * is not silence at all: that finding comes back saying so.
 *
 * A finding that names no engram - an orphaned attachment, a domain's drifted
 * tag vocabulary - is drawn like any other, with its subject in plain text
 * rather than as a link. Fabricating a link to an engram that does not exist
 * would be exactly the thing those findings are about, and dropping the rows
 * instead (which this screen used to do) left the tally counting findings
 * nobody could see.
 *
 * The queue arrives ranked across the whole result and is drawn under the
 * catalog's three families, in the catalog's own order, because the shape of a
 * backlog is what somebody opening this page came to see - a flat hundred rows
 * would answer a different question. The family of a row is read off its rule
 * id (`api/evolve.ts` says why), and a rule from a catalog newer than this
 * client is drawn under its own heading rather than guessed into one of the
 * three.
 *
 * Two things the page refuses to blur. A judgment finding wears its class on
 * its face, because it is a question for a person rather than a change to
 * apply. And an empty queue is good news: it is said in the words of good news
 * and never in the shape of a failure.
 *
 * The domain filter is a lens over what already arrived rather than a second
 * sweep. Its choices come from the findings themselves, so it offers the
 * domains that actually have something waiting, and it keeps offering all of
 * them once one of them is chosen.
 */

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { Link } from "react-router";

import { problemDetail } from "../api/client";
import type { EvolveFinding, EvolveQueue } from "../api/evolve";
import {
  EVOLVE_FAMILIES,
  EVOLVE_FAMILY_BLURBS,
  EVOLVE_FAMILY_TITLES,
  EVOLVE_KEY_ROOT,
  EVOLVE_STALE_MS,
  acknowledgeFinding,
  evolveFamily,
  evolveKey,
  fetchEvolveQueue,
  unacknowledgeFinding,
} from "../api/evolve";
import { deleteAttachment } from "../api/files";
import { useAuth } from "../auth/AuthContext";
import { Skeleton } from "../components/Skeleton";
import { BUTTON, Chip, FIELD, TOGGLE } from "../components/primitives";
import type { ChipVariant } from "../components/primitives";
import { plural } from "../format";
import { engramRoute } from "../paths";

/** Every domain, as the filter writes it. */
const EVERY_DOMAIN = "";

/** The rule whose subject is an attachment nothing references. */
const ORPHAN_ATTACHMENT_RULE = "V108";

/**
 * One group of findings under one heading.
 *
 * `title` rather than a family value, because the last group is the one whose
 * rules this client does not recognize and which therefore has no family at
 * all.
 */
interface FindingGroup {
  key: string;
  title: string;
  blurb: string;
  findings: EvolveFinding[];
}

export default function Maintenance() {
  const [domain, setDomain] = useState(EVERY_DOMAIN);
  const [showAcknowledged, setShowAcknowledged] = useState(false);
  const { capabilities } = useAuth();
  const queryClient = useQueryClient();
  // Fetched on arrival, on the Refresh button, when the acknowledged rows are
  // asked for or given back, and after a write that changes what the queue
  // holds. Never on anything else: the app's default is to refetch a stale
  // query whenever the window comes back, which is right for a listing and
  // wrong for this, because the sweep reads every engram of every domain and
  // alt-tabbing to a page somebody left open is not somebody asking for it
  // again. Both doors are shut rather than one, because they are different
  // doors - the flag stops a focus from re-sweeping a page that is already
  // open, and the freshness window stops the remount that following a finding
  // to its engram and coming back would otherwise cost.
  const sweep = useQuery({
    queryKey: evolveKey([], showAcknowledged),
    queryFn: () => fetchEvolveQueue({ includeAcknowledged: showAcknowledged }),
    staleTime: EVOLVE_STALE_MS,
    refetchOnWindowFocus: false,
    // Asking for the silenced rows is a new question with its own cache entry,
    // so the answer to the old one is held while the new one is on its way:
    // otherwise pressing the toggle replaces the whole queue with a skeleton,
    // including the toggle that was just pressed.
    placeholderData: (previous) => previous,
  });

  /** What a write does afterwards: ask the queue again, both ways of asking. */
  const resweep = async () => {
    await queryClient.invalidateQueries({ queryKey: EVOLVE_KEY_ROOT });
  };

  const queue = sweep.data;
  const findings = queue?.queue ?? [];
  const shown =
    domain === EVERY_DOMAIN
      ? findings
      : findings.filter((finding) => finding.domain === domain);
  const groups = groupByFamily(shown);
  // Fed from the whole sweep rather than from what is on screen, so choosing
  // one domain does not take the others off the list that offered them.
  const domains = [...new Set(findings.map((finding) => finding.domain))].sort(
    (left, right) => left.localeCompare(right),
  );
  const instructions = new Map(
    (queue?.actions ?? []).map((action) => [action.rule, action.instruction]),
  );

  return (
    <div className="flex flex-col gap-6">
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex flex-col gap-1">
          <h1 className="text-display">
            Maintenance - what the knowledge needs next
          </h1>
          <p className="text-sm text-slate-500 dark:text-slate-400">
            A sweep of every registered domain, ranked. Reading it changes
            nothing: a finding names what to go and read, and the work mostly
            happens there.
          </p>
        </div>
        <button
          type="button"
          className={BUTTON.secondary}
          disabled={sweep.isFetching}
          onClick={() => {
            void sweep.refetch();
          }}
        >
          Refresh
        </button>
      </header>

      {queue && (
        <div className="flex flex-wrap items-end justify-between gap-3">
          <span className="flex items-center gap-2">
            <label
              htmlFor="maintenance-domain"
              className="text-xs text-slate-500 dark:text-slate-400"
            >
              Domain
            </label>
            <select
              id="maintenance-domain"
              value={domain}
              onChange={(event) => {
                setDomain(event.target.value);
              }}
              className={FIELD}
            >
              <option value={EVERY_DOMAIN}>Every domain</option>
              {domains.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </select>
          </span>
          <Tally queue={queue} shown={shown.length} scoped={domain} />
        </div>
      )}

      {sweep.isPending && <Skeleton label="Sweeping the domains" rows={6} />}

      {sweep.error && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(sweep.error)}
        </p>
      )}

      {queue && shown.length === 0 && (
        <Nothing scanned={queue.engramsScanned} scoped={domain} />
      )}

      {groups.map((group) => (
        <FamilySection
          key={group.key}
          group={group}
          instructions={instructions}
          canWrite={capabilities.canWrite}
          onChanged={resweep}
        />
      ))}

      {queue && queue.acknowledged.total > 0 && (
        <Acknowledged
          total={queue.acknowledged.total}
          showing={showAcknowledged}
          onToggle={() => {
            setShowAcknowledged((was) => !was);
          }}
        />
      )}

      {queue && queue.truncations.length > 0 && (
        <p className="text-caption text-slate-500 dark:text-slate-400">
          {/*
            Quiet, and never left out: a cap that fired is the difference
            between a short queue and a finished one.
          */}
          Some domains were capped, so this is not the whole of what is waiting:{" "}
          {queue.truncations.join("; ")}.
        </p>
      )}
    </div>
  );
}

/**
 * What the sweep read and what it found, on one line.
 *
 * The family counts are the engine's own, over the whole result rather than
 * over the page, and the section headings count the rows actually drawn. Those
 * two numbers wear the same word and are both right, so whenever they can
 * differ - a cap that kept part of the result off the page, a domain filter
 * that narrowed what is drawn - the breakdown is named as the whole queue
 * rather than left to be read as a second count of the page. It is kept rather
 * than dropped in exactly those cases, because the shape of everything waiting
 * is most worth saying when the page is not all of it.
 *
 * The count of what is drawn names the base it counts against for the same
 * reason: "1 finding in ops" on its own is a true sentence that has lost the
 * only not-all-of-it signal on the screen.
 */
function Tally({
  queue,
  shown,
  scoped,
}: {
  queue: EvolveQueue;
  shown: number;
  scoped: string;
}) {
  // What arrived, which is the page rather than the result. A domain filter
  // counts out of this, and it is said out loud whenever it is less than the
  // whole: a count with no base is what makes "1 finding in ops" sound like
  // the end of the matter.
  const fetched = queue.queue.length;
  const breakdown = queue.families
    .map((count) => `${familyTitle(count.family)} ${String(count.findings)}`)
    .join(", ");
  const page =
    fetched < queue.total
      ? `${String(fetched)} of ${plural(queue.total, "finding", "findings")}`
      : plural(queue.total, "finding", "findings");
  const narrowed =
    scoped === EVERY_DOMAIN ? "" : `, ${String(shown)} of them in ${scoped}`;
  // Whether what is drawn is less than the whole result, by a cap or a filter
  // or both. That is exactly when the engine's counts and the headings above
  // the rows can differ, and exactly when the breakdown has to say which it is.
  const partial = shown < queue.total;
  return (
    <p className="text-caption text-slate-500 tabular-nums dark:text-slate-400">
      {plural(queue.engramsScanned, "engram", "engrams")} swept, {page}
      {narrowed}
      {breakdown === ""
        ? "."
        : partial
          ? `. Everything waiting: ${breakdown}.`
          : `. ${breakdown}.`}
    </p>
  );
}

/**
 * A queue with nothing in it.
 *
 * Two different nothings, and only one of them is about the knowledge base: a
 * clean sweep is good news, while a filter that matches nothing is a fact about
 * the filter. Neither is a failure, and neither wears one.
 */
function Nothing({ scanned, scoped }: { scanned: number; scoped: string }) {
  return (
    <p className="rounded border border-dashed border-slate-300 px-3 py-6 text-sm text-slate-600 dark:border-slate-700 dark:text-slate-300">
      {scoped === EVERY_DOMAIN
        ? `Nothing is waiting. The sweep read ${plural(scanned, "engram", "engrams")} and found nothing that needs attention.`
        : `Nothing is waiting in ${scoped}. Other domains may still have something; choose every domain to see it.`}
    </p>
  );
}

/**
 * What acknowledgments are holding out of the queue, and the way to look at it.
 *
 * Under the rows rather than over them, beside the note about any cap that
 * fired: both answer the same question - is this the whole of what is waiting -
 * and neither is what somebody opening the page came to read first. It is drawn
 * only above zero, because "0 acknowledged findings are staying quiet" is a
 * sentence about nothing.
 */
function Acknowledged({
  total,
  showing,
  onToggle,
}: {
  total: number;
  showing: boolean;
  onToggle: () => void;
}) {
  const counted = plural(
    total,
    "acknowledged finding is",
    "acknowledged findings are",
  );
  return (
    <p className="text-caption flex flex-wrap items-center gap-2 text-slate-500 dark:text-slate-400">
      <span>
        {showing
          ? `${counted} shown above, muted.`
          : `${counted} staying quiet.`}
      </span>
      <button
        type="button"
        aria-pressed={showing}
        className={showing ? TOGGLE.on : TOGGLE.off}
        onClick={onToggle}
      >
        {showing ? "Hide them" : "Show them"}
      </button>
    </p>
  );
}

/** One family, its findings under it. */
function FamilySection({
  group,
  instructions,
  canWrite,
  onChanged,
}: {
  group: FindingGroup;
  instructions: Map<string, string>;
  canWrite: boolean;
  onChanged: () => Promise<void>;
}) {
  const headingId = `maintenance-${group.key}`;
  return (
    <section aria-labelledby={headingId} className="flex flex-col gap-2">
      <div className="flex flex-col gap-0.5">
        <h2 id={headingId} className="text-section">
          {group.title}{" "}
          <span className="text-caption font-normal text-slate-500 tabular-nums dark:text-slate-400">
            {plural(group.findings.length, "finding", "findings")}
          </span>
        </h2>
        <p className="text-caption text-slate-500 dark:text-slate-400">
          {group.blurb}
        </p>
      </div>
      <ul className="flex flex-col gap-2">
        {group.findings.map((finding) => (
          <FindingRow
            key={`${finding.domain}/${finding.permalink}/${finding.rule}/${String(finding.n)}`}
            finding={finding}
            instruction={instructions.get(finding.rule) ?? null}
            canWrite={canWrite}
            onChanged={onChanged}
          />
        ))}
      </ul>
    </section>
  );
}

/** Which question a row currently has open, if any. */
type Asking = "ack" | "delete";

/**
 * One finding: what fired, where, what was read to find it, on request the
 * catalog's own instruction for the rule, and the one or two things that can
 * be settled about it from here.
 *
 * The instruction is folded away rather than printed on every row, because it
 * is a paragraph written for whoever is about to do the work and the row above
 * it is what somebody scanning the queue reads. Each row holds its own open
 * state: the instruction belongs to the rule, but the act of asking for it
 * belongs to the finding in front of the reader.
 *
 * The subject is a link only when there is an engram behind it. An orphaned
 * attachment and a domain's drifted vocabulary have none, so their subject is
 * plain text - and for the same reason they are offered no acknowledgment: an
 * acknowledgment is an entry in an engram's frontmatter, and there is no
 * engram to put one in.
 *
 * The one destructive action lives here rather than on the attachment panel of
 * some engram, because no engram references this file - that is what the
 * finding says. It asks first, by name: the bytes do not come back.
 */
function FindingRow({
  finding,
  instruction,
  canWrite,
  onChanged,
}: {
  finding: EvolveFinding;
  instruction: string | null;
  canWrite: boolean;
  onChanged: () => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [asking, setAsking] = useState<Asking | null>(null);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const noteId = useId();

  const anchored = finding.permalink !== "";
  const orphanAttachment = finding.rule === ORPHAN_ATTACHMENT_RULE;
  // Re-acknowledging is the same write: the server recomputes the scope, so
  // the entry it replaces is the one that stopped matching.
  const ackLabel = finding.ackStale ? "Re-acknowledge" : "Acknowledge";

  /** Run one write, then ask the queue what it looks like now. */
  const run = (work: () => Promise<void>) => {
    void (async () => {
      setBusy(true);
      setFailure(null);
      try {
        await work();
        setAsking(null);
        await onChanged();
      } catch (cause) {
        setFailure(
          cause instanceof Error ? problemDetail(cause) : String(cause),
        );
      } finally {
        setBusy(false);
      }
    })();
  };

  return (
    <li
      className={`rounded border border-slate-200 px-3 py-2 dark:border-slate-800 ${
        // Muted, never hidden: a silenced finding is still a finding, and it is
        // only on the page at all because somebody asked to see it.
        finding.acknowledged ? "opacity-70" : ""
      }`}
    >
      <div className="flex flex-wrap items-baseline gap-2">
        <Chip variant={priorityVariant(finding.priority)}>
          <span className="sr-only">{"Priority "}</span>
          <span className="tabular-nums">{finding.priority}</span>
        </Chip>
        <Chip mono>{finding.rule}</Chip>
        {/*
          Judgment is drawn in the accent so it stands out of a scan: it is the
          class that needs a person, and mechanical work is the quiet default.
        */}
        <Chip variant={finding.class === "judgment" ? "accent" : "neutral"}>
          {finding.class}
        </Chip>
        {finding.acknowledged && <Chip>acknowledged</Chip>}
        {anchored ? (
          <Link
            to={engramRoute(finding.domain, finding.permalink)}
            className="text-sm text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
          >
            {finding.title}
          </Link>
        ) : (
          <span className="text-sm font-medium break-words">
            {finding.title}
          </span>
        )}
        <span className="text-caption text-slate-500 dark:text-slate-400">
          {finding.domain}
          {finding.line !== null && ` line ${String(finding.line)}`}
        </span>
      </div>
      <p className="mt-1 text-sm">{finding.finding}</p>
      <p className="text-caption mt-0.5 font-mono break-words text-slate-500 dark:text-slate-400">
        {finding.evidence}
      </p>
      {finding.acknowledged && (
        <p className="text-caption mt-1 text-slate-500 dark:text-slate-400">
          {finding.ackNote === null
            ? "Ruled intentional, with no note given."
            : `Ruled intentional: ${finding.ackNote}`}
        </p>
      )}
      {finding.ackStale && (
        <p className="text-caption mt-1 text-amber-700 dark:text-amber-300">
          Acknowledged earlier, but the evidence changed.
          {finding.ackNote !== null && ` The note said: ${finding.ackNote}`}
        </p>
      )}
      <div className="mt-1 flex flex-wrap items-center gap-2">
        {instruction !== null && (
          <button
            type="button"
            aria-expanded={open}
            className={`${BUTTON.ghost} -ml-2`}
            onClick={() => {
              setOpen((was) => !was);
            }}
          >
            How to work this
          </button>
        )}
        {canWrite && anchored && asking === null && !finding.acknowledged && (
          <button
            type="button"
            className={BUTTON.ghost}
            onClick={() => {
              setFailure(null);
              // The note it was acknowledged with is the one to keep or to
              // correct, so a re-acknowledgment starts from it.
              setNote(finding.ackNote ?? "");
              setAsking("ack");
            }}
          >
            {ackLabel}
          </button>
        )}
        {canWrite && anchored && finding.acknowledged && (
          <button
            type="button"
            className={BUTTON.ghost}
            disabled={busy}
            onClick={() => {
              run(() =>
                unacknowledgeFinding(
                  finding.domain,
                  finding.permalink,
                  finding.rule,
                ),
              );
            }}
          >
            Unacknowledge
          </button>
        )}
        {canWrite && orphanAttachment && asking === null && (
          <button
            type="button"
            className={BUTTON.ghost}
            onClick={() => {
              setFailure(null);
              setAsking("delete");
            }}
          >
            Delete attachment
          </button>
        )}
      </div>
      {open && instruction !== null && (
        <p className="mt-1 rounded bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:bg-slate-900 dark:text-slate-300">
          {instruction}
        </p>
      )}
      {asking === "ack" && (
        <div className="mt-2 flex flex-wrap items-end gap-2">
          <span className="flex min-w-0 flex-col gap-1">
            <label
              htmlFor={noteId}
              className="text-caption text-slate-500 dark:text-slate-400"
            >
              Why is this intentional? (optional)
            </label>
            <input
              id={noteId}
              type="text"
              className={FIELD}
              value={note}
              onChange={(event) => {
                setNote(event.target.value);
              }}
            />
          </span>
          <button
            type="button"
            className={BUTTON.secondary}
            disabled={busy}
            onClick={() => {
              run(() =>
                acknowledgeFinding(
                  finding.domain,
                  finding.permalink,
                  finding.rule,
                  note,
                ),
              );
            }}
          >
            {ackLabel}
          </button>
          <button
            type="button"
            className={BUTTON.ghost}
            disabled={busy}
            onClick={() => {
              setAsking(null);
            }}
          >
            Cancel
          </button>
        </div>
      )}
      {asking === "delete" && (
        <div className="mt-2 flex flex-col gap-2">
          <span className="text-caption text-slate-600 dark:text-slate-300">
            {`Delete ${finding.title}? Nothing references it, and the bytes do not come back.`}
          </span>
          <span className="flex flex-wrap gap-2">
            <button
              type="button"
              className={BUTTON.destructive}
              disabled={busy}
              onClick={() => {
                run(() => deleteAttachment(finding.domain, finding.title));
              }}
            >
              Delete
            </button>
            <button
              type="button"
              className={BUTTON.ghost}
              disabled={busy}
              onClick={() => {
                setAsking(null);
              }}
            >
              Cancel
            </button>
          </span>
        </div>
      )}
      {failure !== null && (
        <p
          role="alert"
          className="text-caption mt-1 text-red-700 dark:text-red-300"
        >
          {failure}
        </p>
      )}
    </li>
  );
}

/**
 * The findings under their headings, families first in the catalog's own order
 * and anything from a newer catalog last. A family with nothing in it is left
 * out rather than drawn empty.
 */
function groupByFamily(findings: EvolveFinding[]): FindingGroup[] {
  const groups: FindingGroup[] = EVOLVE_FAMILIES.map((family) => ({
    key: family,
    title: EVOLVE_FAMILY_TITLES[family],
    blurb: EVOLVE_FAMILY_BLURBS[family],
    findings: findings.filter(
      (finding) => evolveFamily(finding.rule) === family,
    ),
  }));
  groups.push({
    key: "other",
    title: "Other",
    blurb: "Rules this app does not have a family for yet.",
    findings: findings.filter((finding) => evolveFamily(finding.rule) === null),
  });
  return groups.filter((group) => group.findings.length > 0);
}

/** A family's heading, for a name that may come from a newer catalog. */
function familyTitle(family: string): string {
  return family in EVOLVE_FAMILY_TITLES
    ? EVOLVE_FAMILY_TITLES[family as keyof typeof EVOLVE_FAMILY_TITLES]
    : family;
}

/**
 * How loud a priority is drawn.
 *
 * Two faces rather than a scale of five: the number itself is the ranking, and
 * a chip that graded it into bands would be inventing a verdict the engine did
 * not make. The one thing the color says is "this is near the top of what is
 * waiting", at the threshold the catalog's own high base priorities sit above.
 */
function priorityVariant(priority: number): ChipVariant {
  return priority >= 70 ? "caution" : "neutral";
}
