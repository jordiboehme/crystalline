/**
 * The two things a domain screen reads besides its engrams: the MANIFEST it is
 * introduced by, and the tree it is navigated through.
 *
 * The split between the tree and the engram listing is the server's own: the
 * tree owns the view by folder and the listing owns the view by frontmatter,
 * so there is no folder filter on one and no status on the other. A screen that
 * wants both says which one it is showing rather than blending them.
 */

import { api, encodeSegment } from "./client";
import type { EngramRow } from "./engrams";
import { readEngramRow } from "./engrams";
import { asArray, asObject, asString, asStrings } from "./json";

/** One folder of a domain: its subfolders, and the engrams sitting in it. */
export interface DomainTree {
  /** The domain this is a view of. */
  domain: string;
  /** The folder path, domain relative. The root is the empty string. */
  path: string;
  /** The subfolder names directly below `path`. */
  folders: string[];
  /** The engrams directly in this folder. */
  engrams: EngramRow[];
}

/** Read a browse payload. */
export function readTree(
  payload: unknown,
  domain: string,
  path: string,
): DomainTree {
  const record = asObject(payload);
  return {
    domain: asString(record?.domain) ?? domain,
    path,
    folders: asStrings(record?.folders),
    engrams: asArray(record?.engrams)
      .map((entry) => readEngramRow(entry, domain))
      .filter((row): row is EngramRow => row !== null),
  };
}

/** The cache key of one folder of one domain. */
export function treeKey(domain: string, path: string): readonly unknown[] {
  return ["domain-tree", domain, path];
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
