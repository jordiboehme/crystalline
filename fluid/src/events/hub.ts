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
 * - Every frame names the account the leader's stream was opened as: the
 *   cookie's when the stream's request went out, never the identity the
 *   leader's own tab shows, which can be stale after a sign-in in another
 *   tab. The server does not name it, so the leader asks the probe, which
 *   carries the same cookie, once before each request of the stream (a new
 *   source, or the browser's own reopen) and once after it opened. The two
 *   answers naming the same account is the label. Two answers that name
 *   different accounts, or an account and nobody, disagree: the cookie
 *   changed around the request, and the source is opened again. After
 *   `MAX_REOPENS` reopens in a row the next disagreement makes the lead
 *   start over, after a backoff that grows with each such start until a
 *   label holds. A probe that could
 *   not answer (a network error, a 5xx) is no disagreement: it is asked
 *   again, and when it was the answer before the browser's own reopen,
 *   which cannot be asked again before that request, the source is opened
 *   once more with the answer after it, without counting toward the
 *   bound. Until there is a label no frame is passed on, not even in the
 *   leader's tab; once there is, one reset stands in for whatever was
 *   dropped. What the bracket cannot see is a cookie that changes and
 *   changes back between its two answers; the server re-checks each
 *   stream's own session every 10 seconds (`VISIBILITY_TTL`) and ends one
 *   whose session is gone, which bounds that case.
 * - A tab whose shell shows somebody else than a frame's label drops it
 *   and asks the probe which of the two is stale: a stale tab re-asks who
 *   is signed in and resets, a stale leader re-checks its session and
 *   reopens as the cookie's account, with a reset. Another account is
 *   never taken as a session that ended.
 *
 * The carrier: whether frames flow. The leader reports it down on any
 * `error` of its source (the browser's own reconnect included, since
 * nothing arrives while it runs) and up when a source opens, and tells the
 * other tabs over the channel, so every tab's consumers hear one change
 * per drop and one per return. A fresh hub starts with the carrier up, so
 * the first `open` is no change. Up is what a consumer takes for granted:
 * one that attaches while the carrier is down hears the drop at once, and
 * one that attaches while it is up hears nothing until it changes.
 *
 * A tab takes part while anything in it wants the stream: a mounted
 * provider, a consumer attached from outside the shell, or a `subscribe`
 * listener. It leaves when the last one goes. Attaching any consumer, with
 * an identity or only a carrier callback, counts as the shell mounting: it
 * takes a tab whose session was found ended back into the running. A
 * consumer attached after the session ended only wakes a stream that
 * answers 401 again, which is harmless: `RequireAuth` has already sent the
 * route to the login screen.
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

/**
 * What a mounted provider hands the hub, and what a subscriber outside the
 * shell that passes an identity is registered as.
 */
export interface ChangeConsumer {
  /** Every frame, in order, the resets included. */
  onEvent(event: ChangeEvent): void;
  /**
   * Re-ask who is signed in: the session behind the stream ended, or it
   * names somebody other than the one this tab shows.
   */
  recheckIdentity(): void;
  /**
   * The capability answer this tab holds: who its shell says is signed in.
   * `undefined` is "no shell": the consumer filters nothing.
   */
  heldIdentity(): unknown;
  /** The stream went down (`false`) or came back (`true`). */
  onCarrier?(up: boolean): void;
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
 * right account; `carrier` passes on whether the leader's stream is up.
 */
type Message =
  | { type: "frame"; event: ChangeEvent; account: string | null }
  | { type: "ended" }
  | { type: "recheck" }
  | { type: "carrier"; up: boolean };

/**
 * A message from another tab of this app. Its frames were read by
 * `parseFrame` there, so they are taken as they came.
 */
function readMessage(value: unknown): Message | null {
  const message = asObject(value);
  const type = message?.type;
  if (type === "carrier") {
    return typeof message?.up === "boolean" ? { type, up: message.up } : null;
  }
  return type === "frame" || type === "ended" || type === "recheck"
    ? (value as Message)
    : null;
}

/** What the capability probe said about the cookie this browser holds now. */
type Probed =
  { kind: "is"; identity: string } | { kind: "ended" } | { kind: "unknown" };

/**
 * How often in a row the leader closes and reopens a source whose request
 * the probe's two answers name different accounts for, before it reports
 * the carrier down and starts its lead over after the stream's backoff.
 */
export const MAX_REOPENS = 3;

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
  /**
   * While this tab leads: the account its stream was opened as, as the
   * probe last named it.
   */
  private account: string | null = null;
  /**
   * Whether `account` names the stream open now. Until it does, no frame is
   * passed on: a frame labelled with a guess could reach a tab that shows
   * somebody the frame was not streamed for.
   */
  private accountKnown = false;
  /** A frame was dropped while the account was not known. */
  private missed = false;
  /** While this tab leads: re-check the session and reconnect if it moved. */
  private leaderRecheck: (() => void) | null = null;
  private checking = false;
  private quietUntil = 0;
  /** Whether frames flow; see the module doc. */
  private carrierUp = true;

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
    // Up is what a consumer takes for granted; one that attaches during a
    // drop hears it at once, or it would never hear the drop it came in.
    if (!this.carrierUp) this.tell(consumer, false);
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
    // A tab out of the running has no stream to be down.
    this.carrierUp = true;
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
    } else if (message.type === "carrier") {
      this.setCarrier(message.up, false);
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

  /**
   * The stream went down or came back: tell this tab's consumers, and with
   * `broadcast` the other tabs too. Only a change is passed on.
   */
  private setCarrier(up: boolean, broadcast: boolean): void {
    if (up === this.carrierUp) return;
    this.carrierUp = up;
    if (broadcast) this.channel?.postMessage({ type: "carrier", up });
    for (const consumer of [...this.consumers]) this.tell(consumer, up);
  }

  /**
   * One consumer's carrier news. A consumer's bug must not stop the
   * reconnect or the reset that follow, nor cost the others their news.
   */
  private tell(consumer: ChangeConsumer, up: boolean): void {
    try {
      consumer.onCarrier?.(up);
    } catch (error) {
      console.error("a change consumer threw", error);
    }
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
    if (own !== undefined && own !== account) {
      this.mismatch(account);
      return;
    }
    this.fanOut(event);
  }

  private fanOut(event: ChangeEvent): void {
    // One listener's or consumer's bug must not cost the page its own
    // refresh, nor the others after it their frame.
    for (const listener of [...this.listeners]) {
      try {
        listener(event);
      } catch (error) {
        console.error("a change listener threw", error);
      }
    }
    for (const consumer of [...this.consumers]) {
      try {
        consumer.onEvent(event);
      } catch (error) {
        console.error("a change consumer threw", error);
      }
    }
  }

  /** A frame this tab read off its own source: here and in every other tab. */
  private broadcast(event: ChangeEvent): void {
    if (!this.accountKnown) {
      // Once the account is known, one reset stands in for what was
      // dropped here.
      this.missed = true;
      return;
    }
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
    this.account = null;
    this.accountKnown = false;
    this.missed = false;

    // The account a stream is opened as is the cookie's when its request
    // went out. The probe carries the same cookie, so the leader brackets
    // each request of the stream (every new source, and every reopen the
    // browser makes on its own) between two answers: one asked before the
    // request, one asked after the source opened. Only the order in which
    // they were asked matters, not the order they come back in. When the
    // two name the same account, that is the label; when they name
    // different ones, the cookie changed around the request, and the source
    // is closed and opened again (see the module doc for the bound and for
    // a probe that could not answer).
    let asking = 0;
    let before: string | null | undefined;
    let beforeUnknown = false;
    let after: string | undefined;
    let afterAsked = false;
    let askAttempt = 0;
    let reopens = 0;
    /** Fresh starts after the bound since the last label: their backoff. */
    let restarts = 0;
    let askRetry: ReturnType<typeof setTimeout> | null = null;
    const forgetAccount = () => {
      asking += 1;
      this.accountKnown = false;
      before = undefined;
      beforeUnknown = false;
      after = undefined;
      afterAsked = false;
      if (askRetry) clearTimeout(askRetry);
      askRetry = null;
    };
    /** Close this source and open another, whose "before" is `next`. */
    const reopen = (next: string, delay: number) => {
      detach?.();
      detach = null;
      this.setCarrier(false, true);
      if (retry) clearTimeout(retry);
      retry = setTimeout(() => {
        retry = null;
        connect(true, next);
      }, delay);
    };
    const settleBracket = () => {
      if (after === undefined) return;
      if (beforeUnknown) {
        // The browser's own request went out with nobody asked before it.
        // The answer after it was asked before the next request, so that
        // one is bracketed; this is no disagreement.
        reopen(after, reconnectDelay(0, this.deps.random));
        return;
      }
      if (before === undefined) return;
      if (before === after) {
        reopens = 0;
        restarts = 0;
        this.account = after;
        this.accountKnown = true;
        const own = this.ownIdentity();
        if (own !== undefined && own !== after) {
          // This tab shows somebody the stream is not for.
          for (const consumer of [...this.consumers]) {
            consumer.recheckIdentity();
          }
        }
        if (this.missed) {
          this.missed = false;
          this.broadcast({ event: "reset" });
        }
        return;
      }
      if (reopens >= MAX_REOPENS) {
        // The cookie does not hold still: no frame flows, so say so, and
        // start over after the stream's backoff.
        reopens = 0;
        detach?.();
        detach = null;
        this.setCarrier(false, true);
        if (retry) clearTimeout(retry);
        // Its own count: every open clears `attempt`, and this wait must
        // grow while the cookie keeps moving.
        const delay = reconnectDelay(restarts, this.deps.random);
        restarts += 1;
        retry = setTimeout(() => {
          retry = null;
          begin(true);
        }, delay);
        return;
      }
      // The answer that closed this bracket was asked before the next
      // request, so it opens the next one.
      const delay = reconnectDelay(reopens, this.deps.random);
      reopens += 1;
      reopen(after, delay);
    };
    const askBefore = () => {
      const generation = asking;
      void this.probeIdentity().then((probed) => {
        if (ended || generation !== asking) return;
        if (probed.kind === "unknown") beforeUnknown = true;
        else before = probed.kind === "is" ? probed.identity : null;
        settleBracket();
      });
    };
    const askAfter = () => {
      afterAsked = true;
      const generation = asking;
      void this.probeIdentity().then((probed) => {
        if (ended || generation !== asking) return;
        if (probed.kind === "is") {
          askAttempt = 0;
          after = probed.identity;
          settleBracket();
          return;
        }
        // An ended session is the stream's own to find out: the server
        // ends it, and its reopen is refused. Until then, ask again.
        const delay = reconnectDelay(askAttempt, this.deps.random);
        askAttempt += 1;
        askRetry = setTimeout(() => {
          askRetry = null;
          askAfter();
        }, delay);
      });
    };

    /**
     * Whether the session still stands, and as whom. Another account than
     * the stream was opened as is never an ended session: the cookie now
     * holds a live one, so the stream reconnects as it.
     */
    const checkSession = async (): Promise<
      | { state: "valid"; identity: string }
      | { state: "changed"; identity: string }
      | { state: "ended" }
      | { state: "unknown" }
    > => {
      const probed = await this.probeIdentity();
      if (probed.kind === "ended") return { state: "ended" };
      if (probed.kind === "unknown") return { state: "unknown" };
      const identity = probed.identity;
      const was = this.account;
      // Learning the account of a stream nobody could name is no change.
      if (was === null || was === identity) return { state: "valid", identity };
      if (this.ownIdentity() !== identity) {
        for (const consumer of [...this.consumers]) consumer.recheckIdentity();
      }
      return { state: "changed", identity };
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
      void checkSession().then((checked) => {
        if (ended) return;
        if (checked.state === "ended") {
          this.sessionEnded(true);
        } else if (checked.state === "unknown") {
          scheduleRetry(recover);
        } else {
          const { identity } = checked;
          scheduleRetry(() => {
            connect(true, identity);
          });
        }
      });
    };

    /**
     * The first request of a lead goes out once its "before" answer is in.
     * An answer that names nobody still opens it, and the stream's own
     * refusal then says whether the session ended; a probe that could not
     * answer is asked again.
     */
    const begin = (gap: boolean) => {
      forgetAccount();
      const generation = asking;
      void this.probeIdentity().then((probed) => {
        if (ended || generation !== asking) return;
        if (probed.kind === "unknown") {
          const delay = reconnectDelay(askAttempt, this.deps.random);
          askAttempt += 1;
          askRetry = setTimeout(() => {
            askRetry = null;
            begin(gap);
          }, delay);
          return;
        }
        askAttempt = 0;
        connect(gap, probed.kind === "is" ? probed.identity : null);
      });
    };

    /** A new source, whose request goes out after `known` was answered. */
    const connect = (gap: boolean, known: string | null) => {
      forgetAccount();
      before = known;
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
        if (!afterAsked) askAfter();
        // With no answer before this request the leader will close this
        // source once the answer after it is in: no frame flows on it.
        if (!beforeUnknown) this.setCarrier(true, true);
        if (gap) {
          // Nothing said what happened while no source was open.
          gap = false;
          this.broadcast({ event: "reset" });
        }
      };
      source.addEventListener("open", onOpen);
      source.onerror = () => {
        forgetAccount();
        // Down either way: nothing arrives while the browser reconnects.
        this.setCarrier(false, true);
        if (source.readyState !== CLOSED) {
          // CONNECTING is the browser mid-reconnect, with `Last-Event-ID`,
          // and with whatever cookie it holds then: its request is
          // bracketed like a new one, from here to its `open`.
          askBefore();
          return;
        }
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
      void checkSession().then((checked) => {
        if (ended) return;
        if (checked.state === "ended") {
          this.sessionEnded(true);
        } else if (checked.state === "changed") {
          detach?.();
          detach = null;
          this.setCarrier(false, true);
          if (retry) clearTimeout(retry);
          retry = null;
          connect(true, checked.identity);
        }
      });
    };

    begin(handover);
    return () => {
      ended = true;
      forgetAccount();
      this.account = null;
      this.missed = false;
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
