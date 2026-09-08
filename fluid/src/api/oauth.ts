/**
 * The two ends of OAuth for MCP clients that Fluid draws: deciding a pending
 * authorization on the consent screen, and listing (or revoking) the clients
 * an account has already connected.
 *
 * Neither surface ever carries a token. The consent pair works on a pending
 * request's opaque id - not a credential, a lookup key for a record that
 * authorizes nothing until a decision lands on it - and the grant listing is
 * built from `OauthGrantInfo`, which the server itself never puts a secret
 * into: only hashes are stored, so there is nothing to show back.
 */

import { api, encodeSegment } from "./client";
import type {
  AuthorizationView,
  DecisionResponse,
  OauthGrantInfo,
} from "./model";

/** The cache key of the caller's own connected-clients list. */
export const OAUTH_GRANTS_KEY = ["me-oauth-grants"] as const;

/**
 * The pending authorization the consent screen shows: who is asking, where
 * the answer goes, and who is about to grant it. A 404 here means the
 * request expired, was already decided, or never existed - one answer for
 * all three, so a guessed id learns nothing.
 */
export async function fetchAuthorization(
  id: string,
): Promise<AuthorizationView> {
  return api<AuthorizationView>(`/oauth/authorizations/${encodeSegment(id)}`);
}

/**
 * Allow or deny the pending authorization. The reply names where to navigate
 * next - an allow's code or a deny's `access_denied`, already appended to the
 * client's own redirect uri - and the caller navigates the whole page there,
 * never a fetch: only a real navigation can carry the browser back to the
 * client that started this.
 */
export async function decideAuthorization(
  id: string,
  decision: "allow" | "deny",
): Promise<DecisionResponse> {
  return api<DecisionResponse>(`/oauth/authorizations/${encodeSegment(id)}`, {
    method: "POST",
    body: JSON.stringify({ decision }),
  });
}

/** Every client this account has connected through OAuth. Carries no token material. */
export async function fetchOauthGrants(): Promise<OauthGrantInfo[]> {
  return api<OauthGrantInfo[]>("/me/oauth-grants");
}

/** Revoke one connected client. Both its access and refresh tokens stop working at once. */
export async function revokeOauthGrant(id: number): Promise<void> {
  await api(`/me/oauth-grants/${encodeSegment(String(id))}`, {
    method: "DELETE",
  });
}
