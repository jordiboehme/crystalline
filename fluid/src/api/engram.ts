/**
 * One engram in full, as `GET /domains/{d}/engrams/{permalink}` answers it.
 *
 * The endpoint hands the engine's own read payload over unchanged, so this is
 * where that payload becomes something the screens can hold. Two of its habits
 * are worth knowing before reading the reader.
 *
 * The frontmatter is the parsed struct rather than the YAML as written: the
 * `type` key is `engram_type` there, and every key the model does not name is
 * kept verbatim under `extra`, which is where `salience` lives. Both are read
 * from either place, so a future engine that promotes one of them does not
 * quietly blank a field here.
 *
 * The reference lists say whether the index resolved each target, and never
 * where it resolved to: `{ domain, target }` is the text inside the brackets,
 * a title as often as a permalink. Turning that into a link takes the
 * neighborhood graph as well, which is why `wikilinks.ts` reads both.
 *
 * `checksum` is the version of the engram this app is holding: the same token
 * the response's `ETag` carries, and the one a later conditional write presents
 * as `expected_checksum`. It is kept on the cached detail for that reason.
 */

import { api, engramPath } from "./client";
import { asArray, asNumber, asObject, asString, asStrings } from "./json";

/** Where a `[[Target]]` points, as the server parsed the brackets. */
export interface LinkTarget {
  /** The domain a `[[domain:Target]]` prefix named, or null for a bare one. */
  domain: string | null;
  /** The target text: a title or a permalink, as it was written. */
  target: string;
}

/** One parsed reference out of the body: a prose wikilink or a relation. */
export interface EngramReference {
  /** The one-based line it sits on, or null when the payload did not say. */
  line: number | null;
  /** The relation type, or null for a prose wikilink, which declares none. */
  relType: string | null;
  /** What it points at. */
  target: LinkTarget;
  /** Whether the index found something at the other end. */
  resolved: boolean;
}

/** One observation bullet. */
export interface EngramObservation {
  /** The one-based line it sits on, or null when the payload did not say. */
  line: number | null;
  /** The bracket token it opens with, free form. */
  category: string | null;
  /** The text, with its trailing tags and context taken out. */
  content: string;
  /** Its trailing hashtags, without the `#`. */
  tags: string[];
  /** Its trailing parenthesized group, when it has one. */
  context: string | null;
}

/** One entry in the verification trail. */
export interface VerifiedEntry {
  /** The actor that checked the knowledge, absent on a legacy date. */
  by: string | null;
  /** When it was checked. */
  at: string | null;
}

/** The frontmatter fields the engram page presents. */
export interface EngramFrontmatter {
  /** The `type`, free form. */
  type: string | null;
  /** The `status`, free form. */
  status: string | null;
  /** Its tags, in the order the source gave them. */
  tags: string[];
  /** Its `salience`, when it carries one. */
  salience: number | null;
  /** `valid_from`. Absent means it has always been valid. */
  validFrom: string | null;
  /** `valid_to`. Absent means it is valid forever. */
  validTo: string | null;
  /** `stale_after`, or the legacy `review_after` when only that is written. */
  staleAfter: string | null;
  /** The verification trail, oldest first. Empty when nothing verified it. */
  verified: VerifiedEntry[];
}

/** One of the capped inbound references the detail payload samples. */
export interface InboundRef {
  /** The domain the reference comes from. */
  domain: string | null;
  /** The path of the file it comes from. */
  path: string | null;
  /** `relation` or `link`. */
  kind: string | null;
}

/** One engram, as the page draws it. */
export interface EngramDetail {
  domain: string;
  permalink: string;
  /** Its title, falling back to the permalink when it has none. */
  title: string;
  /** Its `crystalline://` address. */
  url: string;
  /** The file it lives in, for a file domain. */
  path: string | null;
  /** The markdown as written, frontmatter and all. */
  content: string;
  /** The version of it this app is holding. Equal to the response's `ETag`. */
  checksum: string | null;
  frontmatter: EngramFrontmatter;
  observations: EngramObservation[];
  /** Its `- rel_type [[Target]]` bullets. */
  relations: EngramReference[];
  /** The wikilinks in its prose. */
  links: EngramReference[];
  /** How many references point at it, across the whole index. */
  inboundCount: number;
  /** The capped sample of them the payload carries. */
  inboundRefs: InboundRef[];
}

/** The `crystalline://` address of one engram, which is what it is called. */
export function crystallineAddress(domain: string, permalink: string): string {
  return `crystalline://${domain}/${permalink}`;
}

/**
 * Read an address back apart, or null when it is not one.
 *
 * The other direction of {@link crystallineAddress}, and here beside it so the
 * one form an engram is named in is written down once. A screen that takes an
 * address from its URL needs the two halves back: the permalink is a path, so
 * the split is at the first slash and every slash after it stays in the
 * permalink.
 */
