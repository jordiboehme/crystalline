/**
 * The consolidation queue, as `GET /evolve` answers it: what the registered
 * domains need next, ranked, with the evidence behind each finding and the
 * instruction that says how to work it.
 *
 * The sweep is a read and only a read. The `evolve` MCP tool records that a
 * sweep happened, because an agent calling it is about to do the work; this
 * endpoint runs the detection half instead, so a page left open never tells
 * the instance its backlog was attended to. The two acknowledgment verbs below
 * it are the exception that proves it: they write, and they write only when
 * somebody presses the button that calls them.
 *
 * Two shapes of the payload are worth naming here rather than at the screen.
 * A finding's `class` decides how it is drawn - `mechanical` completes intent
 * the knowledge already records, `judgment` changes what it claims - so a class
 * this client has never heard of is read as `judgment`: failing toward asking a
 * person is the only safe direction. And a queue row carries no family of its
 * own. The engine counts families over the whole filtered result and numbers
 * every rule by family in its catalog (`V0xx` temporal, `V1xx` structure,
 * `V2xx` redundancy), so the section a row belongs under is read off its rule
 * id, and a rule id from a newer catalog belongs to no section rather than to
 * the wrong one.
 */

import { api, encodeSegment } from "./client";
import { asArray, asNumber, asObject, asString } from "./json";
import type { AckBody } from "./model";

/** The detector families, in the catalog's own order. */
export const EVOLVE_FAMILIES = ["temporal", "structure", "redundancy"] as const;

/** One detector family. */
export type EvolveFamily = (typeof EVOLVE_FAMILIES)[number];

/** How a family is titled where a reader sees it. */
export const EVOLVE_FAMILY_TITLES: Record<EvolveFamily, string> = {
  temporal: "Temporal",
  structure: "Structure",
  redundancy: "Redundancy",
};

/** What a family section says it is about, in one line. */
export const EVOLVE_FAMILY_BLURBS: Record<EvolveFamily, string> = {
  temporal: "Validity windows, staleness and the supersede lifecycle.",
  structure: "References, reciprocity, orphans, stubs and size.",
  redundancy: "Duplicate content, colliding titles and tag drift.",
};

/**
 * What kind of work a finding is.
 *
 * `mechanical` completes intent the knowledge already records, so an agent may
 * just do it. `judgment` changes what the knowledge claims, so it is a question
 * for a person rather than a change to apply.
 */
export type EvolveClass = "mechanical" | "judgment";

/** What an unrecognized class reads as: the one that asks before acting. */
export const DEFAULT_EVOLVE_CLASS: EvolveClass = "judgment";

/** How many findings one sweep asks for. The engine clamps anything above it. */
export const EVOLVE_LIMIT = 100;

/**
 * How long a sweep stays fresh, in ms.
 *
 * Detection is the heaviest read this API has - it reads every engram of every
 * registered domain - and what it answers is a backlog, which moves at the
 * speed of somebody editing the knowledge rather than at the speed of a screen.
 * A minute is what the tree already uses for the same kind of value
 * (`TREE_STALE_TIME` in `api/domain.ts`), and it is what makes the core loop of
 * the maintenance screen cheap: following a finding to its engram and coming
 * back is a remount, and a remount inside the window reads the cache instead of
 * sweeping again.
 */
export const EVOLVE_STALE_MS = 60_000;

/** One finding, ranked across the whole result. */
export interface EvolveFinding {
  /** Its rank across the whole result, not within the page. */
  n: number;
  /** 0 to 100, after the salience and hub boosts. */
  priority: number;
  /** The rule that fired, for example `V006`. */
  rule: string;
  class: EvolveClass;
  domain: string;
  /**
   * The engram it fired on, or `""` for a finding that names no engram at all
   * - an orphaned attachment, a domain's drifted tag vocabulary. Empty is a
   * shape rather than a missing field: those findings are about the domain,
   * and an acknowledgment, which lives on an engram, has nowhere to hang.
   */
  permalink: string;
  /**
   * Its subject, in the words a reader knows it by: the engram's title, the
   * attachment's path for a finding about a file, and the domain's own name
   * for one that carries neither.
   */
  title: string;
  /** The line the rule fired on, or null when the finding is about the whole. */
  line: number | null;
  /** What was found, in a few words. */
  finding: string;
  /** What the detector read to find it. */
  evidence: string;
  /** What would settle it, in a few words. */
  fix: string;
  /**
   * An acknowledgment matched, and this row is here only because the sweep
   * was asked for the silenced ones.
   */
  acknowledged: boolean;
  /**
   * An acknowledgment for this rule exists on the engram but was given for
   * evidence that has since changed, so it no longer silences anything. The
   * finding is drawn as usual, saying so.
   */
  ackStale: boolean;
  /** The note the matching or stale acknowledgment carries, when it has one. */
  ackNote: string | null;
}

