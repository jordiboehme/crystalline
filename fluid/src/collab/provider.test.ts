import { describe, expect, it, vi } from "vitest";
import * as Y from "yjs";
import { Awareness, encodeAwarenessUpdate } from "y-protocols/awareness";
import * as syncProtocol from "y-protocols/sync";
import * as encoding from "lib0/encoding";
import * as decoding from "lib0/decoding";

import {
  CollabProvider,
  MESSAGE_CONTROL,
  MESSAGE_SYNC,
  collabSocketUrl,
  collabUrl,
} from "./provider";

class FakeSocket {
  static instances: FakeSocket[] = [];
  url: string;
  binaryType = "";
  readyState = 0; // CONNECTING
  sent: Uint8Array[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: ArrayBuffer }) => void) | null = null;
  onclose: ((event: { code: number }) => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(url: string) {
    this.url = url;
    FakeSocket.instances.push(this);
  }
  send(data: Uint8Array) {
    this.sent.push(new Uint8Array(data));
  }
  close() {
    this.readyState = 3;
    this.onclose?.({ code: 1000 });
  }
  // test drivers
  open() {
    this.readyState = 1;
    this.onopen?.();
  }
  receive(bytes: Uint8Array) {
    const data = bytes.buffer.slice(
      bytes.byteOffset,
      bytes.byteOffset + bytes.byteLength,
    );
    this.onmessage?.({ data: data as ArrayBuffer });
  }
  dropWith(code: number) {
    this.readyState = 3;
    this.onclose?.({ code });
  }
}

function factory(url: string): WebSocket {
  return new FakeSocket(url) as unknown as WebSocket;
}

function makeProvider(
  handlers: Partial<import("./provider").ProviderHandlers> = {},
) {
  const doc = new Y.Doc();
  const awareness = new Awareness(doc);
  const all = {
    onControl: handlers.onControl ?? (() => undefined),
    onStatus: handlers.onStatus ?? (() => undefined),
    onSynced: handlers.onSynced ?? (() => undefined),
    onEpochGap: handlers.onEpochGap ?? (() => undefined),
  };
  FakeSocket.instances = [];
  const provider = new CollabProvider(
    "/ws-under-test",
    doc,
    awareness,
    all,
    factory,
  );
  const socket = FakeSocket.instances[0];
  if (!socket) {
    throw new Error("the provider opened no socket");
  }
  return { doc, awareness, provider, socket };
}

/** A server frame: the yrs side is byte-compatible with y-protocols. */
function serverStep1(serverDoc: Y.Doc): Uint8Array {
  const encoder = encoding.createEncoder();
  encoding.writeVarUint(encoder, MESSAGE_SYNC);
  syncProtocol.writeSyncStep1(encoder, serverDoc);
  return encoding.toUint8Array(encoder);
}

function controlFrame(control: object): Uint8Array {
  const encoder = encoding.createEncoder();
  encoding.writeVarUint(encoder, MESSAGE_CONTROL);
  encoding.writeVarUint8Array(
    encoder,
    new TextEncoder().encode(JSON.stringify(control)),
  );
  return encoding.toUint8Array(encoder);
}

function helloFrame(epoch: string): Uint8Array {
  return controlFrame({
    kind: "hello",
    epoch,
    separator: "\n",
    checksum: "c1",
    permalink: "alpha",
    save_state: "ok",
  });
}

/** Complete the epoch handshake the way the server's greeting does. */
function accept(socket: FakeSocket, epoch = "e1"): void {
  socket.receive(helloFrame(epoch));
}

function concat(...frames: Uint8Array[]): Uint8Array {
  const total = frames.reduce((sum, frame) => sum + frame.length, 0);
  const out = new Uint8Array(total);
  let at = 0;
  for (const frame of frames) {
    out.set(frame, at);
    at += frame.length;
  }
  return out;
}

describe("collabUrl", () => {
  it("keeps permalink slashes and encodes segments", () => {
    expect(collabUrl("eng", "notes/deep/gamma")).toBe(
      "/api/v1/collab/eng/notes/deep/gamma",
    );
    expect(collabUrl("a b", "x%y")).toBe("/api/v1/collab/a%20b/x%25y");
  });
});

