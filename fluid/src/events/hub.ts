/**
 * The one change stream of this browser, and this tab's place in it.
 *
 * Every tab of one profile and origin shares a single `EventSource`: a
 * browser gives an HTTP/1.1 host six connections, shared by all its tabs,
 * and a stream per tab would leave the seventh request of the whole
 * browser waiting forever once six tabs were open. So one tab leads (see
 * `election.ts`): it holds the source, reads each frame and passes it on
 * over a `BroadcastChannel`, and every other tab reads the frames from
 * there. What a frame invalidates is decided in each tab by its own
 * provider, against its own query cache.
 *
 * The leader handles what the browser does not:
 * - A dropped connection is the browser's own to reconnect; it sends
 *   `Last-Event-ID` itself.
 * - A source the browser closed for good (any answer that is not a 200
 *   stream: a 401, a proxy's 502 during a restart, the stream cap's 503)
 *   is not taken as a session that ended. The leader asks the capability
 *   probe first. A session that is gone sends every tab back to its login
 *   screen; a live one gets a new source after a backoff of 1, 2, 4 ... 30
 *   seconds with jitter.
 * - A new source cannot carry the last id the old one saw, so when a
 *   reopened source, or the source of a tab that took over from a closed
 *   leader, opens, every tab resets once: the pages refetch what they show.
 *
 * - Every frame names the account the leader's stream was opened as. A tab
 *   whose shell shows somebody else drops it and asks the probe which of
 *   the two is stale: a stale tab re-asks who is signed in and resets, a
 *   stale leader re-checks its session and reopens as the cookie's account,
 *   with a reset. Another account is never taken as a session that ended.
 *
 * A tab takes part while anything in it wants the stream: a mounted
 * provider or a `subscribe` listener. It leaves when the last one goes.
 */

import { API_BASE, ApiProblem, api } from "../api/client";
import type { ChangeEvent } from "../api/events";
import { parseFrame } from "../api/events";
import { asObject, asString } from "../api/json";
import { reconnectDelay } from "./backoff";
import type {
  ChannelFactory,
  ChannelLike,
  Election,
  LockManagerLike,
} from "./election";
import { channelElection, lockElection } from "./election";

/** The name of the lock and of the channel the tabs share. */
export const STREAM_NAME = "crystalline-change-stream";

/**
 * `EventSource.CLOSED`, spelled here because a runtime without the global
 * has no constructor to read it off.
 */
const CLOSED = 2;

const EVENT_NAMES = ["engram", "domain", "reset"] as const;

export type StreamFactory = (url: string) => EventSource;

/** Asks the server who this session is; the default is the capability probe. */
export type SessionProbe = () => Promise<unknown>;

/** A callback for every frame the one stream carries, whatever it invalidates. */
export type ChangeListener = (event: ChangeEvent) => void;

/** What a mounted provider hands the hub. */
export interface ChangeConsumer {
  /** Every frame, in order, the resets included. */
  onEvent(event: ChangeEvent): void;
  /**
   * Re-ask who is signed in: the session behind the stream ended, or it
   * names somebody other than the one this tab shows.
   */
  recheckIdentity(): void;
  /** The capability answer this tab holds: who its shell says is signed in. */
  heldIdentity(): unknown;
}

/**
 * Where the hub reaches the browser. Every field defaults to the global it
 * names, read when it is needed rather than when the hub is built, so a
 * test can stub a global after the module loaded. `null` means "this
 * browser has none".
 */
export interface HubDeps {
  streamFactory?: StreamFactory | null;
  sessionProbe?: SessionProbe;
  locks?: LockManagerLike | null;
  channel?: ChannelFactory | null;
  random?: () => number;
}

/**
 * What the tabs say to each other. A frame names the account the leader's
 * stream was opened as, so a tab signed in as somebody else never takes
 * it; `recheck` asks the leader to find out whether it still streams as the
 * right account.
 */
type Message =
  | { type: "frame"; event: ChangeEvent; account: string | null }
  | { type: "ended" }
  | { type: "recheck" };

/**
 * A message from another tab of this app. Its frames were read by
 * `parseFrame` there, so they are taken as they came.
 */
function readMessage(value: unknown): Message | null {
  const type = asObject(value)?.type;
  return type === "frame" || type === "ended" || type === "recheck"
    ? (value as Message)
    : null;
}

/** What the capability probe said about the cookie this browser holds now. */
type Probed =
  { kind: "is"; identity: string } | { kind: "ended" } | { kind: "unknown" };

/** How long a tab that found a mismatch waits before it looks again. */
export const RECHECK_QUIET_MS = 2_000;

/** Who a `/auth/me` answer names: an account, the anonymous viewer, or nobody. */
function identityOf(value: unknown): string | null {
  const me = asObject(value);
  const name = asString(asObject(me?.user)?.name);
  if (name) return `user:${name}`;
  return me?.anonymous === true ? "anonymous" : null;
}

function defaultStreamFactory(): StreamFactory | null {
  if (typeof EventSource === "undefined") return null;
  return (url) => new EventSource(url, { withCredentials: true });
}

