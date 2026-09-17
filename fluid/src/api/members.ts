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
import type { MemberLevel, MembersResponse } from "./model";

/** The cache key of one domain's membership. */
export function membersKey(domain: string): readonly unknown[] {
  return ["domain-members", domain];
}

/** Read one domain's owner, visibility and membership list. */
export async function fetchMembers(domain: string): Promise<MembersResponse> {
  return api<MembersResponse>(`/domains/${encodeSegment(domain)}/members`);
}

/**
 * Invite an account to a private domain, or move it to a different level.
 * One verb for both, since a `PUT` states what the membership should be
 * rather than whether it existed before.
 */
export async function setMember(
  domain: string,
  principal: string,
  level: MemberLevel,
): Promise<void> {
  await api(
    `/domains/${encodeSegment(domain)}/members/${encodeSegment(principal)}`,
    {
      method: "PUT",
      body: JSON.stringify({ level }),
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
