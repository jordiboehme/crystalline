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
import { asObject, asStrings } from "./json";

/** One registered domain, as much of it as a client can rely on. */
export interface DomainSummary {
  /** The domain name, which is its key everywhere else. */
  name: string;
  /** `file` for a folder of markdown, `virtual` for a database-backed domain. */
  kind: string | null;
  /** How many engrams it holds, or null when the listing did not say. */
  engrams: number | null;
  /**
   * When this domain was last synced into the index, or null when the listing
   * did not say. A sync is not the same fact as an engram being recorded, so
   * whatever shows it has to name it as what it is.
   */
  lastSync: string | null;
  /** The routing bullets from its MANIFEST: what this domain is for. */
  whenToUse: string[];
  /**
   * Whether the domain is private: visible only to its owner, the accounts
   * invited into it and instance admins.
   *
   * False for a shared domain and false for a listing that does not say -
   * an older server, or an installation with no accounts database, where no
   * domain has ever been made private. A domain this session may not read is
   * not in the listing at all, so a row is never withheld here, only marked.
   */
  private: boolean;
  /**
   * `"overlay"` while this domain reviews changes before they land - every
   * write joins its author's own draft and the folder changes only through a
   * reviewed proposal - and null while it takes changes directly, which is how
   * a domain starts out and what an older server says about every domain.
   */
  review: string | null;
  /**
   * How many drafts this session's own account holds in this domain, null when
   * the domain takes changes directly (nobody can draft there), and null again
   * when the server could not count them.
   *
   * It rides on the listing rather than only on the domain's sync status
   * because that status is gated with the share verbs: a plain member of a
   * reviewing domain could not reach their own count, and a count of your own
   * unshared work is a fact about you rather than about the team.
   */
  myDrafts: number | null;
}

/** Everything `GET /domains` says. */
export interface DomainListing {
  domains: DomainSummary[];
  /** The behavior rules that govern every domain on this instance. */
  behavior: string[];
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
    lastSync: typeof record?.last_sync === "string" ? record.last_sync : null,
    whenToUse: asStrings(record?.when_to_use),
    private: record?.private === true,
    review: typeof record?.review === "string" ? record.review : null,
    myDrafts: typeof record?.my_drafts === "number" ? record.my_drafts : null,
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
