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
