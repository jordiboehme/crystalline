/**
 * Invite links: a link someone new opens to join this instance, used once.
 * The token travels in the link's fragment and comes back from the server
 * once, in the answer that made it. Callers keep it in a plain component
 * field and nowhere else.
 */

import { api, encodeSegment } from "./client";
import type {
  CreateInviteBody,
  CreatedInviteResponse,
  InvitesResponse,
} from "./model";

/** The cache key of the invite list. */
export const INVITES_KEY = ["invites"] as const;

/** Every link the caller may see, newest first, and the way in they offer. */
export async function fetchInvites(): Promise<InvitesResponse> {
  return api<InvitesResponse>("/invites");
}

/** Make an invite link. */
export async function createInvite(
  body: CreateInviteBody,
): Promise<CreatedInviteResponse> {
  return api<CreatedInviteResponse>("/invites", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

/** Revoke a pending link. It stops working at once. */
export async function revokeInvite(id: number): Promise<void> {
  await api(`/invites/${encodeSegment(String(id))}`, { method: "DELETE" });
}
