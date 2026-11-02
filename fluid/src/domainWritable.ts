/**
 * Whether this session may change one domain, and whether the domain wears
 * the lock.
 *
 * Two facts with two sources, kept apart on purpose (spec review point 6).
 * The lock is the domain's own `read_only` policy row, read off the MANIFEST
 * query the domain page and the policies card already share: only that row
 * draws the chip and its "its owner switches read_only off" line. Whether
 * this caller may write is the listing row's `read_only` (a viewer, an
 * account whose write list leaves the domain out, or the lock), folded with
 * the instance gate `capabilities.canWrite`. Every control that writes into
 * a domain asks `canWrite` here, never `capabilities.canWrite` alone.
 *
 * On a domain GitHub governs the instance role is not the gate: a viewer
 * with write access on GitHub writes there, and a GitHub reader does not
 * (Decision 6). The listing row's `read_only` already says so for a
 * signed-in account, and on a domain GitHub does not govern it already
 * carries the instance role's cap, so `mayWriteIn` follows the row alone
 * where the server sends the per-domain answer, and keeps the role rule
 * for the anonymous visitor, an instance without accounts and an older
 * server. The share surfaces follow the row's `mayShare` the same way.
 */
import { useQuery } from "@tanstack/react-query";

import { fetchManifest, manifestKey } from "./api/domain";
import type { ManifestView, PolicyView } from "./api/domain";
import { DOMAINS_QUERY_KEY, fetchDomains } from "./api/domains";
import type { DomainSummary } from "./api/domains";
import { useAuth } from "./auth/AuthContext";
import type { Capabilities } from "./auth/AuthContext";

/** What a one-click write control says while the lock is not read yet. */
export const CHECKING_LOCK = "Checking whether this domain is locked";

/** Whether these policy rows hold `read_only` on. */
export function policiesLocked(policies: PolicyView[] | undefined): boolean {
  return (
    policies?.some(
      (row) => row.key === "read_only" && row.effective === "on",
    ) === true
  );
}

/** Whether this MANIFEST's `read_only` row is on. */
export function domainLockedIn(manifest: ManifestView | undefined): boolean {
  return policiesLocked(manifest?.sections?.policies);
}

/**
 * Whether this session may write into the domain `entry` describes, the
 * lock left out (the caller folds it in where it applies).
 *
 * For a signed-in account on a server that sends the per-domain answer
 * (`mayShare` is present) the row's `read_only` is the whole answer: it
 * follows the GitHub role on a domain GitHub governs and carries the
 * instance role's cap everywhere else. The anonymous visitor, an instance
 * without accounts (its mark is never set) and an older server keep the
 * role rule.
 */
export function mayWriteIn(
  capabilities: Capabilities,
  entry: DomainSummary | undefined,
): boolean {
  const blocked = entry?.writeBlocked === true;
  if (
    entry?.mayShare != null &&
    capabilities.role !== null &&
    !capabilities.anonymous
  ) {
    return !capabilities.readOnly && !blocked;
  }
  return capabilities.canWrite && !blocked;
}

/**
 * Whether this session may drive the share surfaces of the domain `entry`
 * describes: the server's per-domain answer, or the instance-wide one from
 * a server that does not send it.
 */
export function mayShareIn(
  capabilities: Capabilities,
  entry: DomainSummary | undefined,
): boolean {
  return entry?.mayShare ?? capabilities.canShare;
}

export function useDomainWritable(domain: string): {
  canWrite: boolean;
  /** Whether this session may drive this domain's share surfaces. */
  canShare: boolean;
  domainLocked: boolean;
  /**
   * Whether the MANIFEST has been read (or failed to read), so
   * `domainLocked` is an answer rather than "not known yet". A control that
   * writes on one click waits for it.
   */
  lockKnown: boolean;
} {
  const { capabilities } = useAuth();
  const listing = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });
  const manifest = useQuery({
    queryKey: manifestKey(domain),
    queryFn: () => fetchManifest(domain),
    enabled: domain !== "",
  });
  const domainLocked = domainLockedIn(manifest.data);
  const entry = listing.data?.domains.find((row) => row.name === domain);
  return {
    canWrite: mayWriteIn(capabilities, entry) && !domainLocked,
    canShare: mayShareIn(capabilities, entry),
    domainLocked,
    lockKnown: domain !== "" && !manifest.isPending,
  };
}