/** What acknowledgments kept out of the queue. */
export interface EvolveAcknowledged {
  /** Every silenced finding, over the whole result. */
  total: number;
  /** The same count per family, keyed as the engine names them. */
  byFamily: Record<string, number>;
}

/** One sweep, as this app reads it. */
export interface EvolveQueue {
  /** How many engrams the sweep read. */
  engramsScanned: number;
  /** How many findings the whole filtered result holds, page or no page. */
  total: number;
  /** Counts over the whole result rather than this page. */
  families: { family: string; findings: number }[];
  /** This page of the ranked queue. */
  queue: EvolveFinding[];
  /** The prescribed action, once per rule on this page. */
  actions: { rule: string; instruction: string }[];
  /** Any per-domain cap that fired, so a short queue is never mistaken for a
   * finished one. */
  truncations: string[];
  /**
   * What acknowledgments silenced, counted whether or not this sweep asked to
   * see it. A queue never shrinks quietly: what somebody ruled intentional is
   * still said out loud, as a number with a way to look at it.
   */
  acknowledged: EvolveAcknowledged;
}

/**
 * The family a rule belongs to, read off its id.
 *
 * Null for anything this client does not recognize, which includes a rule from
 * a catalog newer than it: a finding filed under the wrong heading would read
 * as a claim about what kind of work it is.
 */
export function evolveFamily(rule: string): EvolveFamily | null {
  const match = /^V(\d)\d\d$/.exec(rule);
  const index = Number(match?.[1] ?? NaN);
  return EVOLVE_FAMILIES[index] ?? null;
}

/** Read one class, falling back to the one that asks before acting. */
function readClass(value: unknown): EvolveClass {
  return value === "mechanical" ? "mechanical" : DEFAULT_EVOLVE_CLASS;
}

/**
 * Read one finding, or null when it names neither a rule nor a domain.
 *
 * Only those two are required. A permalink is NOT: several rules are about a
 * domain rather than about any one engram - an orphaned attachment, a drifted
 * tag vocabulary - and they arrive with an empty one by design. Requiring it
 * dropped exactly those rows while the engine went on counting them in
 * `total`, so the queue said one number and drew a smaller one.
 */
function readFinding(value: unknown): EvolveFinding | null {
  const record = asObject(value);
  const rule = asString(record?.rule);
  const domain = asString(record?.domain);
  if (rule === null || domain === null) {
    return null;
  }
  const permalink = asString(record?.permalink) ?? "";
  return {
    n: asNumber(record?.n) ?? 0,
    priority: asNumber(record?.priority) ?? 0,
    rule,
    class: readClass(record?.class),
    domain,
    permalink,
    // The attachment rules put the path in the title, so an anchorless finding
    // usually names its own subject. The one that does not - tag drift - is
    // about the domain, which is then the truest subject there is.
    title: asString(record?.title) ?? (permalink === "" ? domain : permalink),
    line: asNumber(record?.line),
    finding: asString(record?.finding) ?? "",
    evidence: asString(record?.evidence) ?? "",
    fix: asString(record?.fix) ?? "",
    // Both flags are omitted rather than sent false, so their absence is the
    // ordinary case and only a literal `true` means anything.
    acknowledged: record?.acknowledged === true,
    ackStale: record?.ack_stale === true,
    ackNote: asString(record?.ack_note),
  };
}

/** Read what acknowledgments silenced, whole and per family. */
function readAcknowledged(value: unknown): EvolveAcknowledged {
  const record = asObject(value);
  const byFamily: Record<string, number> = {};
  for (const [family, count] of Object.entries(
    asObject(record?.by_family) ?? {},
  )) {
    const findings = asNumber(count);
    if (findings !== null) {
      byFamily[family] = findings;
    }
  }
  return { total: asNumber(record?.total) ?? 0, byFamily };
}

/** Read one family count, or null when either half is missing. */
function readFamilyCount(
  value: unknown,
): { family: string; findings: number } | null {
  const record = asObject(value);
  const family = asString(record?.family);
  const findings = asNumber(record?.findings);
  return family === null || findings === null ? null : { family, findings };
}