describe("collabSocketUrl", () => {
  it("swaps the page scheme for the socket one and keeps the host", () => {
    expect(
      collabSocketUrl("/api/v1/collab/eng/alpha", {
        protocol: "http:",
        host: "127.0.0.1:7411",
      }),
    ).toBe("ws://127.0.0.1:7411/api/v1/collab/eng/alpha");
    expect(
      collabSocketUrl("/api/v1/collab/eng/alpha", {
        protocol: "https:",
        host: "knowledge.example",
      }),
    ).toBe("wss://knowledge.example/api/v1/collab/eng/alpha");
  });

  it("leaves an already absolute socket URL alone", () => {
    expect(
      collabSocketUrl("ws://elsewhere/api", {
        protocol: "https:",
        host: "knowledge.example",
      }),
    ).toBe("ws://elsewhere/api");
  });
});

describe("CollabProvider", () => {
  it("stays silent until the hello, then greets with SyncStep1 and syncs", () => {
    const synced = vi.fn();
    const { doc, provider, socket } = makeProvider({ onSynced: synced });
    socket.open();
    // NOTHING is sent on open: y-sync waits for the epoch handshake, or a
    // reconnect could pour an old doc into a fresh server session.
    expect(socket.sent).toHaveLength(0);
    accept(socket);
    // The first sent frame is our SyncStep1.
    const first = socket.sent[0];
    if (!first) throw new Error("nothing sent after the hello");
    expect(first[0]).toBe(MESSAGE_SYNC);

    // A "server": a doc holding the session text, answering our step1.
    const serverDoc = new Y.Doc();
    serverDoc.getText("content").insert(0, "hello from the server");
    const decoder = decoding.createDecoder(first);
    decoding.readVarUint(decoder); // message type
    const reply = encoding.createEncoder();
    encoding.writeVarUint(reply, MESSAGE_SYNC);
    syncProtocol.readSyncMessage(decoder, reply, serverDoc, "test");
    socket.receive(encoding.toUint8Array(reply)); // SyncStep2 back to us

    // `toJSON` rather than `toString`: yjs declares only the former on YText,
    // and both return the same unformatted string.
    expect(doc.getText("content").toJSON()).toBe("hello from the server");
    expect(synced).toHaveBeenCalledTimes(1);
    provider.destroy();
  });

  it("sends local updates and applies remote ones", () => {
    const { doc, provider, socket } = makeProvider();
    socket.open();
    accept(socket);
    socket.sent = [];
    doc.getText("content").insert(0, "typed here");
    const sent = socket.sent.find((frame) => frame[0] === MESSAGE_SYNC);
    expect(sent).toBeDefined();

    // A remote update: another doc's edit, framed as messageYjsUpdate.
    const remote = new Y.Doc();
    remote.getText("content").insert(0, "remote ");
    const update = Y.encodeStateAsUpdate(remote);
    const encoder = encoding.createEncoder();
    encoding.writeVarUint(encoder, MESSAGE_SYNC);
    syncProtocol.writeUpdate(encoder, update);
    socket.receive(encoding.toUint8Array(encoder));
    expect(doc.getText("content").toJSON()).toContain("remote ");
    provider.destroy();
  });

  it("relays awareness both ways", () => {
    const { awareness, provider, socket } = makeProvider();
    socket.open();
    accept(socket);
    socket.sent = [];
    awareness.setLocalStateField("user", { name: "Ada" });
    expect(socket.sent.some((frame) => frame[0] === 1)).toBe(true);

    const otherDoc = new Y.Doc();
    const other = new Awareness(otherDoc);
    other.setLocalStateField("user", { name: "Grace" });
    const encoder = encoding.createEncoder();
    encoding.writeVarUint(encoder, 1);
    encoding.writeVarUint8Array(
      encoder,
      encodeAwarenessUpdate(other, [otherDoc.clientID]),
    );
    socket.receive(encoding.toUint8Array(encoder));
    const states = [...awareness.getStates().values()] as {
      user?: { name?: string };
    }[];
    expect(states.some((state) => state.user?.name === "Grace")).toBe(true);
    provider.destroy();
  });

  it("hands control messages to the handler and sends flush/resolve", () => {
    const controls: object[] = [];
    const { provider, socket } = makeProvider({
      onControl: (control) => controls.push(control),
    });
    socket.open();
    socket.receive(
      controlFrame({ kind: "save-failed", detail: "no frontmatter" }),
    );
    expect(controls).toContainEqual({
      kind: "save-failed",
      detail: "no frontmatter",
    });

    socket.sent = [];
    provider.flush();
    provider.resolve("mine");
    const decoded = socket.sent.map((frame) => {
      const decoder = decoding.createDecoder(frame);
      expect(decoding.readVarUint(decoder)).toBe(MESSAGE_CONTROL);
      return JSON.parse(
        new TextDecoder().decode(decoding.readVarUint8Array(decoder)),
      ) as object;
    });
    expect(decoded).toEqual([
      { kind: "flush" },
      { kind: "resolve", choice: "mine" },
    ]);
    provider.destroy();
  });

  it("reconnects with backoff on ordinary closes but never on 44xx", () => {
    vi.useFakeTimers();
    const statuses: string[] = [];
    const { provider, socket } = makeProvider({
      onStatus: (status) => statuses.push(status),
    });
    socket.open();
    socket.dropWith(1006);
    expect(statuses).toContain("reconnecting");
    expect(FakeSocket.instances.length).toBe(1);
    vi.advanceTimersByTime(600);
    // A retry socket opened.
    expect(FakeSocket.instances.length).toBe(2);

    const retry = FakeSocket.instances[1];
    if (!retry) throw new Error("no retry socket");
    retry.open();
    retry.dropWith(4404); // permanent: the session was closed for good
    const count = FakeSocket.instances.length;
    vi.advanceTimersByTime(60_000);
    // No further retries after a permanent close.
    expect(FakeSocket.instances.length).toBe(count);
    expect(statuses.at(-1)).toBe("failed");
    provider.destroy();
    vi.useRealTimers();
  });

  it("climbs the backoff ladder when a socket opens but never says hello", () => {
    vi.useFakeTimers();
    // No jitter, so the ladder's rungs are exact: 250ms, 500ms, 1000ms, ...
    const random = vi.spyOn(Math, "random").mockReturnValue(0);
    const { provider, socket } = makeProvider();
    // A flapping daemon: the TCP connection comes up and dies before the
    // session is usable. "Connected" is not "accepted", so the ladder must
    // keep climbing instead of hammering the server twice a second forever.
    socket.open();
    socket.dropWith(1006);
    vi.advanceTimersByTime(260);
    expect(FakeSocket.instances).toHaveLength(2);

    const second = FakeSocket.instances[1];
    if (!second) throw new Error("no second socket");
    second.open();
    second.dropWith(1006);
    vi.advanceTimersByTime(260);
    // Rung two is 500ms: a reset-on-open would already have dialed again.
    expect(FakeSocket.instances).toHaveLength(2);
    vi.advanceTimersByTime(260);
    expect(FakeSocket.instances).toHaveLength(3);

    const third = FakeSocket.instances[2];
    if (!third) throw new Error("no third socket");
    third.open();
    third.dropWith(1006);
    // Rung three is 1000ms, and five seconds of flapping stays bounded.
    vi.advanceTimersByTime(5_000);
    expect(FakeSocket.instances.length).toBeLessThanOrEqual(6);
    provider.destroy();
    random.mockRestore();
    vi.useRealTimers();
  });

  it("resets the backoff ladder only once a hello is accepted", () => {
    vi.useFakeTimers();
    const random = vi.spyOn(Math, "random").mockReturnValue(0);
    const { provider, socket } = makeProvider();
    socket.open();
    socket.dropWith(1006);
    vi.advanceTimersByTime(260); // rung one, 250ms
    const second = FakeSocket.instances[1];
    if (!second) throw new Error("no second socket");
    second.open();
    second.dropWith(1006);
    vi.advanceTimersByTime(520); // rung two, 500ms
    const third = FakeSocket.instances[2];
    if (!third) throw new Error("no third socket");

    // This one is a real session: it greets us in our own epoch.
    third.open();
    accept(third, "e1");
    third.dropWith(1006);
    // Back to rung one, because a usable session is what "success" means.
    vi.advanceTimersByTime(260);
    expect(FakeSocket.instances).toHaveLength(4);
    provider.destroy();
    random.mockRestore();
    vi.useRealTimers();
  });

  it("never speaks y-sync into a different epoch: silence, then a rebuild signal", () => {
    vi.useFakeTimers();
    const epochGap = vi.fn();
    const { doc, provider, socket } = makeProvider({ onEpochGap: epochGap });
    socket.open();
    accept(socket, "e1");
    // Synced and edited: this doc now holds state a fresh server session
    // must never receive.
    doc.getText("content").insert(0, "local history");
    socket.dropWith(1006);
    vi.advanceTimersByTime(600);
    const retry = FakeSocket.instances[1];
    if (!retry) throw new Error("no retry socket");
    retry.open();
    // The restarted server greets in ONE frame: hello with a NEW epoch,
    // then its SyncStep1. The provider must stop at the hello - answering
    // that step1 with a SyncStep2 of the old doc is exactly the room-wide
    // text duplication this test exists to prevent.
    const freshServer = new Y.Doc();
    freshServer.getText("content").insert(0, "fresh from the file");
    retry.receive(concat(helloFrame("e2"), serverStep1(freshServer)));
    expect(retry.sent).toHaveLength(0);
    expect(epochGap).toHaveBeenCalledTimes(1);
    expect(retry.readyState).toBe(3);
    // And it stays down: the gap is terminal for THIS provider - the owner
    // builds a new doc + provider pair around the new epoch.
    vi.advanceTimersByTime(60_000);
    expect(FakeSocket.instances).toHaveLength(2);
    provider.destroy();
    vi.useRealTimers();
  });

  it("ignores frames that land after the epoch gap tore the session down", () => {
    vi.useFakeTimers();
    const epochGap = vi.fn();
    const controls: object[] = [];
    const { provider, socket } = makeProvider({
      onEpochGap: epochGap,
      onControl: (control) => controls.push(control),
    });
    socket.open();
    accept(socket, "e1");
    socket.dropWith(1006);
    vi.advanceTimersByTime(600);
    const retry = FakeSocket.instances[1];
    if (!retry) throw new Error("no retry socket");
    retry.open();
    retry.receive(helloFrame("e2"));
    const after = controls.length;
    // A frame the browser had already queued when we tore the socket down.
    // The owner is rebuilding around the new epoch by now; hearing from a
    // discarded provider would make it rebuild twice.
    retry.receive(helloFrame("e3"));
    expect(epochGap).toHaveBeenCalledTimes(1);
    expect(controls).toHaveLength(after);
    provider.destroy();
    vi.useRealTimers();
  });

  it("fails the FIRST connect after the timeout so the editor can go solo", () => {
    vi.useFakeTimers();
    const statuses: string[] = [];
    const { provider, socket } = makeProvider({
      onStatus: (status) => statuses.push(status),
    });
    // Never opens.
    vi.advanceTimersByTime(4100);
    expect(statuses.at(-1)).toBe("failed");
    expect(socket.readyState).toBe(3);
    provider.destroy();
    vi.useRealTimers();
  });

  it("says nothing to an owner that destroyed it while still connecting", () => {
    vi.useFakeTimers();
    const statuses: string[] = [];
    const doc = new Y.Doc();
    // A browser closes a CONNECTING socket asynchronously, so `close()` here
    // reports the new state without delivering an event of its own: the
    // deadline is on its own to be cleared by destroy().
    const silent = {
      binaryType: "",
      readyState: 0,
      send: () => undefined,
      close() {
        this.readyState = 3;
      },
    };
    const provider = new CollabProvider(
      "/ws-under-test",
      doc,
      new Awareness(doc),
      {
        onControl: () => undefined,
        onStatus: (status) => statuses.push(status),
        onSynced: () => undefined,
        onEpochGap: () => undefined,
      },
      () => silent as unknown as WebSocket,
    );
    // Torn down before the socket ever opened: the connect deadline must go
    // with it, or it fires "failed" into an unmounted owner.
    provider.destroy();
    vi.advanceTimersByTime(10_000);
    expect(statuses).not.toContain("failed");
    vi.useRealTimers();
  });
});
