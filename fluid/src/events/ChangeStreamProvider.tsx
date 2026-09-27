/**
 * One `EventSource` per tab, and what arrives on it turned into
 * invalidations.
 *
 * Mounted once by the shell, inside the auth provider and never on the
 * login or setup screens: a stream needs a session, and two streams per tab
 * would be two refetches per change. A dropped connection is the browser's
 * own to reconnect: an `EventSource` reopens after a network error and sends
 * `Last-Event-ID` itself. A source the browser closed for good (any answer
 * that is not a 200 stream: a 401, a proxy's 502 during a restart, the
 * stream cap's 503) is not taken as a session that ended. The provider asks
 * the capability probe first; only a session that is gone sends the shell
 * back to the login screen, and a live one gets a new source after a
 * backoff of 1, 2, 4 ... 30 seconds with jitter, followed by a reset,
 * since a new source cannot carry the last id it saw (controller ruling
 * C1b, 2026-09-28).
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

import { API_BASE, ApiProblem, api } from "../api/client";
import type { ChangeEvent } from "../api/events";
import { parseFrame } from "../api/events";
import { asObject, asString } from "../api/json";
import { ME_QUERY_KEY } from "../auth/keys";
import { reconnectDelay } from "./backoff";
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

/** Asks the server who this session is; the default is the capability probe. */
export type SessionProbe = () => Promise<unknown>;

function defaultProbe(): Promise<unknown> {
  return api<unknown>("/auth/me");
}

/** Who a `/auth/me` answer names: an account, the anonymous viewer, or nobody. */
function identityOf(value: unknown): string | null {
  const me = asObject(value);
  const name = asString(asObject(me?.user)?.name);
  if (name) return `user:${name}`;
  return me?.anonymous === true ? "anonymous" : null;
}

type SessionState = "valid" | "ended" | "unknown";

export function ChangeStreamProvider({
  children,
  streamFactory,
  sessionProbe,
}: {
  children: ReactNode;
  streamFactory?: StreamFactory;
  /** Test seam; the shell leaves it to the default. */
  sessionProbe?: SessionProbe;
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
    const probe = sessionProbe ?? defaultProbe;
    const pending = new Map<string, QueryKey>();
    let timer: ReturnType<typeof setTimeout> | null = null;
    let retry: ReturnType<typeof setTimeout> | null = null;
    let disposed = false;
    // Failures since the last source that opened; reset on `open` only, so a
    // stream the cap keeps refusing waits longer each time.
    let attempt = 0;
    let detach: (() => void) | null = null;

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
    const fanOut = (event: ChangeEvent) => {
      for (const listener of changeSubscribers) {
        // One listener's bug must not cost the page its own refresh, nor the
        // listeners after it their frame.
        try {
          listener(event);
        } catch (error) {
          console.error("a change listener threw", error);
        }
      }
    };
    const resetAll = () => {
      pending.clear();
      if (timer) clearTimeout(timer);
      timer = null;
      void queryClient.invalidateQueries();
    };
    const onFrame = (name: string) => (message: MessageEvent<string>) => {
      const event = parseFrame(name, message.data);
      if (!event) return;
      fanOut(event);
      const keys = keysFor(event);
      if (keys === "everything") {
        resetAll();
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

    /**
     * Whether the session behind the stream still stands. A closed source
     * is not proof that it ended: by the EventSource rules any answer that
     * is not a 200 stream closes it for good, which is also a proxy's 502
     * during a restart and the stream cap's 503. The probe answers 200 with
     * no identity for an ended cookie session, and an identity other than
     * the one the shell holds is a session that ended too.
     */
    const checkSession = async (): Promise<SessionState> => {
      try {
        const now = identityOf(await probe());
        if (now === null) return "ended";
        const held = queryClient.getQueryData(ME_QUERY_KEY);
        if (held !== undefined && identityOf(held) !== now) return "ended";
        return "valid";
      } catch (error) {
        return error instanceof ApiProblem && error.status === 401
          ? "ended"
          : "unknown";
      }
    };

    const scheduleRetry = (step: () => void) => {
      const delay = reconnectDelay(attempt);
      attempt += 1;
      retry = setTimeout(() => {
        retry = null;
        step();
      }, delay);
    };

    const recover = () => {
      void checkSession().then((state) => {
        if (disposed) return;
        if (state === "ended") {
          // The path the query client runs for a 401: the probe answers,
          // and the login screen replaces the shell and this provider.
          void queryClient.invalidateQueries({ queryKey: ME_QUERY_KEY });
        } else if (state === "valid") {
          scheduleRetry(() => {
            connect(true);
          });
        } else {
          scheduleRetry(recover);
        }
      });
    };

    const connect = (reopened: boolean) => {
      const source = open(`${API_BASE}/events`);
      const listeners = EVENT_NAMES.map(
        (name) => [name, onFrame(name)] as const,
      );
      for (const [name, listener] of listeners) {
        source.addEventListener(name, listener as EventListener);
      }
      const onOpen = () => {
        attempt = 0;
        if (reopened) {
          // A new source carries no `Last-Event-ID` (the browser sends it on
          // its own reconnects only), so what happened while it was closed
          // is covered the way the server covers a lost id: a reset.
          reopened = false;
          fanOut({ event: "reset" });
          resetAll();
        }
      };
      source.addEventListener("open", onOpen);
      source.onerror = () => {
        // CONNECTING is the browser mid-reconnect, with `Last-Event-ID`.
        if (source.readyState !== CLOSED) return;
        detach?.();
        detach = null;
        recover();
      };
      detach = () => {
        for (const [name, listener] of listeners) {
          source.removeEventListener(name, listener as EventListener);
        }
        source.removeEventListener("open", onOpen);
        source.onerror = null;
        source.close();
      };
    };

    connect(false);
    return () => {
      disposed = true;
      if (timer) clearTimeout(timer);
      if (retry) clearTimeout(retry);
      detach?.();
      detach = null;
    };
  }, [queryClient, streamFactory, sessionProbe, ignored, recent]);

  return (
    <IgnoredEngramsContext value={ignored}>
      <RecentChangesContext value={recent}>{children}</RecentChangesContext>
    </IgnoredEngramsContext>
  );
}