function defaultLocks(): LockManagerLike | null {
  return (navigator as { locks?: LockManagerLike }).locks ?? null;
}

function defaultChannel(): ChannelFactory | null {
  if (typeof BroadcastChannel === "undefined") return null;
  return (name) => new BroadcastChannel(name);
}

export class ChangeHub {
  private readonly deps: HubDeps;
  private readonly listeners = new Set<ChangeListener>();
  private readonly consumers = new Set<ChangeConsumer>();
  private channel: ChannelLike | null = null;
  private election: Election | null = null;
  /**
   * Set once this tab's session was found ended: it stops taking part
   * until a provider attaches again (the shell after a new sign-in).
   */
  private dormant = false;
  /** While this tab leads: the account its stream was opened as. */
  private account: string | null = null;
  /** While this tab leads: re-check the session and reconnect if it moved. */
  private leaderRecheck: (() => void) | null = null;
  private checking = false;
  private quietUntil = 0;

  constructor(deps: HubDeps = {}) {
    this.deps = deps;
  }

  /** Listen to every frame. Starts this tab's part in the stream if needed. */
  subscribe(listener: ChangeListener): () => void {
    this.listeners.add(listener);
    this.demandChanged();
    return () => {
      this.listeners.delete(listener);
      this.demandChanged();
    };
  }

  /** A provider's registration; see `ChangeConsumer`. */
  attach(consumer: ChangeConsumer): () => void {
    this.consumers.add(consumer);
    // A provider attaching is the shell mounting, which after an ended
    // session is a new sign-in: take part again.
    this.dormant = false;
    this.demandChanged();
    return () => {
      this.consumers.delete(consumer);
      this.demandChanged();
    };
  }

  private get wanted(): boolean {
    return this.listeners.size + this.consumers.size > 0;
  }

  private demandChanged(): void {
    if (!this.wanted) {
      this.dormant = false;
      this.stop();
    } else if (!this.election && !this.dormant) {
      this.start();
    }
  }

  private start(): void {
    const channelFactory =
      this.deps.channel === undefined ? defaultChannel() : this.deps.channel;
    const locks =
      this.deps.locks === undefined ? defaultLocks() : this.deps.locks;
    const channel = channelFactory ? channelFactory(STREAM_NAME) : null;
    this.channel = channel;
    if (channel) {
      channel.onmessage = (event: MessageEvent) => {
        this.receive(event.data);
      };
    }
    const lead = (handover: boolean) => this.lead(handover);
    if (locks) {
      this.election = lockElection(locks, STREAM_NAME, lead);
    } else if (channelFactory) {
      this.election = channelElection(
        channelFactory(`${STREAM_NAME}:election`),
        lead,
        this.deps.random,
      );
    } else {
      // Neither: no other tab can hear this one, so it leads alone, as a
      // tab of a browser without either API always did.
      const end = lead(false);
      this.election = { stop: end };
    }
  }

  private stop(): void {
    this.election?.stop();
    this.election = null;
    if (this.channel) {
      this.channel.onmessage = null;
      this.channel.close();
      this.channel = null;
    }
  }

  /** A message from another tab. */
  private receive(value: unknown): void {
    const message = readMessage(value);
    if (!message) return;
    if (message.type === "ended") {
      this.sessionEnded(false);
    } else if (message.type === "recheck") {
      this.leaderRecheck?.();
    } else {
      this.deliver(message.event, message.account);
    }
  }

  /** Who this tab's shell says is signed in; `undefined` with no shell. */
  private ownIdentity(): string | null | undefined {
    for (const consumer of this.consumers) {
      const held = consumer.heldIdentity();
      if (held !== undefined) return identityOf(held);
    }
    return undefined;
  }

  private probeIdentity(): Promise<Probed> {
    const probe = this.deps.sessionProbe ?? (() => api<unknown>("/auth/me"));
    return probe().then(
      (answer): Probed => {
        const identity = identityOf(answer);
        return identity === null ? { kind: "ended" } : { kind: "is", identity };
      },
      (error: unknown): Probed =>
        error instanceof ApiProblem && error.status === 401
          ? { kind: "ended" }
          : { kind: "unknown" },
    );
  }

  /**
   * A frame streamed as another account than this tab shows. One of the
   * two is stale: the probe says which. A stale tab re-asks who is signed
   * in and resets, since the frames it dropped are lost to it; a stale
   * leader is asked to re-check its session and reconnect.
   */
  private mismatch(account: string | null): void {
    if (this.checking || Date.now() < this.quietUntil) return;
    this.checking = true;
    void this.probeIdentity().then((probed) => {
      this.checking = false;
      this.quietUntil = Date.now() + RECHECK_QUIET_MS;
      if (probed.kind === "unknown") return;
      const now = probed.kind === "is" ? probed.identity : null;
      if (now !== this.ownIdentity()) {
        for (const consumer of [...this.consumers]) consumer.recheckIdentity();
        this.fanOut({ event: "reset" });
      }
      if (now !== null && now !== account) {
        if (this.leaderRecheck) this.leaderRecheck();
        else this.channel?.postMessage({ type: "recheck" });
      }
    });
  }

