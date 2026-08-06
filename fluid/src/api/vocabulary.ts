/**
 * The words a domain is written in.
 *
 * Only the tags are read here, because they are the one filter axis the server
 * can enumerate completely: `type` and `status` are free form by design and no
 * endpoint lists the values in use, so a screen offers those as suggestions
 * rather than as a closed set.
 *
 * The domain is a filter on this route rather than a path segment, so an
 * unknown name answers an empty vocabulary rather than a 404.
 */

import { api, encodeSegment } from "./client";
import { asArray, asNumber, asObject, asString } from "./json";

/** One tag, with how many engrams carry it. */
export interface TagCount {
  /** The tag itself, lowercase with hyphens by convention. */
  name: string;
  /** How many engrams carry it. */
  engrams: number;
}

/** Read the tags out of a vocabulary payload, commonest first. */
export function readTags(payload: unknown): TagCount[] {
  const tags: TagCount[] = [];
  for (const entry of asArray(asObject(payload)?.tags)) {
    const record = asObject(entry);
    const name = asString(record?.name);
    if (name === null) {
      continue;
    }
    tags.push({ name, engrams: asNumber(record?.engrams) ?? 0 });
  }
  return tags.sort(
    (a, b) => b.engrams - a.engrams || a.name.localeCompare(b.name),
  );
}

/** The cache key of one domain's vocabulary. */
export function vocabularyKey(domain: string): readonly unknown[] {
  return ["vocabulary", domain];
}

/** Fetch the tags in use in one domain. */
export async function fetchTags(domain: string): Promise<TagCount[]> {
  return readTags(
    await api<unknown>(`/vocabulary?domain=${encodeSegment(domain)}`),
  );
}