/** Read one per-rule instruction, or null when either half is missing. */
function readAction(
  value: unknown,
): { rule: string; instruction: string } | null {
  const record = asObject(value);
  const rule = asString(record?.rule);
  const instruction = asString(record?.instruction);
  return rule === null || instruction === null ? null : { rule, instruction };
}

/** Read a sweep payload. */
export function readEvolveQueue(payload: unknown): EvolveQueue {
  const record = asObject(payload);
  return {
    engramsScanned: asNumber(record?.engrams_scanned) ?? 0,
    total: asNumber(record?.total) ?? 0,
    families: asArray(record?.families)
      .map(readFamilyCount)
      .filter(
        (count): count is { family: string; findings: number } =>
          count !== null,
      ),
    queue: asArray(record?.queue)
      .map(readFinding)
      .filter((finding): finding is EvolveFinding => finding !== null),
    actions: asArray(record?.actions)
      .map(readAction)
      .filter(
        (action): action is { rule: string; instruction: string } =>
          action !== null,
      ),
    truncations: asArray(record?.truncations).filter(
      (entry): entry is string => typeof entry === "string",
    ),
    acknowledged: readAcknowledged(record?.acknowledged),
  };
}

/**
 * Every cached sweep, whatever it was asked for: the prefix a write
 * invalidates, so acknowledging something refreshes the sweep that hides the
 * silenced rows AND the one that shows them.
 */
export const EVOLVE_KEY_ROOT = ["evolve"] as const;

/**
 * The cache key of one sweep, which is every parameter it carries.
 *
 * Whether the silenced findings were asked for is one of them: it is a
 * different question with a different answer, and reading one back for the
 * other would draw a queue that does not match the toggle above it.
 */
export function evolveKey(
  domains: string[] = [],
  includeAcknowledged = false,
): readonly unknown[] {
  return [...EVOLVE_KEY_ROOT, domains, includeAcknowledged];
}

/**
 * Fetch one sweep.
 *
 * A full page by default rather than the engine's own ten: this screen draws
 * the queue in one go, grouped by family, and paging a report somebody opened
 * to see the shape of the backlog would hide exactly the thing they came for.
 * `total` says how much a fired cap left out.
 */
export async function fetchEvolveQueue(
  opts: {
    domains?: string[];
    limit?: number;
    /** Ask for the silenced findings too, each marked acknowledged. */
    includeAcknowledged?: boolean;
  } = {},
): Promise<EvolveQueue> {
  const query = new URLSearchParams();
  const domains = opts.domains ?? [];
  if (domains.length > 0) {
    query.set("domains", domains.join(","));
  }
  query.set("limit", String(opts.limit ?? EVOLVE_LIMIT));
  // Sent only when it is asked for, so an ordinary sweep goes out as the
  // ordinary sweep it always was.
  if (opts.includeAcknowledged === true) {
    query.set("include_acknowledged", "true");
  }
  return readEvolveQueue(await api<unknown>(`/evolve?${query.toString()}`));
}

/** Where both acknowledgment verbs live, for one domain. */
function ackPath(domain: string): string {
  return `/domains/${encodeSegment(domain)}/evolve/ack`;
}

/**
 * The body both verbs take.
 *
 * A note that is only whitespace is left out rather than sent: it would be
 * stored in the engram's frontmatter, where a blank string is a line of noise
 * that reads as a reason somebody gave and did not.
 */
function ackBody(permalink: string, rule: string, note?: string): AckBody {
  const said = note?.trim() ?? "";
  return said === "" ? { permalink, rule } : { permalink, rule, note: said };
}

/**
 * Rule one finding intentional, so future sweeps stop raising it.
 *
 * The scope it holds for is never sent: the server runs detection for the
 * engram and takes the firing finding's own evidence, so neither a person nor
 * an agent ever handles a fingerprint. Acknowledging the same rule again
 * replaces the entry, which is what makes this the re-acknowledge call too.
 */
export async function acknowledgeFinding(
  domain: string,
  permalink: string,
  rule: string,
  note?: string,
): Promise<void> {
  await api(ackPath(domain), {
    method: "POST",
    body: JSON.stringify(ackBody(permalink, rule, note)),
  });
}

/** Take an acknowledgment back, leaving the engram's others alone. */
export async function unacknowledgeFinding(
  domain: string,
  permalink: string,
  rule: string,
): Promise<void> {
  await api(ackPath(domain), {
    method: "DELETE",
    body: JSON.stringify(ackBody(permalink, rule)),
  });
}