export function parseCrystallineAddress(
  address: string,
): { domain: string; permalink: string } | null {
  const scheme = "crystalline://";
  if (!address.startsWith(scheme)) {
    return null;
  }
  const rest = address.slice(scheme.length);
  const cut = rest.indexOf("/");
  if (cut <= 0) {
    return null;
  }
  const domain = rest.slice(0, cut);
  const permalink = rest.slice(cut + 1);
  return permalink === "" ? null : { domain, permalink };
}

/** The cache key of one engram. */
export function engramDetailKey(
  domain: string,
  permalink: string,
): readonly unknown[] {
  return ["engram", domain, permalink];
}

/** Read a `{ domain, target }` pair, or null when there is no target in it. */
function readTarget(value: unknown): LinkTarget | null {
  const record = asObject(value);
  const target = asString(record?.target);
  return target === null ? null : { domain: asString(record?.domain), target };
}

/** Read one reference, or null when it names nothing to point at. */
function readReference(value: unknown): EngramReference | null {
  const record = asObject(value);
  const target = readTarget(record?.target);
  if (target === null) {
    return null;
  }
  return {
    line: asNumber(record?.line),
    relType: asString(record?.rel_type),
    target,
    resolved: record?.resolved === true,
  };
}

/** Read one observation, or null when it carries no text. */
function readObservation(value: unknown): EngramObservation | null {
  const record = asObject(value);
  const content = asString(record?.content);
  if (content === null) {
    return null;
  }
  return {
    line: asNumber(record?.line),
    category: asString(record?.category),
    content,
    tags: asStrings(record?.tags),
    context: asString(record?.context),
  };
}

/**
 * Read the verification trail.
 *
 * `last_verified` is the legacy spelling and records no actor, so it becomes an
 * entry with none rather than one attributed to nobody in particular. It is
 * read only when the current key is absent: an engram that has both is one
 * mid-migration, and the newer key is the one that was written last.
 */
function readVerified(record: Record<string, unknown> | null): VerifiedEntry[] {
  const entries = asArray(record?.verified)
    .map((value) => {
      const entry = asObject(value);
      const by = asString(entry?.by);
      const at = asString(entry?.at);
      return by === null && at === null ? null : { by, at };
    })
    .filter((entry): entry is VerifiedEntry => entry !== null);
  if (entries.length > 0) {
    return entries;
  }
  const legacy = asString(record?.last_verified);
  return legacy === null ? [] : [{ by: null, at: legacy }];
}

/** Read the frontmatter block. */
function readFrontmatter(
  payload: Record<string, unknown> | null,
): EngramFrontmatter {
  const record = asObject(payload?.frontmatter);
  const extra = asObject(record?.extra);
  return {
    // The parsed struct spells it `engram_type`; the descriptor beside it on
    // the payload spells it `type`, and either one is the engram's own type.
    type: asString(record?.engram_type) ?? asString(payload?.type),
    status: asString(record?.status) ?? asString(payload?.status),
    tags: asStrings(record?.tags),
    // An unmodelled key, so it arrives under `extra`. Read from the top level
    // too, in case a later engine promotes it to a field of its own.
    salience: asNumber(extra?.salience) ?? asNumber(record?.salience),
    validFrom: asString(record?.valid_from),
    validTo: asString(record?.valid_to),
    staleAfter: asString(record?.stale_after) ?? asString(record?.review_after),
    verified: readVerified(record),
  };
}

/**
 * Read a detail payload.
 *
 * `domain` and `permalink` are what was asked for, used when the payload does
 * not name them: everything on this screen is addressed by that pair, and a
 * page that lost it would link back to nowhere.
 */
export function readEngramDetail(
  payload: unknown,
  domain: string,
  permalink: string,
): EngramDetail {
  const record = asObject(payload);
  const where = asString(record?.domain) ?? domain;
  const slug = asString(record?.permalink) ?? permalink;
  const inbound = asObject(record?.inbound);
  const inboundRefs = asArray(inbound?.refs).map((value) => {
    const ref = asObject(value);
    return {
      domain: asString(ref?.domain),
      path: asString(ref?.path),
      kind: asString(ref?.kind),
    };
  });
  return {
    domain: where,
    permalink: slug,
    title: asString(record?.title) ?? slug,
    url: asString(record?.url) ?? crystallineAddress(where, slug),
    path: asString(record?.path),
    content: asString(record?.content) ?? "",
    checksum: asString(record?.checksum),
    frontmatter: readFrontmatter(record),
    observations: asArray(record?.observations)
      .map(readObservation)
      .filter((entry): entry is EngramObservation => entry !== null),
    relations: asArray(record?.relations)
      .map(readReference)
      .filter((entry): entry is EngramReference => entry !== null),
    links: asArray(record?.links)
      .map(readReference)
      .filter((entry): entry is EngramReference => entry !== null),
    inboundCount: asNumber(inbound?.count) ?? inboundRefs.length,
    inboundRefs,
  };
}

/** Fetch one engram in full. */
export async function fetchEngramDetail(
  domain: string,
  permalink: string,
): Promise<EngramDetail> {
  const payload = await api<unknown>(engramPath(domain, permalink));
  return readEngramDetail(payload, domain, permalink);
}
