/**
 * One `EventSource` per tab, and what arrives on it turned into
 * invalidations.
 *
 * Mounted once by the shell, inside the auth provider and never on the
 * login or setup screens: a stream needs a session, and two streams per tab
 * would be two refetches per change. Reconnect is the browser's own: an
 * `EventSource` reopens after an error and sends `Last-Event-ID` itself, so
 * the only error this handles is the one it cannot recover from - a source
 * the server closed for good, which is a session that ended.
 *
 * Bursts are coalesced: keys collect for 250 ms after the first frame and
 * one `invalidateQueries` fires per distinct key. A `reset` flushes the set
 * and invalidates everything at once. Nothing is ever written into the
 * cache; the pages refetch what they show.
 *
 * `subscribeToChanges` is a small public seam onto this same source (Jordi,
 * 2026-09-27, Section J (m)): a listener registration usable from anywhere,
 * not only from inside `Layout`. It shares the one `EventSource` per tab;
 * it never opens a second connection and never polls. A listener added
 * while no provider is mounted is simply not called until one is - the
 * station game's route mounts outside `Layout` and plugs in here during
 * its own milestone, not this wave.
 */

import type { Query } from "@tanstack/react-query";
import { useQueryClient } from "@tanstack/react-query";
import type { ReactElement, ReactNode } from "react";
import { useEffect, useMemo } from "react";

import { API_BASE } from "../api/client";
import type { ChangeEvent } from "../api/events";
import { parseFrame } from "../api/events";
import { ME_QUERY_KEY } from "../auth/keys";
import type { IgnoredEngrams } from "./ignored";
import { IgnoredEngramsContext, createIgnoredEngrams } from "./ignored";
import type { QueryKey } from "./invalidation";
import { keysFor } from "./invalidation";
import { RecentChanges, RecentChangesContext } from "./recent";

/** How long after the first frame the pending keys wait for company. */
export const COALESCE_MS = 250;

/**
 * `EventSource.CLOSED`, spelled here because a runtime without the global
 * (jsdom, where the provider runs on an injected factory) has no constructor
 * to read it off.
 */
const CLOSED = 2;

export type StreamFactory = (url: string) => EventSource;

/** A callback for every frame the one stream carries, whatever it invalidates. */
export type ChangeListener = (event: ChangeEvent) => void;

const changeSubscribers = new Set<ChangeListener>();

/**
 * Register `listener` on the one stream, usable outside `Layout`. Returns
 * the unsubscribe function. No second connection, no polling: this fans
 * out whatever the mounted provider's own `EventSource` already receives.
 */
// The seam is the point of this module as much as the provider is; a hot
// reload that remounts the provider keeps the listeners, which live here.
// eslint-disable-next-line react-refresh/only-export-components
export function subscribeToChanges(listener: ChangeListener): () => void {
  changeSubscribers.add(listener);
  return () => {
    changeSubscribers.delete(listener);
  };
}

const EVENT_NAMES = ["engram", "domain", "reset"] as const;

function defaultFactory(url: string): EventSource {
  return new EventSource(url, { withCredentials: true });
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
  streamFactory,
}: {
  children: ReactNode;
  streamFactory?: StreamFactory;
}): ReactElement {
  const queryClient = useQueryClient();
  const ignored = useMemo(() => createIgnoredEngrams(), []);
  const recent = useMemo(() => new RecentChanges(), []);

  useEffect(() => {
    // A runtime with no `EventSource` (the test harness's jsdom) mounts the
    // shell inert rather than throwing: every screen test renders this.
    if (!streamFactory && typeof EventSource === "undefined") {
      return undefined;
    }
    const open = streamFactory ?? defaultFactory;
    const source = open(`${API_BASE}/events`);
    const pending = new Map<string, QueryKey>();
    let timer: ReturnType<typeof setTimeout> | null = null;

    const flush = () => {
      timer = null;
      const keys = [...pending.values()];
      pending.clear();
      for (const queryKey of keys) {
        if (
          (queryKey[0] === "engram" || queryKey[0] === "graph") &&
          queryKey.length < 3
        ) {
          // A domain event's prefix over every engram of the domain: the
          // editor's exemption holds here too, so the prefix passes over the
          // engram whose room carries its text and reaches every other one.
          void queryClient.invalidateQueries({
            queryKey,
            predicate: (query: Query) => !exempt(ignored, query.queryKey),
          });
        } else {
          void queryClient.invalidateQueries({ queryKey });
        }
      }
    };
    const onFrame = (name: string) => (message: MessageEvent<string>) => {
      const event = parseFrame(name, message.data);
      if (!event) return;
      for (const listener of changeSubscribers) {
        // One listener's bug must not cost the page its own refresh, nor the
        // listeners after it their frame.
        try {
          listener(event);
        } catch (error) {
          console.error("a change listener threw", error);
        }
      }
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
        // The editor's exemption: the open engram's detail and graph stay.
        if (exempt(ignored, key)) {
          continue;
        }
        pending.set(JSON.stringify(key), key);
      }
      timer ??= setTimeout(flush, COALESCE_MS);
    };
    const listeners = EVENT_NAMES.map((name) => [name, onFrame(name)] as const);
    for (const [name, listener] of listeners) {
      source.addEventListener(name, listener as EventListener);
    }
    source.onerror = () => {
      // CONNECTING is the browser mid-reconnect; CLOSED is a 401 or 403 the
      // browser will not retry, which is the session having ended.
      if (source.readyState === CLOSED) {
        void queryClient.invalidateQueries({ queryKey: ME_QUERY_KEY });
      }
    };
    return () => {
      if (timer) clearTimeout(timer);
      for (const [name, listener] of listeners) {
        source.removeEventListener(name, listener as EventListener);
      }
      source.onerror = null;
      source.close();
    };
  }, [queryClient, streamFactory, ignored, recent]);

  return (
    <IgnoredEngramsContext value={ignored}>
      <RecentChangesContext value={recent}>{children}</RecentChangesContext>
    </IgnoredEngramsContext>
  );
}
