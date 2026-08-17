/**
 * The consolidation queue, as `GET /evolve` answers it: what the registered
 * domains need next, ranked, with the evidence behind each finding and the
 * instruction that says how to work it.
 *
 * A read and only a read. The `evolve` MCP tool records that a sweep happened,
 * because an agent calling it is about to do the work; this endpoint runs the
 * detection half instead, so a page left open never tells the instance its
 * backlog was attended to.
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

import { api } from "./client";
import { asArray, asNumber, asObject, asString } from "./json";

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
  permalink: string;
  /** Its title, falling back to the permalink when it has none. */
  title: string;
  /** The line the rule fired on, or null when the finding is about the whole. */
  line: number | null;
  /** What was found, in a few words. */
  finding: string;
  /** What the detector read to find it. */
  evidence: string;
  /** What would settle it, in a few words. */
  fix: string;
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

/** Read one finding, or null when it carries no address to link to. */
function readFinding(value: unknown): EvolveFinding | null {
  const record = asObject(value);
  const rule = asString(record?.rule);
  const domain = asString(record?.domain);
  const permalink = asString(record?.permalink);
  if (rule === null || domain === null || permalink === null) {
    return null;
  }
  return {
    n: asNumber(record?.n) ?? 0,
    priority: asNumber(record?.priority) ?? 0,
    rule,
    class: readClass(record?.class),
    domain,
    permalink,
    title: asString(record?.title) ?? permalink,
    line: asNumber(record?.line),
    finding: asString(record?.finding) ?? "",
    evidence: asString(record?.evidence) ?? "",
    fix: asString(record?.fix) ?? "",
  };
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
  };
}

/** The cache key of one sweep, which is every parameter it carries. */
export function evolveKey(domains: string[] = []): readonly unknown[] {
  return ["evolve", domains];
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
  opts: { domains?: string[]; limit?: number } = {},
): Promise<EvolveQueue> {
  const query = new URLSearchParams();
  const domains = opts.domains ?? [];
  if (domains.length > 0) {
    query.set("domains", domains.join(","));
  }
  query.set("limit", String(opts.limit ?? EVOLVE_LIMIT));
  return readEvolveQueue(await api<unknown>(`/evolve?${query.toString()}`));
}
