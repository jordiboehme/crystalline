/**
 * The domain listing, read defensively.
 *
 * `GET /domains` passes the engine's own JSON through untouched, which is the
 * point (the MCP tools and this API answer with one payload rather than two
 * shapes that drift) and also means the OpenAPI document types it as an opaque
 * object. So the shape is asserted here, once, by reading it rather than by
 * casting it: a field that is missing or a different type is dropped instead of
 * turning into a `TypeError` three components deep.
 */

import { api } from "./client";

/** One registered domain, as much of it as a client can rely on. */
export interface DomainSummary {
  /** The domain name, which is its key everywhere else. */
  name: string;
  /** `file` for a folder of markdown, `virtual` for a database-backed domain. */
  kind: string | null;
  /** How many engrams it holds, or null when the listing did not say. */
  engrams: number | null;
  /** The routing bullets from its MANIFEST: what this domain is for. */
  whenToUse: string[];
}

/** Everything `GET /domains` says. */
export interface DomainListing {
  domains: DomainSummary[];
  /** The behavior rules that govern every domain on this instance. */
  behavior: string[];
}

/** A JSON object, for reading fields off an unknown payload. */
type JsonObject = Record<string, unknown>;

function asObject(value: unknown): JsonObject | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as JsonObject)
    : null;
}

function asStrings(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === "string")
    : [];
}

/** Read one domain, or null when there is not even a name to key it by. */
function readDomain(value: unknown): DomainSummary | null {
  const record = asObject(value);
  const name = record?.name;
  if (typeof name !== "string" || name === "") {
    return null;
  }
  return {
    name,
    kind: typeof record?.kind === "string" ? record.kind : null,
    engrams: typeof record?.engrams === "number" ? record.engrams : null,
    whenToUse: asStrings(record?.when_to_use),
  };
}

/** Read the listing out of whatever the server sent. */
export function readListing(payload: unknown): DomainListing {
  const record = asObject(payload);
  const domains = Array.isArray(record?.domains) ? record.domains : [];
  return {
    domains: domains
      .map(readDomain)
      .filter((domain): domain is DomainSummary => domain !== null),
    behavior: asStrings(record?.behavior),
  };
}

/** The cache key of the domain listing. */
export const DOMAINS_QUERY_KEY = ["domains"] as const;

/** Fetch the domain listing. */
export async function fetchDomains(): Promise<DomainListing> {
  return readListing(await api<unknown>("/domains"));
}
