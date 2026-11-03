/**
 * Who owns a domain and who is invited into it, while it is private.
 *
 * One route answers all of it (`GET /domains/{domain}/members`): the owner,
 * the visibility and the membership list in one payload, because the card
 * that draws them decides what to offer from that one read rather than from
 * the caller's instance role alone - a manager and an owner and an instance
 * admin all reach a private domain by three different doors, and only this
 * response says which one a given caller came through.
 *
 * A shared domain answers honestly rather than refusing: `visibility:
 * "shared"`, no owner, no members - membership only means something while a
 * domain is private.
 */

import { api, encodeSegment } from "./client";
import type {
  DomainMember,
  GithubPerson,
  MemberLevel,
  MembersResponse,
  ReflectionGrant,
  ReflectionLevel,
} from "./model";

/**
 * Whether two login names are the same account.
 *
 * Names are folded to lowercase and trimmed on the way in, so every reader
 * of this payload compares them the same way - the members card deriving
 * what a caller may do here, and the danger zone deciding whether the caller
 * owns the domain it is about to change the visibility of.
 */
export function sameAccount(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}

/** The cache key of one domain's membership. */
export function membersKey(domain: string): readonly unknown[] {
  return ["domain-members", domain];
}

/**
 * The fields a domain GitHub governs adds to the members read, with what
 * each one means when an older server leaves it out: a domain Crystalline
 * decides, no GitHub people, nothing overridden, no collaborators to manage.
 */
export interface Governance {
  /** Whether access to this domain follows the GitHub App. */
  github: boolean;
  /** The people GitHub lists, or null where GitHub does not decide. */
  githubPeople: GithubPerson[] | null;
  /** The Crystalline grants GitHub overrides on a governed domain. */
  overridden: DomainMember[];
  /** The Reflection collaborators, for a caller who manages them; else null. */
  reflectionCollaborators: ReflectionGrant[] | null;
  /** The sentence that says the visibility is set in the environment, or null. */
  visibilityFixedBy: string | null;
  /** Why a closed domain is closed, for an instance admin; else null. */
  closedReason: string | null;
  /** The sentence that says the repository is archived, or null. */
  archivedReason: string | null;
}

/** The governance half of one members read, defaulted where a field is absent. */
export function governanceOf(data: MembersResponse): Governance {
  // Read as a partial: a server from before these fields answers without
  // them, and a missing one means the plain, Crystalline-decided case.
  const wire = data as Partial<MembersResponse>;
  return {
    github: wire.governance === "github",
    githubPeople: wire.github_people ?? null,
    overridden: wire.overridden ?? [],
    reflectionCollaborators: wire.reflection_collaborators ?? null,
    visibilityFixedBy: wire.visibility_fixed_by ?? null,
    closedReason: wire.closed_reason ?? null,
    archivedReason: wire.archived_reason ?? null,
  };
}

/** Read one domain's owner, visibility and membership list. */
export async function fetchMembers(domain: string): Promise<MembersResponse> {
  return api<MembersResponse>(`/domains/${encodeSegment(domain)}/members`);
}

/**
 * Give an account the Reflection of one domain at `reader` or
 * `contributor`, or change the level it holds there.
 */
export async function setReflectionCollaborator(
  domain: string,
  account: string,
  level: "reader" | "contributor",
): Promise<void> {
  await api(
    `/domains/${encodeSegment(domain)}/reflection-collaborators/${encodeSegment(account)}`,
    { method: "PUT", body: JSON.stringify({ level }) },
  );
}

/** Take an account's Reflection grant on one domain away, or give up one's own. */
export async function removeReflectionCollaborator(
  domain: string,
  account: string,
): Promise<void> {
  await api(
    `/domains/${encodeSegment(domain)}/reflection-collaborators/${encodeSegment(account)}`,
    { method: "DELETE" },
  );
}

/**
 * Invite an account to a private domain, or move it to a different level.
 * One verb for both, since a `PUT` states what the membership should be
 * rather than whether it existed before. Both axes travel together: the
 * knowledge level and the Reflection level beside it.
 */
export async function setMember(
  domain: string,
  principal: string,
  level: MemberLevel,
  reflection: ReflectionLevel,
): Promise<void> {
  await api(
    `/domains/${encodeSegment(domain)}/members/${encodeSegment(principal)}`,
    {
      method: "PUT",
      body: JSON.stringify({ level, reflection }),
    },
  );
}

/** Remove a membership, or leave a domain - the same call, by who is named. */
export async function removeMember(
  domain: string,
  principal: string,
): Promise<void> {
  await api(
    `/domains/${encodeSegment(domain)}/members/${encodeSegment(principal)}`,
    { method: "DELETE" },
  );
}

/** Hand a private domain to a different account. */
export async function setOwner(domain: string, owner: string): Promise<void> {
  await api(`/domains/${encodeSegment(domain)}/owner`, {
    method: "PUT",
    body: JSON.stringify({ owner }),
  });
}

/** Make a domain private, or share it with the whole instance again. */
export async function setVisibility(
  domain: string,
  private_: boolean,
): Promise<void> {
  await api(`/domains/${encodeSegment(domain)}/visibility`, {
    method: "PUT",
    body: JSON.stringify({ private: private_ }),
  });
}
