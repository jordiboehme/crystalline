/**
 * Single sign-on, from this app's two ends of it: which providers the sign-in
 * screen should draw a button for, and which provider identities the signed-in
 * account holds.
 *
 * The sign-in and the linking themselves are NOT fetches. Both are a whole-page
 * navigation to `/api/v1/auth/oidc/login`, because what follows is a redirect
 * to the provider's own domain and a redirect back: a `fetch` would follow that
 * hop invisibly, in the background, and land nowhere a person can type a
 * password into. {@link ssoLoginUrl} is that address, and the only correct way
 * to use it is `window.location.assign` or an ordinary link.
 */

import { API_BASE, api, encodeSegment } from "./client";
import type { IdentityLinksResponse, ProvidersResponse } from "./model";

/** The cache key of the sign-in screen's provider probe. */
export const PROVIDERS_KEY = ["auth-providers"] as const;

/** The cache key of the caller's own identity links. */
export const IDENTITY_LINKS_KEY = ["me-identity-links"] as const;

/**
 * Which ways into this instance exist. Public: it is read before anybody is
 * signed in, and it carries the button's label and nothing else - no issuer,
 * no client id, no secret.
 */
export async function fetchProviders(): Promise<ProvidersResponse> {
  return api<ProvidersResponse>("/auth/providers");
}

/** The identities this account holds, and whether it also has a password. */
export async function fetchIdentityLinks(): Promise<IdentityLinksResponse> {
  return api<IdentityLinksResponse>("/me/identity-links");
}

/**
 * Give up the identity this account holds at `issuer`.
 *
 * Refused by the server when it is the account's last way in - no password and
 * no other identity - and that refusal is shown in the server's own words,
 * because they name the command that gives the account a password first.
 */
export async function unlinkIdentity(issuer: string): Promise<void> {
  await api(`/me/identity-links/${encodeSegment(issuer)}`, {
    method: "DELETE",
  });
}

/**
 * Where to send the browser to start a sign-on. `link` turns it into an
 * explicit linking of the provider identity to the account that is already
 * signed in, which is the only way an identity ever reaches an existing
 * account.
 *
 * `link=true` and never `link=1`: the server's query layer reads a bool, and
 * `1` is answered with a 400 that a whole-page navigation has no way to show.
 */
export function ssoLoginUrl(link = false): string {
  return `${API_BASE}/auth/oidc/login${link ? "?link=true" : ""}`;
}
