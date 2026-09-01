/**
 * Whose GitHub identity a write to a team origin would go out on.
 *
 * An instance shares one of two ways. In the default mode every proposal is
 * authored by the machine's own credential, and nothing here has anything to
 * say: the dialogs are the dialogs they always were, and not one request is
 * made about an identity that is not in play. In personal mode the write
 * carries the name of whoever made it, which in the browser is the SESSION -
 * never the machine owner, whose slot the status report also names and which is
 * what a CLI or a local stdio share resolves instead.
 *
 * Two facts, two reads, and neither is a new round trip in the common case. The
 * mode rides the per-domain sync status the card behind the dialog already
 * asked for - same key, same fetcher, so mounting a dialog over that card is a
 * cache read, and one opened from the top bar for a domain nobody is standing
 * in shares the single request its own status query makes. The identity is the
 * profile card's own query under the profile card's own key, so a session that
 * has been to its profile pays nothing either; it is fetched only where the
 * mode says it matters, which is what lets the default mode be pinned as asking
 * nothing at all.
 *
 * The strictness this serves is the spec's, not a nicety: in personal mode a
 * write with no personal credential is REFUSED by the engine, with a sentence
 * naming the fix. A dialog that still offered its button would be offering a
 * refusal, so the primary action becomes the fix itself.
 *
 * Everything a read can show still shows, and that is the server's behavior
 * rather than this file's hope: `GET /domains/{domain}/sync/changes` serves the
 * plan to a caller with no personal identity, computing it on the instance
 * credential, because it writes nothing and a personal token reads nothing more
 * (pinned on the wire in `rest_admin_api.rs`, "a disconnected editor previews a
 * share and is still refused the share"). So the checkbox list is on the screen
 * before the connect, which is what makes connecting the last step before a
 * decision rather than a hoop in front of an unknown - while the share itself,
 * one route down, still refuses.
 */

import { useQuery } from "@tanstack/react-query";

import {
  MY_GITHUB_IDENTITY_KEY,
  fetchMyGithubIdentity,
  fetchSyncStatus,
  syncStatusKey,
} from "../api/admin";

/** What a dialog needs to know about the identity behind its primary action. */
export interface ShareIdentity {
  /**
   * Personal mode with no credential on file: the write would be refused, so
   * the dialog offers the way to fix that instead of the write.
   */
  mustConnect: boolean;
  /**
   * The login a write would go out as, or null when there is nothing to say -
   * the default mode, an identity still arriving, or a connection whose login
   * the server did not name.
   */
  sharingAs: string | null;
  /**
   * Personal mode with the identity read still in flight. Brief, and the one
   * moment a dialog holds its primary action: offering a write that is about
   * to be refused, or a connect that may not be needed, are both worse than a
   * button that is grey for a beat.
   */
  asking: boolean;
}

/**
 * Read whose identity a write to `domain` would carry.
 *
 * `active` is what keeps this out of the way once a dialog has stopped being a
 * dialog about a write. An invalidation refetches ACTIVE observers whatever
 * their staleness, and the status key is invalidated by every share: an
 * observer left live here would answer that by pulling the origin again, for a
 * dialog that is showing an outcome by then. The share dialog switches this off
 * for exactly that reason, the same way it switches off its own two queries.
 */
export function useShareIdentity(domain: string, active = true): ShareIdentity {
  // The card's own query to the letter, which is what makes this a cache read
  // rather than a second call. Never retried and never refreshed while a
  // dialog is up: the refusals it can carry are immediate and final, and
  // nothing about which mode an instance shares in changes because a field was
  // typed in.
  const status = useQuery({
    queryKey: syncStatusKey(domain),
    queryFn: () => fetchSyncStatus(domain),
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    retry: false,
    enabled: active,
  });
  const personal = status.data?.shareIdentity === "personal";

  // Only in personal mode, and that gate is the whole promise the default mode
  // is pinned on: an instance sharing as itself asks nothing about anybody's
  // personal credential. `staleTime: Infinity` is settled rather than stale
  // here - connecting and disconnecting both invalidate this key, from the
  // profile card, which is the only place either happens.
  const identity = useQuery({
    queryKey: MY_GITHUB_IDENTITY_KEY,
    queryFn: fetchMyGithubIdentity,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    retry: false,
    enabled: personal && active,
  });

  const connected = identity.data?.connected === true;
  return {
    // Only on an answer. A read that failed leaves the write offered and the
    // engine's own sentence to explain itself: an unreadable credential store
    // is not grounds for telling somebody to go and connect what they may
    // already have connected.
    mustConnect: personal && identity.isSuccess && !connected,
    sharingAs: personal && connected ? (identity.data?.login ?? null) : null,
    asking: personal && active && identity.isPending,
  };
}
