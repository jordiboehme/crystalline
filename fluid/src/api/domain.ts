/**
 * The two things a domain screen reads besides its engrams: the MANIFEST it is
 * introduced by, and the tree it is navigated through.
 *
 * The split between the tree and the engram listing is the server's own, and
 * it is a split by shape rather than by subject: the tree answers one level -
 * the subfolders of a folder and the engrams directly in it, capped - while
 * the listing pages a folder or a filter without bound. So navigation reads
 * the tree and any list of engrams reads the listing, whichever of the two it
 * is a list of.
 */

import { api, encodeSegment } from "./client";
import type { EngramRow } from "./engrams";
import { readEngramRow } from "./engrams";
import { asArray, asNumber, asObject, asString, asStrings } from "./json";

/** One folder of a domain: its subfolders, and the engrams sitting in it. */
export interface DomainTree {
  /** The domain this is a view of. */
  domain: string;
  /** The folder path, domain relative. The root is the empty string. */
  path: string;
  /**
   * The subfolder names directly below `path`. Never cut: the server derives
   * them from the paths themselves rather than from the rows that survived its
   * cap, so a level too big to draw still names every folder under it.
   */
  folders: string[];
  /** The engrams directly in this folder, up to the server's per-level cap. */
  engrams: EngramRow[];
  /**
   * Whether the level holds more engrams than `engrams` carries.
   *
   * The endpoint caps a level rather than answering with a folder of tens of
   * thousands, so a reader of this payload has to know the difference between
   * a folder and the first page of one.
   */
  truncated: boolean;
  /** How many engrams the level holds, cap or no cap. */
  total: number;
}

/**
 * Read a browse payload.
 *
 * `truncated` and `total` are additive fields: a daemon that predates them
 * answered with everything the level held, which is exactly what an uncut
 * level of `engrams.length` rows means. That fallback is the one
 * `readEngramPage` uses for the same field, for the same reason.
 */
export function readTree(
  payload: unknown,
  domain: string,
  path: string,
): DomainTree {
  const record = asObject(payload);
  const engrams = asArray(record?.engrams)
    .map((entry) => readEngramRow(entry, domain))
    .filter((row): row is EngramRow => row !== null);
  return {
    domain: asString(record?.domain) ?? domain,
    path,
    folders: asStrings(record?.folders),
    engrams,
    truncated: record?.truncated === true,
    total: asNumber(record?.total) ?? engrams.length,
  };
}

/** The cache key of every folder of one domain, which is what a write moves. */
export function domainTreeKey(domain: string): readonly unknown[] {
  return ["domain-tree", domain];
}

/** The cache key of one folder of one domain. */
export function treeKey(domain: string, path: string): readonly unknown[] {
  return [...domainTreeKey(domain), path];
}

/**
 * How long a folder of the tree stays fresh.
 *
 * A minute, because a tree is the shape of a domain rather than its contents:
 * it moves when somebody creates, moves or retires an engram, and those three
 * invalidate it by hand (see the dialog bodies). Without this, every window
 * focus refetched every open level of every tree on screen - the sidebar's and
 * the folder picker's at once - which is a burst of requests for an answer
 * that almost never changed between two glances at the same window.
 */
export const TREE_STALE_TIME = 60_000;

/**
 * One folder of a domain, as a query: the key, the fetch and the freshness in
 * one place, so the three screens that walk this tree cannot drift apart on
 * any of the three.
 */
export function treeQuery(domain: string, path: string) {
  return {
    queryKey: treeKey(domain, path),
    queryFn: () => fetchTree(domain, path),
    staleTime: TREE_STALE_TIME,
  };
}

/** Fetch one folder of a domain. The root is the empty path. */
export async function fetchTree(
  domain: string,
  path: string,
): Promise<DomainTree> {
  const query = new URLSearchParams();
  if (path !== "") {
    query.set("path", path);
  }
  const suffix = query.size > 0 ? `?${query.toString()}` : "";
  const payload = await api<unknown>(
    `/domains/${encodeSegment(domain)}/tree${suffix}`,
  );
  return readTree(payload, domain, path);
}

/** The cache key of one domain's MANIFEST. */
export function manifestKey(domain: string): readonly unknown[] {
  return ["domain-manifest", domain];
}

