/**
 * Which tab of this browser holds the one change stream.
 *
 * Browsers give an HTTP/1.1 host six connections per profile, shared by
 * every tab, and a stream holds one for as long as it is open. So the tabs
 * of one origin elect a leader that holds the stream and passes each frame
 * on to the others. With Web Locks the browser itself keeps the election
 * honest: the lock is held while the leading tab lives and handed to the
 * next waiting tab when it closes. Without Web Locks a small heartbeat
 * protocol over a `BroadcastChannel` stands in.
 */

/** The part of the Web Locks API the election uses. */
export interface LockManagerLike {
  request(
    name: string,
    options: { ifAvailable?: boolean; signal?: AbortSignal },
    callback: (lock: unknown) => Promise<void> | void,
  ): Promise<unknown>;
}

/** The part of `BroadcastChannel` the stream uses. */
export interface ChannelLike {
  postMessage(message: unknown): void;
  onmessage: ((event: MessageEvent) => void) | null;
  close(): void;
}

export type ChannelFactory = (name: string) => ChannelLike;

/**
 * Called when this tab becomes the leader. `handover` is true when another
 * tab led before it, so what happened in between has to be covered. Returns
 * what ends the leadership; it runs before the election lets go.
 */
export type Lead = (handover: boolean) => () => void;

export interface Election {
  /** Stop taking part: end the leadership if held, stop waiting if not. */
  stop(): void;
}

/** A Web Locks election: the lock is the leadership. */
export function lockElection(
  locks: LockManagerLike,
  name: string,
  lead: Lead,
): Election {
  const abort = new AbortController();
  let stopped = false;
  let release: (() => void) | null = null;

  const hold = (handover: boolean) => {
    if (stopped) return undefined;
    const end = lead(handover);
    return new Promise<void>((resolve) => {
      release = () => {
        // The stream closes before the lock goes, so two tabs never hold
        // one at the same time.
        end();
        resolve();
      };
    });
  };

  // First the lock as it is now: free means nobody leads and there is no
  // gap to cover. Taken means another tab leads; wait in line, and when the
  // lock comes, the leader before has gone.
  void locks
    .request(name, { ifAvailable: true }, (lock) => {
      if (lock !== null) return hold(false);
      if (stopped) return undefined;
      void locks
        .request(name, { signal: abort.signal }, () => hold(true))
        .catch(() => undefined);
      return undefined;
    })
    .catch(() => undefined);

  return {
    stop() {
      stopped = true;
      const held = release;
      release = null;
      if (held) {
        held();
      } else {
        abort.abort();
      }
    },
  };
}

/** How often a leader says it is there, in the fallback. */
export const HEARTBEAT_MS = 1_000;
/** How long a silent leader is waited for before a tab claims its place. */
export const LEADER_TIMEOUT_MS = 3_500;
/** How long a claim waits for an objection. */
export const CLAIM_MS = 150;

type ElectionMessage =
  | { type: "heartbeat"; id: string }
  | { type: "claim"; id: string }
  | { type: "resign"; id: string };

function readMessage(value: unknown): ElectionMessage | null {
  const message = value as Partial<ElectionMessage> | null;
  return typeof message?.id === "string" && typeof message.type === "string"
    ? (message as ElectionMessage)
    : null;
}

/**
 * The fallback for a browser without Web Locks. A leader posts a heartbeat
 * every second and answers a claim at once; a tab that hears no heartbeat
 * for a while, or a resignation, claims the place, and of two claims the
 * lower id wins. Two leaders that hear each other settle the same way.
 */
export function channelElection(
  channel: ChannelLike,
  lead: Lead,
  random: () => number = Math.random,
): Election {
  const id = random().toString(36).slice(2).padEnd(12, "0");
  let state: "claiming" | "following" | "leading" = "claiming";
  let end: (() => void) | null = null;
  let sawLeader = false;
  let claimTimer: ReturnType<typeof setTimeout> | null = null;
  let watchdog: ReturnType<typeof setTimeout> | null = null;
  let heartbeat: ReturnType<typeof setInterval> | null = null;

  const post = (type: ElectionMessage["type"]) => {
    channel.postMessage({ type, id });
  };
  const clearTimers = () => {
    if (claimTimer) clearTimeout(claimTimer);
    if (watchdog) clearTimeout(watchdog);
    if (heartbeat) clearInterval(heartbeat);
    claimTimer = watchdog = heartbeat = null;
  };
  const follow = () => {
    state = "following";
    sawLeader = true;
    clearTimers();
    watchdog = setTimeout(claim, LEADER_TIMEOUT_MS);
  };
  const becomeLeader = () => {
    clearTimers();
    state = "leading";
    end = lead(sawLeader);
    post("heartbeat");
    heartbeat = setInterval(() => {
      post("heartbeat");
    }, HEARTBEAT_MS);
  };
  const stepDown = () => {
    end?.();
    end = null;
  };
  function claim() {
    clearTimers();
    state = "claiming";
    post("claim");
    claimTimer = setTimeout(becomeLeader, CLAIM_MS);
  }

  channel.onmessage = (event: MessageEvent) => {
    const message = readMessage(event.data);
    if (!message || message.id === id) return;
    switch (message.type) {
      case "heartbeat":
        if (state === "leading") {
          if (message.id < id) {
            stepDown();
            follow();
          }
        } else {
          follow();
        }
        break;
      case "claim":
        if (state === "leading") {
          post("heartbeat");
        } else if (state === "claiming" && message.id < id) {
          // The lower id takes it; wait for its heartbeat.
          sawLeader = true;
          clearTimers();
          state = "following";
          watchdog = setTimeout(claim, LEADER_TIMEOUT_MS);
        }
        break;
      case "resign":
        if (state !== "leading") {
          sawLeader = true;
          claim();
        }
        break;
    }
  };
  claim();

  return {
    stop() {
      clearTimers();
      if (state === "leading") {
        stepDown();
        post("resign");
      }
      state = "following";
      channel.onmessage = null;
      channel.close();
    },
  };
}
