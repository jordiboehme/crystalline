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
  InviteOffer,
  InvitesResponse,
  RedeemBody,
  RedeemResponse,
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

/**
 * What an invite link offers: who made it, the domain and level, when it
 * ends and the way in. The token goes in the body, never in a URL. Every
 * way a link can be bad answers the same 404 sentence.
 */
export async function confirmInvite(token: string): Promise<InviteOffer> {
  return api<InviteOffer>("/invites/confirm", {
    method: "POST",
    body: JSON.stringify({ token }),
  });
}

/**
 * Take an invite link: as the signed-in account with the token alone, or
 * with a login and a password for a new local account. Without either the
 * server answers where to sign in (`outcome: sign_in`).
 */
export async function redeemInvite(body: RedeemBody): Promise<RedeemResponse> {
  return api<RedeemResponse>("/invites/redeem", {
    method: "POST",
    body: JSON.stringify(body),
  });
}