/** A bullet the core crate flagged, kept verbatim beside why. */
export interface ManifestProblem {
  /** The category, in snake case: `malformed`, `unknown_type`, ... */
  kind: string;
  /** The bullet as written, without its dash. */
  bullet: string;
  /** Why it was flagged, in the crate's words. */
  reason: string;
}

/**
 * The MANIFEST's features as the server reads them out of the source.
 *
 * Nothing here is interpreted a second time: the server parsed it, the
 * panels show it, and a change goes through the editor. `null` for the two
 * optional sections means the MANIFEST has no such section.
 */
export interface ManifestSections {
  scope: string[];
  whenToUse: string[];
  /** Which of the two an agent reads, or `none` when both are empty. */
  routing: "when_to_use" | "scope" | "none";
  /** The required sections the MANIFEST lacks: `Scope`, `When to Use`. */
  missing: string[];
  provisioning: {
    decls: { kind: string; path: string }[];
    problems: ManifestProblem[];
  } | null;
  tagAliases: {
    decls: { alias: string; canonical: string }[];
    problems: ManifestProblem[];
  } | null;
  /**
   * Every MANIFEST configuration key the server's registry knows, with what
   * this MANIFEST declares beside it. Empty for a MANIFEST that did not
   * parse: a document nobody can read declares nothing.
   */
  policies: PolicyView[];
}

/** One MANIFEST policy key, as the registry describes it beside what this MANIFEST says. */
export interface PolicyView {
  key: string;
  /** As the frontmatter writes it, or null when the key is absent. */
  declared: string | null;
  /** The value that holds: absent and unrecognized both fall to `default`. */
  effective: string;
  values: string[];
  default: string;
  meaning: string;
  /** Who may change it: the domain's owner (or an admin), or an admin alone. */
  changedBy: "owner" | "admin";
}

/** A MANIFEST as the domain page reads it. */
export interface ManifestView {
  /** The markdown as written; empty when the MANIFEST carries nothing. */
  markdown: string;
  /**
   * The features the server read out of it, or null when the server sent
   * none: an older daemon answers the markdown alone, and the page draws
   * the document without the panels rather than panels that say nothing.
   */
  sections: ManifestSections | null;
}

function readProblems(value: unknown): ManifestProblem[] {
  return asArray(value).flatMap((entry) => {
    const record = asObject(entry);
    const kind = asString(record?.kind);
    const bullet = asString(record?.bullet);
    const reason = asString(record?.reason);
    return kind !== null && bullet !== null && reason !== null
      ? [{ kind, bullet, reason }]
      : [];
  });
}

/**
 * The registry rows, dropped one by one where a row says nothing.
 *
 * A row is its key and what holds; without either there is no line to draw,
 * so it goes rather than being drawn as a blank. The three describing fields
 * have quiet defaults instead: a row that carries no `default` is read as
 * declaring that what holds IS the default, which is true of every key
 * nobody wrote in the frontmatter.
 */
function readPolicies(value: unknown): PolicyView[] {
  return asArray(value).flatMap((entry) => {
    const record = asObject(entry);
    const key = asString(record?.key);
    const effective = asString(record?.effective);
    if (key === null || effective === null) {
      return [];
    }
    return [
      {
        key,
        declared: asString(record?.declared),
        effective,
        values: asStrings(record?.values),
        default: asString(record?.default) ?? effective,
        meaning: asString(record?.meaning) ?? "",
        changedBy: asString(record?.changed_by) === "admin" ? "admin" : "owner",
      },
    ];
  });
}

