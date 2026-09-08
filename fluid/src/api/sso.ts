/**
 * Single sign-on, from this app's two ends of it: which providers the sign-in
 * screen should draw a button for, and which provider identities the signed-in
 * account holds.
 *
 * Neither the sign-in nor the linking ENDS in a fetch: what follows either one
 * is a redirect to the provider's own domain and a redirect back, so the whole
 * page has to navigate. A background fetch would walk that hop invisibly and
 * land nowhere anybody can type a password into.
 *
 * They start differently, though, and that difference is a security property
 * rather than a style choice. An ordinary sign-in is a link to
 * {@link ssoSignInUrl}: it is public, it signs in whoever the provider says,
 * and there is nothing for another origin to abuse. Starting a LINK ties the
 * identity to the account already signed in here, so it must not be startable
 * by another origin - the session cookie is `SameSite=Lax` and would ride a
 * cross-site top-level navigation. {@link startSsoLink} therefore POSTs, which
 * carries the CSRF token and which no cross-site form can send with the
 * cookie, and hands back where to navigate.
 */

import { API_BASE, api, encodeSegment } from "./client";
import type {
  IdentityLinksResponse,
  ProvidersResponse,
  StartLinkResponse,
} from "./model";

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
 * Where to send the browser to sign in with the provider. A plain address, for
 * a plain link: this is the public way in, and it signs in whoever the
 * provider turns out to say it is.
 *
 * `returnTo`, when given, rides along as `return_to`: a path on this
 * instance for the callback to land the browser on once the sign-in
 * completes, instead of the home screen. The OAuth consent page is what this
 * exists for - `RequireAuth` carries the address it interrupted to the login
 * screen, and a provider sign-in started from there has to come back to that
 * exact address, because the pending authorization it names is only
 * reachable by its id.
 */
export function ssoSignInUrl(returnTo?: string): string {
  const url = `${API_BASE}/auth/oidc/login`;
  return returnTo ? `${url}?return_to=${encodeURIComponent(returnTo)}` : url;
}

/**
 * Start a sign-on that links its identity to the account already signed in,
 * and hand back where to navigate.
 *
 * The POST does two things before the browser leaves: it proves the request
 * came from this app (the CSRF token `api` attaches) and it records, server
 * side, which account the journey is for. The caller then navigates the whole
 * page to `location`; nothing is linked until the provider sends the browser
 * back to the callback, and only ever to the account that started it.
 */
export async function startSsoLink(): Promise<StartLinkResponse> {
  return api<StartLinkResponse>("/auth/oidc/login", { method: "POST" });
}
