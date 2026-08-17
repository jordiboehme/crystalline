/**
 * Maintenance: what the knowledge needs next.
 *
 * A report, not a workbench. The sweep behind it reads every registered domain
 * and changes nothing, and neither does this screen: there is no button here
 * that edits an engram, because the work a finding names is work somebody does
 * in the engram itself, having read it. So every finding links to the engram it
 * fired on, and that link is the whole of what this screen asks of a reader.
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

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Link } from "react-router";

import { problemDetail } from "../api/client";
import type { EvolveFinding, EvolveQueue } from "../api/evolve";
import {
  EVOLVE_FAMILIES,
  EVOLVE_FAMILY_BLURBS,
  EVOLVE_FAMILY_TITLES,
  evolveFamily,
  evolveKey,
  fetchEvolveQueue,
} from "../api/evolve";
import { Skeleton } from "../components/Skeleton";
import { BUTTON, Chip, FIELD } from "../components/primitives";
import type { ChipVariant } from "../components/primitives";
import { plural } from "../format";
import { engramRoute } from "../paths";

/** Every domain, as the filter writes it. */
const EVERY_DOMAIN = "";

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
  const sweep = useQuery({
    queryKey: evolveKey(),
    queryFn: () => fetchEvolveQueue(),
  });

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
            nothing: each finding names an engram to go and read, and the work
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
        />
      ))}

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
 * over the page, so they say the shape of the backlog even when a cap kept some
 * of it off screen. They are dropped once a domain filter narrows the view,
 * because they would then be counting something other than what is drawn.
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
  const breakdown = queue.families
    .map((count) => `${familyTitle(count.family)} ${String(count.findings)}`)
    .join(", ");
  const counted =
    scoped === EVERY_DOMAIN
      ? shown < queue.total
        ? `${String(shown)} of ${plural(queue.total, "finding", "findings")}`
        : plural(queue.total, "finding", "findings")
      : `${plural(shown, "finding", "findings")} in ${scoped}`;
  return (
    <p className="text-caption text-slate-500 tabular-nums dark:text-slate-400">
      {counted} over {plural(queue.engramsScanned, "engram", "engrams")}
      {scoped === EVERY_DOMAIN && breakdown !== "" ? `. ${breakdown}.` : "."}
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

/** One family, its findings under it. */
function FamilySection({
  group,
  instructions,
}: {
  group: FindingGroup;
  instructions: Map<string, string>;
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
          />
        ))}
      </ul>
    </section>
  );
}

/**
 * One finding: what fired, where, what was read to find it, and - on request -
 * the catalog's own instruction for the rule.
 *
 * The instruction is folded away rather than printed on every row, because it
 * is a paragraph written for whoever is about to do the work and the row above
 * it is what somebody scanning the queue reads. Each row holds its own open
 * state: the instruction belongs to the rule, but the act of asking for it
 * belongs to the finding in front of the reader.
 */
function FindingRow({
  finding,
  instruction,
}: {
  finding: EvolveFinding;
  instruction: string | null;
}) {
  const [open, setOpen] = useState(false);
  return (
    <li className="rounded border border-slate-200 px-3 py-2 dark:border-slate-800">
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
        <Link
          to={engramRoute(finding.domain, finding.permalink)}
          className="text-sm text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
        >
          {finding.title}
        </Link>
        <span className="text-caption text-slate-500 dark:text-slate-400">
          {finding.domain}
          {finding.line !== null && ` line ${String(finding.line)}`}
        </span>
      </div>
      <p className="mt-1 text-sm">{finding.finding}</p>
      <p className="text-caption mt-0.5 font-mono break-words text-slate-500 dark:text-slate-400">
        {finding.evidence}
      </p>
      {instruction !== null && (
        <>
          <button
            type="button"
            aria-expanded={open}
            className={`${BUTTON.ghost} mt-1 -ml-2`}
            onClick={() => {
              setOpen((was) => !was);
            }}
          >
            How to work this
          </button>
          {open && (
            <p className="mt-1 rounded bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:bg-slate-900 dark:text-slate-300">
              {instruction}
            </p>
          )}
        </>
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
