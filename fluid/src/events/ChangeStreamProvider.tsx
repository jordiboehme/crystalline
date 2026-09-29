/**
 * What arrives on the change stream, turned into invalidations in this tab.
 *
 * Mounted once by the shell, inside the auth provider and never on the
 * login or setup screens: a stream needs a session. The stream itself is
 * one per browser, not per tab: `hub.ts` elects the tab that holds it and
 * passes every frame to the others, so any number of tabs costs the
 * browser one connection.
 *
 * Bursts are coalesced: keys collect for 250 ms after the first frame and
 * one `invalidateQueries` fires per distinct key. A `reset` flushes the set
 * and invalidates everything at once. Nothing is ever written into the
 * cache; the pages refetch what they show.
 *
 * `subscribeToChanges` is a small public seam onto the same stream (Jordi,
 * 2026-09-27): a listener registration usable from anywhere, in any tab,
 * inside the shell or not. It never opens a second connection and never
 * polls; a listener in a tab with no shell makes that tab take part in the
 * stream on its own. Without options the listener hears every frame this
 * tab takes. A subscriber that shows somebody as signed in passes
 * `identity`: it is then registered like the shell, so frames streamed as
 * another account never reach it and it is asked to re-check when the
 * session ended or moved. `onCarrier` hears the stream go down and come
 * back. `subscribeOn` is the same on a given hub.
 */

import type { Query } from "@tanstack/react-query";
import { useQueryClient } from "@tanstack/react-query";
import type { ReactElement, ReactNode } from "react";
import { useEffect, useMemo } from "react";

import type { ChangeEvent } from "../api/events";
import { ME_QUERY_KEY } from "../auth/keys";
import type { ChangeConsumer, ChangeHub, ChangeListener } from "./hub";
import { defaultHub } from "./hub";
import type { IgnoredEngrams } from "./ignored";
import { IgnoredEngramsContext, createIgnoredEngrams } from "./ignored";
import type { QueryKey } from "./invalidation";
import { keysFor } from "./invalidation";
import { RecentChanges, RecentChangesContext } from "./recent";

export type {
  ChangeListener,
  HubDeps,
  SessionProbe,
  StreamFactory,
} from "./hub";

/** How long after the first frame the pending keys wait for company. */
export const COALESCE_MS = 250;

/** What a subscriber outside the shell may hand the stream (M4 C10, C11). */
export interface StreamSubscription {
  /** Who the subscriber shows as signed in, and how it re-asks. */
  identity?: { held(): unknown; recheck(): void };
  /** The stream went down (false) or came back (true). */
  onCarrier?: (up: boolean) => void;
}

/**
 * `subscribeToChanges` on a given hub: the seam the tests build tabs with.
 *
 * Without options this is `hub.subscribe(listener)`. With either option the
 * listener is attached as a `ChangeConsumer`, which counts as a shell
 * mounting (it takes a tab whose session ended back into the running). A
 * consumer whose `held()` answers `undefined`, or one that passes no
 * `identity` at all, counts as "no shell" when the hub asks who this tab
 * shows: it filters nothing.
 */
// The seam is the point of this module as much as the provider is.
// eslint-disable-next-line react-refresh/only-export-components
export function subscribeOn(
  hub: ChangeHub,
  listener: ChangeListener,
  options?: StreamSubscription,
): () => void {
  if (!options?.identity && !options?.onCarrier) {
    return hub.subscribe(listener);
  }
  // Called on the object it came with, so a subscriber's methods keep
  // their `this`.
  const identity = options.identity;
  const consumer: ChangeConsumer = {
    onEvent: listener,
    heldIdentity: () => identity?.held(),
    recheckIdentity: () => {
      identity?.recheck();
    },
    ...(options.onCarrier ? { onCarrier: options.onCarrier } : {}),
  };
  return hub.attach(consumer);
}

/**
 * Register `listener` on the one stream, usable outside `Layout` and in any
 * tab: `subscribeOn(defaultHub(), ...)`. Returns the unsubscribe function.
 * No second connection, no polling: this fans out whatever the browser's
 * one `EventSource` receives.
 */
// eslint-disable-next-line react-refresh/only-export-components
export function subscribeToChanges(
  listener: ChangeListener,
  options?: StreamSubscription,
): () => void {
  return subscribeOn(defaultHub(), listener, options);
}

/** Whether `key` is the detail or a graph of an engram the editor holds. */
function exempt(ignored: IgnoredEngrams, key: readonly unknown[]): boolean {
  return (
    (key[0] === "engram" || key[0] === "graph") &&
    typeof key[1] === "string" &&
    typeof key[2] === "string" &&
    ignored.has(key[1], key[2])
  );
}

export function ChangeStreamProvider({
  children,
  hub,
}: {
  children: ReactNode;
  /** Test seam; the shell uses this tab's own hub. */
  hub?: ChangeHub;
}): ReactElement {
  const queryClient = useQueryClient();
  const ignored = useMemo(() => createIgnoredEngrams(), []);
  const recent = useMemo(() => new RecentChanges(), []);

  useEffect(() => {
    const pending = new Map<string, QueryKey>();
    let timer: ReturnType<typeof setTimeout> | null = null;

    const flush = () => {
      timer = null;
      const keys = [...pending.values()];
      pending.clear();
      for (const queryKey of keys) {
        // The editor's exemption, decided here for every key alike, so an
        // editor that mounted inside the window is honoured too.
        if (queryKey[0] === "engram" || queryKey[0] === "graph") {
          if (queryKey.length >= 3) {
            if (exempt(ignored, queryKey)) continue;
            void queryClient.invalidateQueries({ queryKey });
          } else {
            // A domain event's prefix over every engram of the domain passes
            // over the engram whose room carries its text.
            void queryClient.invalidateQueries({
              queryKey,
              predicate: (query: Query) => !exempt(ignored, query.queryKey),
            });
          }
        } else {
          void queryClient.invalidateQueries({ queryKey });
        }
      }
    };
    const onEvent = (event: ChangeEvent) => {
      const keys = keysFor(event);
      if (keys === "everything") {
        pending.clear();
        if (timer) clearTimeout(timer);
        timer = null;
        void queryClient.invalidateQueries();
        return;
      }
      if (event.event === "engram") {
        recent.note(event.change);
      }
      for (const key of keys) {
        pending.set(JSON.stringify(key), key);
      }
      timer ??= setTimeout(flush, COALESCE_MS);
    };

    const detach = (hub ?? defaultHub()).attach({
      onEvent,
      // The path the query client runs for a 401: the probe answers, and
      // the login screen or the right account replaces what the shell shows.
      recheckIdentity: () => {
        void queryClient.invalidateQueries({ queryKey: ME_QUERY_KEY });
      },
      heldIdentity: () => queryClient.getQueryData(ME_QUERY_KEY),
    });
    return () => {
      if (timer) clearTimeout(timer);
      detach();
    };
  }, [queryClient, hub, ignored, recent]);

  return (
    <IgnoredEngramsContext value={ignored}>
      <RecentChangesContext value={recent}>{children}</RecentChangesContext>
    </IgnoredEngramsContext>
  );
}