  /**
   * Hand one frame to everything in this tab that listens, unless it was
   * streamed as somebody other than the account this tab shows.
   */
  private deliver(event: ChangeEvent, account: string | null): void {
    const own = this.ownIdentity();
    if (own !== undefined && account !== null && own !== account) {
      this.mismatch(account);
      return;
    }
    this.fanOut(event);
  }

  private fanOut(event: ChangeEvent): void {
    for (const listener of [...this.listeners]) {
      // One listener's bug must not cost the page its own refresh, nor the
      // listeners after it their frame.
      try {
        listener(event);
      } catch (error) {
        console.error("a change listener threw", error);
      }
    }
    for (const consumer of [...this.consumers]) {
      consumer.onEvent(event);
    }
  }

  /** A frame this tab read off its own source: here and in every other tab. */
  private broadcast(event: ChangeEvent): void {
    const account = this.account;
    this.channel?.postMessage({ type: "frame", event, account });
    this.deliver(event, account);
  }

  private sessionEnded(announce: boolean): void {
    if (announce) this.channel?.postMessage({ type: "ended" });
    // Out of the running, so the other tabs, whose session is the same
    // cookie, do not take the lock one after another only to hear a 401.
    this.dormant = true;
    this.stop();
    for (const consumer of [...this.consumers]) {
      consumer.recheckIdentity();
    }
  }

  /** This tab leads: hold the source until the returned function runs. */
  private lead(handover: boolean): () => void {
    const factory =
      this.deps.streamFactory === undefined
        ? defaultStreamFactory()
        : this.deps.streamFactory;
    if (!factory) {
      // A runtime with no `EventSource` (the test harness's jsdom without a
      // stub) leads inert rather than throwing.
      return () => undefined;
    }
    let ended = false;
    let attempt = 0;
    let retry: ReturnType<typeof setTimeout> | null = null;
    let detach: (() => void) | null = null;
    // The account the stream opens as is the one this tab shows; a tab with
    // no shell asks.
    this.account = this.ownIdentity() ?? null;
    if (this.consumers.size === 0) {
      void this.probeIdentity().then((probed) => {
        if (!ended && this.account === null && probed.kind === "is") {
          this.account = probed.identity;
        }
      });
    }

    /**
     * Whether the session still stands, and as whom. Another account than
     * the stream was opened as is never an ended session: the cookie now
     * holds a live one, so the stream reconnects as it.
     */
    const checkSession = async (): Promise<
      "valid" | "changed" | "ended" | "unknown"
    > => {
      const probed = await this.probeIdentity();
      if (probed.kind !== "is") return probed.kind;
      const was = this.account;
      this.account = probed.identity;
      // Learning the account of a stream nobody could name is no change.
      if (was === null || was === probed.identity) return "valid";
      if (this.ownIdentity() !== probed.identity) {
        for (const consumer of [...this.consumers]) consumer.recheckIdentity();
      }
      return "changed";
    };

    const scheduleRetry = (step: () => void) => {
      const delay = reconnectDelay(attempt, this.deps.random);
      attempt += 1;
      retry = setTimeout(() => {
        retry = null;
        step();
      }, delay);
    };

    const recover = () => {
      void checkSession().then((state) => {
        if (ended) return;
        if (state === "ended") {
          this.sessionEnded(true);
        } else if (state === "valid" || state === "changed") {
          scheduleRetry(() => {
            connect(true);
          });
        } else {
          scheduleRetry(recover);
        }
      });
    };

    const connect = (gap: boolean) => {
      const source = factory(`${API_BASE}/events`);
      const frames = EVENT_NAMES.map(
        (name) =>
          [
            name,
            (message: MessageEvent<string>) => {
              const event = parseFrame(name, message.data);
              if (event) this.broadcast(event);
            },
          ] as const,
      );
      for (const [name, listener] of frames) {
        source.addEventListener(name, listener as EventListener);
      }
      const onOpen = () => {
        attempt = 0;
        if (gap) {
          // Nothing said what happened while no source was open.
          gap = false;
          this.broadcast({ event: "reset" });
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
        for (const [name, listener] of frames) {
          source.removeEventListener(name, listener as EventListener);
        }
        source.removeEventListener("open", onOpen);
        source.onerror = null;
        source.close();
      };
    };

    // Another tab saw frames streamed as an account its shell does not
    // show and found this tab the stale one: if the session moved, the
    // stream reopens as it, with a reset.
    this.leaderRecheck = () => {
      void checkSession().then((state) => {
        if (ended) return;
        if (state === "ended") {
          this.sessionEnded(true);
        } else if (state === "changed") {
          detach?.();
          detach = null;
          if (retry) clearTimeout(retry);
          retry = null;
          connect(true);
        }
      });
    };

    connect(handover);
    return () => {
      ended = true;
      this.account = null;
      this.leaderRecheck = null;
      if (retry) clearTimeout(retry);
      detach?.();
      detach = null;
    };
  }
}

let shared: ChangeHub | null = null;

/** This tab's hub, the one the shell and `subscribeToChanges` use. */
export function defaultHub(): ChangeHub {
  shared ??= new ChangeHub();
  return shared;
}
