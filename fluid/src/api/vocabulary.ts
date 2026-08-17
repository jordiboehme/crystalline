/**
 * The words a domain is written in.
 *
 * Only the tags are read here, because they are the one filter axis the server
 * can enumerate completely: `type` and `status` are free form by design and no
 * endpoint lists the values in use, so a screen offers those as suggestions
 * rather than as a closed set.
 *
 * The domain is a filter on this route rather than a path segment, so an
 * unknown name answers an empty vocabulary rather than a 404, and leaving it
 * off asks what every domain on the instance is written in - which is what a
 * search across all of them needs.
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

/** The cache key of one domain's vocabulary, or of the whole instance's. */
export function vocabularyKey(domain: string | null): readonly unknown[] {
  return ["vocabulary", domain];
}

/**
 * The cache key of one domain's whole vocabulary payload - tags, categories
 * and relation types together.
 *
 * A key of its own rather than `vocabularyKey`: `DomainHome` already caches
 * `fetchTags` under that key, and the two payloads are different shapes at
 * the same key would mean whichever query landed second overwrote the first
 * with data the other reader cannot parse.
 */
export function fullVocabularyKey(domain: string | null): readonly unknown[] {
  return ["vocabulary-full", domain];
}

/** Fetch the tags in use in one domain, or in every domain for `null`. */
export async function fetchTags(domain: string | null): Promise<TagCount[]> {
  const path =
    domain === null
      ? "/vocabulary"
      : `/vocabulary?domain=${encodeSegment(domain)}`;
  return readTags(await api<unknown>(path));
}

/** One name with how many engrams use it: a category or a relation type. */
export interface NamedCount {
  name: string;
  count: number;
}

/** Everything the vocabulary route enumerates. */
export interface Vocabulary {
  tags: TagCount[];
  categories: NamedCount[];
  relationTypes: NamedCount[];
}

/** Read one named-count list off the payload, commonest first. */
function readNamedCounts(value: unknown): NamedCount[] {
  const counts: NamedCount[] = [];
  for (const entry of asArray(value)) {
    const record = asObject(entry);
    const name = asString(record?.name);
    if (name === null) {
      continue;
    }
    counts.push({
      name,
      count: asNumber(record?.count) ?? asNumber(record?.engrams) ?? 0,
    });
  }
  return counts.sort(
    (a, b) => b.count - a.count || a.name.localeCompare(b.name),
  );
}

/** Read the whole vocabulary payload. */
export function readVocabulary(payload: unknown): Vocabulary {
  const record = asObject(payload);
  return {
    tags: readTags(payload),
    categories: readNamedCounts(record?.categories),
    relationTypes: readNamedCounts(record?.relation_types),
  };
}

/** Fetch the whole vocabulary of one domain, or of every domain for null. */
export async function fetchVocabulary(
  domain: string | null,
): Promise<Vocabulary> {
  const path =
    domain === null
      ? "/vocabulary"
      : `/vocabulary?domain=${encodeSegment(domain)}`;
  return readVocabulary(await api<unknown>(path));
}
