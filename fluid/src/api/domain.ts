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

/**
 * Fetch a domain's MANIFEST markdown.
 *
 * Answers the empty string for a domain whose MANIFEST carries nothing, which
 * a caller shows the same way it shows a missing one: there is nothing to read
 * either way.
 */
export async function fetchManifest(domain: string): Promise<string> {
  const payload = await api<unknown>(
    `/domains/${encodeSegment(domain)}/manifest`,
  );
  return asString(asObject(payload)?.markdown) ?? "";
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
 * `fetchManifest` DomainHome reads answers with just the markdown, and a
 * detail read landing under the same key would overwrite it with a shape the
 * plain reader cannot parse the checksum out of.
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
