import type { QueryClient } from "@tanstack/react-query";

import type { AttachmentRow } from "./files";

/** How much of a file's sha256 its URL carries: enough to tell versions apart. */
const VERSION_CHARS = 12;

/**
 * The address of a file's current bytes: the plain URL, plus `?v=<first 12 of
 * its sha256>` once the file is known to have changed. An address for the
 * browser only; no document is ever written with it.
 */
export function versionedUrl(plain: string, sha: string | undefined): string {
  return sha === undefined || sha === ""
    ? plain
    : `${plain}?v=${encodeURIComponent(sha.slice(0, VERSION_CHARS))}`;
}

/**
 * The first sha256 this document saw for each file, by `<scope>\0<path>`.
 * One per QueryClient: one per open document in the app, and a fresh one per
 * test.
 */
const baselines = new WeakMap<QueryClient, Map<string, string>>();

/**
 * The map `changedVersions` last handed out for each scope, so an equal
 * answer comes back as the same map. One per QueryClient, like the baselines.
 */
const lastHanded = new WeakMap<
  QueryClient,
  Map<string, ReadonlyMap<string, string>>
>();

/** What `changedVersions` hands out whenever no file changed: always this one. */
const NONE: ReadonlyMap<string, string> = Object.freeze(
  new Map<string, string>(),
);

function sameEntries(
  a: ReadonlyMap<string, string>,
  b: ReadonlyMap<string, string>,
): boolean {
  if (a.size !== b.size) {
    return false;
  }
  for (const [path, sha] of a) {
    if (b.get(path) !== sha) {
      return false;
    }
  }
  return true;
}

/**
 * Path to sha256 for every file whose bytes changed since this document
 * first listed it, for the asset homes of Reflections and of engrams. A file
 * seen for the first time sets its baseline and is left out, so its address
 * stays plain: the image drawn before the listing landed is not fetched a
 * second time under another URL. The pages and the editors share the
 * baseline through the QueryClient, so a file replaced on one page is drawn
 * anew on the next.
 *
 * `key` names the file store: a Reflection passes its domain, the engram
 * files of a domain pass `engram:<domain>`, so the two never share a
 * baseline for a path they both hold.
 *
 * The answer keeps its identity while its entries stay the same: one shared
 * empty map when nothing changed, and the map handed out last for `key`
 * when it holds the same paths and shas. A reading body builds its component
 * map from it, and a new map would remount every image, close an open full
 * window and redraw every diagram each time the listing lands or refetches.
 *
 * The rare race: the plain image loads, then somebody replaces the file
 * before this document's first listing arrives. The baseline is then the
 * new sha, and the old picture stays until a reload.
 */
export function changedVersions(
  queryClient: QueryClient,
  key: string,
  rows: readonly AttachmentRow[] | undefined,
): ReadonlyMap<string, string> {
  let seen = baselines.get(queryClient);
  if (seen === undefined) {
    seen = new Map();
    baselines.set(queryClient, seen);
  }
  const changed = new Map<string, string>();
  for (const row of rows ?? []) {
    const id = `${key}\u0000${row.path}`;
    const first = seen.get(id);
    if (first === undefined) {
      seen.set(id, row.sha256);
    } else if (first !== row.sha256) {
      changed.set(row.path, row.sha256);
    }
  }
  if (changed.size === 0) {
    return NONE;
  }
  let handed = lastHanded.get(queryClient);
  if (handed === undefined) {
    handed = new Map();
    lastHanded.set(queryClient, handed);
  }
  const previous = handed.get(key);
  if (previous !== undefined && sameEntries(previous, changed)) {
    return previous;
  }
  handed.set(key, changed);
  return changed;
}
