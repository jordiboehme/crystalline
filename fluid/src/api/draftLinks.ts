/**
 * Draft share-links: handing one unfolded page to one person, and - if they
 * take the second step - letting them type in it.
 *
 * Two states, never one. A link makes a draft VISIBLE to whoever redeems it;
 * a join makes it EDITABLE, and the join belongs to this browser session
 * rather than to the account. That is why the key lives in session storage and
 * never in local storage: closing the window ends the join, and an account's
 * agent - which authenticates as the same account - has not joined anything
 * because its person pressed a button here.
 */

import { api } from "./client";
import type { AcceptedDraft, MintedLinkResponse, OverlayGrant } from "./model";

/**
 * Where this window remembers the draft it is working inside.
 *
 * Session storage, deliberately: it is scoped to one tab and one window and is
 * gone when either ends, which is exactly the lifetime a join has. Local
 * storage would outlive the browser session on disk and would silently rejoin
 * somebody's draft days later.
 */
export const JOIN_KEY_STORAGE = "fluid.draft.join";

/** The header a write carries its join key in. Mirrors the server's own name. */
export const JOIN_HEADER = "X-Crystalline-Join";

/**
 * Fired on `window` whenever the stored join changes.
 *
 * The browser's own `storage` event fires only in the OTHER tabs of a shared
 * origin, and session storage is not shared between tabs at all - so the tab
 * that joined, which is the one that has to redraw, would never hear anything.
 * This is that missing event.
 */
export const JOIN_CHANGED_EVENT = "fluid:draft-join-changed";

/** The draft this window is inside, as it is remembered between renders. */
export interface HeldJoin {
  /** The key every write sends back. */
  key: string;
  domain: string;
  path: string;
  owner: string;
  /** The address the draft answers to, which is what a save is addressed by. */
  permalink: string;
}

/**
 * The join this window is holding, or null.
 *
 * A browser that refuses storage - a private window, an embedded view - is not
 * a reason to fail to draw anything, so it reads as "not joined" and the
 * person can join again.
 */
export function heldJoin(): HeldJoin | null {
  try {
    const raw = sessionStorage.getItem(JOIN_KEY_STORAGE);
    if (!raw) return null;
    const held: unknown = JSON.parse(raw);
    if (
      typeof held === "object" &&
      held !== null &&
      typeof (held as HeldJoin).key === "string"
    ) {
      return held as HeldJoin;
    }
    return null;
  } catch {
    return null;
  }
}

/** Remember the draft this window is inside, or forget it. */
export function rememberJoin(held: HeldJoin | null): void {
  try {
    if (held) {
      sessionStorage.setItem(JOIN_KEY_STORAGE, JSON.stringify(held));
    } else {
      sessionStorage.removeItem(JOIN_KEY_STORAGE);
    }
  } catch {
    // A window that cannot remember simply is not joined on the next render,
    // which is a worse experience and not a broken one.
  }
  // Fired whether or not the write landed: a bar drawn from a join this
  // window could not store would be a bar nothing can take down.
  window.dispatchEvent(new Event(JOIN_CHANGED_EVENT));
}

/**
 * Mint a link on one of the caller's own drafts. The reply is the only place
 * the link is readable - only its hash is stored - so a caller that loses it
 * mints another rather than reading this one back.
 */
export async function mintDraftLink(
  domain: string,
  path: string,
): Promise<MintedLinkResponse> {
  return api<MintedLinkResponse>(
    `/domains/${encodeURIComponent(domain)}/draft-links`,
    { method: "POST", body: JSON.stringify({ path }) },
  );
}

/** The cache key of the links standing on one draft. */
export function draftLinksKey(domain: string, path: string) {
  return ["draft-links", domain, path] as const;
}

/** The links standing on one of the caller's own drafts. Never carries a token. */
export async function fetchDraftLinks(
  domain: string,
  path: string,
): Promise<OverlayGrant[]> {
  return api<OverlayGrant[]>(
    `/domains/${encodeURIComponent(domain)}/draft-links?path=${encodeURIComponent(path)}`,
  );
}

/** Take one link back. It stops opening anything at once. */
export async function revokeDraftLink(id: number): Promise<void> {
  await api(`/draft-links/${encodeURIComponent(String(id))}`, {
    method: "DELETE",
  });
}

/** Present a link and receive the draft it opens, read-only or not. */
export async function acceptDraftLink(token: string): Promise<AcceptedDraft> {
  return api<AcceptedDraft>("/draft-links/accept", {
    method: "POST",
    body: JSON.stringify({ token }),
  });
}

/** Start working inside the granted draft. The reply carries this window's key. */
export async function joinDraft(token: string): Promise<AcceptedDraft> {
  return api<AcceptedDraft>("/draft-links/join", {
    method: "POST",
    body: JSON.stringify({ token }),
  });
}

/**
 * Stop working inside it. The link is untouched, so the draft is still
 * readable and joining again is one press.
 */
export async function leaveDraft(key: string): Promise<void> {
  await api("/draft-links/leave", {
    method: "POST",
    body: JSON.stringify({ key }),
  });
}

/** Save the granted draft. Carries the join key, which is what routes it. */
export async function saveJoinedDraft(
  held: HeldJoin,
  content: string,
  checksum: string,
): Promise<AcceptedDraft> {
  return api<AcceptedDraft>(
    `/domains/${encodeURIComponent(held.domain)}/engrams/${held.permalink
      .split("/")
      .map(encodeURIComponent)
      .join("/")}`,
    {
      method: "PUT",
      headers: { "If-Match": `"${checksum}"`, [JOIN_HEADER]: held.key },
      body: JSON.stringify({ content }),
    },
  );
}