/** Read the `sections` member of a manifest payload, or null when it is not there. */
export function readManifestSections(value: unknown): ManifestSections | null {
  const record = asObject(value);
  if (record === null) {
    return null;
  }
  const routing = record.routing;
  const provisioning = asObject(record.provisioning);
  const aliases = asObject(record.tag_aliases);
  return {
    scope: asStrings(record.scope),
    whenToUse: asStrings(record.when_to_use),
    routing:
      routing === "when_to_use" || routing === "scope" ? routing : "none",
    missing: asStrings(record.missing),
    provisioning:
      provisioning === null
        ? null
        : {
            decls: asArray(provisioning.decls).flatMap((entry) => {
              const decl = asObject(entry);
              const kind = asString(decl?.kind);
              const path = asString(decl?.path);
              return kind !== null && path !== null ? [{ kind, path }] : [];
            }),
            problems: readProblems(provisioning.problems),
          },
    tagAliases:
      aliases === null
        ? null
        : {
            decls: asArray(aliases.decls).flatMap((entry) => {
              const decl = asObject(entry);
              const alias = asString(decl?.alias);
              const canonical = asString(decl?.canonical);
              return alias !== null && canonical !== null
                ? [{ alias, canonical }]
                : [];
            }),
            problems: readProblems(aliases.problems),
          },
    policies: readPolicies(record.policies),
  };
}

/**
 * Fetch a domain's MANIFEST: the markdown, and the features the server read
 * out of it.
 *
 * Answers an empty markdown for a domain whose MANIFEST carries nothing,
 * which a caller shows the same way it shows a missing one: there is nothing
 * to read either way.
 */
export async function fetchManifest(domain: string): Promise<ManifestView> {
  const payload = await api<unknown>(
    `/domains/${encodeSegment(domain)}/manifest`,
  );
  const record = asObject(payload);
  return {
    markdown: asString(record?.markdown) ?? "",
    sections: readManifestSections(record?.sections),
  };
}

/** A manifest with the version token an edit of it needs. */
export interface ManifestDetail {
  markdown: string;
  /** sha256 of the markdown, the manifest save's If-Match token. */
  checksum: string | null;
}

/**
 * The cache key of one domain's MANIFEST detail read - a different shape
 * from `manifestKey`, and its own key rather than a reuse of it: the plain
 * `fetchManifest` DomainHome reads answers with the markdown plus the
 * sections the server read out of it, and a detail read landing under the
 * same key would overwrite it with a shape the plain reader cannot parse the
 * checksum out of.
 */
export function manifestDetailKey(domain: string): readonly unknown[] {
  return ["domain-manifest-detail", domain];
}

/** Fetch a domain's MANIFEST with its checksum, for editing. */
export async function fetchManifestDetail(
  domain: string,
): Promise<ManifestDetail> {
  const payload = await api<unknown>(
    `/domains/${encodeSegment(domain)}/manifest`,
  );
  const record = asObject(payload);
  return {
    markdown: asString(record?.markdown) ?? "",
    checksum: asString(record?.checksum),
  };
}

/** Save a MANIFEST verbatim, guarded by the checksum it is based on. */
export async function saveManifest(
  domain: string,
  markdown: string,
  checksum: string,
): Promise<ManifestDetail> {
  const payload = await api<unknown>(
    `/domains/${encodeSegment(domain)}/manifest`,
    {
      method: "PUT",
      headers: { "If-Match": `"${checksum}"` },
      body: JSON.stringify({ markdown }),
    },
  );
  const record = asObject(payload);
  return {
    markdown: asString(record?.markdown) ?? markdown,
    checksum: asString(record?.checksum),
  };
}

/** What a policy write answers: the manifest as it now reads, and whether that is the caller's draft. */
export interface PolicyWrite {
  markdown: string;
  checksum: string | null;
  sections: ManifestSections | null;
  /** True in a domain that reviews changes: the write is the caller's draft of the MANIFEST. */
  draft: boolean;
}

/**
 * Set one or more MANIFEST policy keys.
 *
 * No `If-Match`: the server rewrites one keyed line under its own
 * compare-and-write, and a whole-document save from a stale editor afterwards
 * still gets the PUT's 412.
 */
export async function setDomainPolicies(
  domain: string,
  changes: Record<string, string>,
): Promise<PolicyWrite> {
  const payload = await api<unknown>(
    `/domains/${encodeSegment(domain)}/manifest`,
    {
      method: "PATCH",
      body: JSON.stringify(changes),
    },
  );
  const record = asObject(payload);
  return {
    markdown: asString(record?.markdown) ?? "",
    checksum: asString(record?.checksum),
    sections: readManifestSections(record?.sections),
    // Said only when the server said it: an ordinary domain's answer carries
    // no such key, and the write landed in the domain itself.
    draft: record?.draft === true,
  };
}
