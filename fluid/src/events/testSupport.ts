/**
 * The fake stream the provider's tests drive, in the `FakeSocket` shape of
 * `collab/testSupport.ts`. Test-only; nothing in the app imports it.
 */

type Listener = (event: MessageEvent<string>) => void;

export class FakeEventSource {
  static instances: FakeEventSource[] = [];
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSED = 2;
  url: string;
  /** What the second constructor argument asked for. */
  withCredentials: boolean;
  readyState = 1;
  /** Whether `open` ever fired on this source. */
  opened = false;
  onerror: ((event: Event) => void) | null = null;
  private listeners = new Map<string, Set<Listener>>();
  constructor(url: string, init?: EventSourceInit) {
    this.url = url;
    this.withCredentials = init?.withCredentials ?? false;
    FakeEventSource.instances.push(this);
  }
  addEventListener(type: string, listener: Listener) {
    const set = this.listeners.get(type) ?? new Set<Listener>();
    set.add(listener);
    this.listeners.set(type, set);
  }
  removeEventListener(type: string, listener: Listener) {
    this.listeners.get(type)?.delete(listener);
  }
  close() {
    this.readyState = 2;
  }
  // test drivers
  /** The browser having connected: fires `open`. */
  open() {
    this.readyState = 1;
    this.opened = true;
    for (const listener of this.listeners.get("open") ?? []) {
      listener(new MessageEvent<string>("open"));
    }
  }
  emit(type: string, data: unknown, id?: string) {
    const event = new MessageEvent<string>(type, {
      data: JSON.stringify(data),
      lastEventId: id ?? "",
    });
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
  fail(readyState: number) {
    this.readyState = readyState;
    this.onerror?.(new Event("error"));
  }
}

/** The stream factory a provider under test is built with. */
export function fakeStreamFactory(url: string): EventSource {
  return new FakeEventSource(url) as unknown as EventSource;
}

/**
 * The tabs of one fake browser: channels of one bus hear each other, the
 * way every tab of a profile hears a `BroadcastChannel` of the same name.
 */
export class FakeChannelBus {
  readonly channels = new Set<FakeBroadcastChannel>();
}

const defaultBus = new FakeChannelBus();

/**
 * A `BroadcastChannel` that delivers on a microtask to every other open
 * channel of its name on its bus, never to itself, as the real one does.
 */
export class FakeBroadcastChannel {
  readonly name: string;
  onmessage: ((event: MessageEvent) => void) | null = null;
  closed = false;
  private readonly bus: FakeChannelBus;
  constructor(name: string, bus: FakeChannelBus = defaultBus) {
    this.name = name;
    this.bus = bus;
    bus.channels.add(this);
  }
  postMessage(message: unknown) {
    if (this.closed) throw new Error("posted on a closed channel");
    const data = structuredClone(message);
    for (const other of this.bus.channels) {
      if (other === this || other.name !== this.name) continue;
      queueMicrotask(() => {
        if (!other.closed) {
          other.onmessage?.(new MessageEvent("message", { data }));
        }
      });
    }
  }
  close() {
    this.closed = true;
    this.bus.channels.delete(this);
  }
  addEventListener() {
    throw new Error("the fake channel carries onmessage only");
  }
}

interface Waiting {
  run: () => void;
}

/**
 * Web Locks, in memory: one holder per name, the rest queued in order,
 * `ifAvailable` answered with `null` when the lock is held, a queued
 * request withdrawn by its signal. The holder keeps the lock until the
 * promise its callback returned settles.
 */
export class FakeLocks {
  private readonly held = new Set<string>();
  private readonly queues = new Map<string, Waiting[]>();

  /** Whether anybody holds `name` right now. */
  isHeld(name: string): boolean {
    return this.held.has(name);
  }

  request(
    name: string,
    options: { ifAvailable?: boolean; signal?: AbortSignal },
    callback: (lock: unknown) => Promise<void> | void,
  ): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const release = () => {
        this.held.delete(name);
        const next = this.queues.get(name)?.shift();
        next?.run();
      };
      const run = () => {
        this.held.add(name);
        void Promise.resolve()
          .then(() => callback({ name }))
          .then(
            (value) => {
              release();
              resolve(value);
            },
            (error: unknown) => {
              release();
              reject(error instanceof Error ? error : new Error(String(error)));
            },
          );
      };
      if (!this.held.has(name)) {
        run();
      } else if (options.ifAvailable) {
        void Promise.resolve()
          .then(() => callback(null))
          .then(resolve, reject);
      } else {
        const queue = this.queues.get(name) ?? [];
        const entry: Waiting = { run };
        queue.push(entry);
        this.queues.set(name, queue);
        options.signal?.addEventListener("abort", () => {
          const at = queue.indexOf(entry);
          if (at >= 0) {
            queue.splice(at, 1);
            reject(new DOMException("aborted", "AbortError"));
          }
        });
      }
    });
  }
}
