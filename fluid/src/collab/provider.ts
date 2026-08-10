/**
 * The session transport: one WebSocket speaking the y-sync + awareness
 * protocol plus Crystalline's control channel (message type 4, JSON). The
 * editor binding never sees this class - y-codemirror.next takes the Y.Doc's
 * text and the Awareness directly - so everything here is headless.
 *
 * Reconnect policy: ordinary closes retry with exponential backoff and the
 * same doc (a resync reconciles); close codes 4400-4499 are permanent, the
 * y-websocket convention the server uses for "this session is gone for good".
 * The FIRST connect gets CONNECT_TIMEOUT_MS to succeed; failing that, status
 * goes "failed" and the editor opens solo instead.
 *
 * The epoch discipline, and why no y-sync leaves before the server's hello is
 * accepted: a daemon restart replaces the session with a fresh doc seeded
 * from the file. Answering the fresh session's SyncStep1 with this doc's
 * state would MERGE two unrelated histories - duplicated text for the whole
 * room. So every connection is mute until its hello matches the adopted
 * epoch; a mismatch stops the frame unanswered, closes for good and hands
 * the owner onEpochGap to snapshot and rebuild around.
 */

import * as decoding from "lib0/decoding";
import * as encoding from "lib0/encoding";
import {
  Awareness,
  applyAwarenessUpdate,
  encodeAwarenessUpdate,
  removeAwarenessStates,
} from "y-protocols/awareness";
import * as syncProtocol from "y-protocols/sync";
import * as Y from "yjs";

import { API_BASE, encodePermalink, encodeSegment } from "../api/client";

/**
 * The name of the shared text inside the session document, matching the
 * server's own `TEXT_NAME`. The one client-side spelling: a second literal
 * somewhere else would bind an editor to an empty text and look like a sync
 * failure rather than a typo.
 */
export const TEXT_NAME = "content";

export const MESSAGE_SYNC = 0;
export const MESSAGE_AWARENESS = 1;
export const MESSAGE_QUERY_AWARENESS = 3;
export const MESSAGE_CONTROL = 4;
export const CONNECT_TIMEOUT_MS = 4000;
export const BACKOFF_BASE_MS = 500;
export const BACKOFF_CAP_MS = 10_000;

export interface CollabHello {
  kind: "hello";
  epoch: string;
  separator: "\r\n" | "\n";
  checksum: string;
  permalink: string;
  save_state: "ok" | "failed" | "conflict";
}

export type CollabControl =
  | CollabHello
  | { kind: "saved"; checksum: string; permalink: string }
  | { kind: "save-failed"; detail: string }
  | { kind: "merged" }
  | {
      kind: "conflict";
      conflict_kind: "edit" | "deleted";
      theirs: string | null;
      detail: string;
    }
  | { kind: "closed"; reason: string };

export type CollabStatus =
  "connecting" | "connected" | "reconnecting" | "failed";

export interface ProviderHandlers {
  onControl: (control: CollabControl) => void;
  onStatus: (status: CollabStatus) => void;
  onSynced: () => void;
  /** A reconnect landed on a DIFFERENT server session; this provider is
   *  already permanently silent. Snapshot and rebuild - see the class doc. */
  onEpochGap: () => void;
}

export type SocketFactory = (url: string) => WebSocket;

export function collabUrl(domain: string, permalink: string): string {
  return `${API_BASE}/collab/${encodeSegment(domain)}/${encodePermalink(permalink)}`;
}

/**
 * The absolute socket URL for a same-origin path: the page's own host with
 * its scheme swapped for the WebSocket one, so a page served over TLS keeps
 * its session encrypted. An already absolute URL is returned untouched.
 */
export function collabSocketUrl(
  path: string,
  location: { protocol: string; host: string } = window.location,
): string {
  if (path.startsWith("ws://") || path.startsWith("wss://")) {
    return path;
  }
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${location.host}${path}`;
}

export class CollabProvider {
  private readonly url: string;
  private readonly doc: Y.Doc;
  private readonly awareness: Awareness;
  private readonly handlers: ProviderHandlers;
  private readonly socketFactory: SocketFactory;
  private socket: WebSocket | null = null;
  private closed = false;
  private everConnected = false;
  private synced = false;
  private retries = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  /** The first connect's deadline, held so a teardown mid-CONNECTING takes
   *  it down too: a browser closes a connecting socket asynchronously, so
   *  the close event cannot be relied on to clear it in time. */
  private deadline: ReturnType<typeof setTimeout> | null = null;
  private statusValue: CollabStatus = "connecting";
  /** The server session's epoch, adopted from the first hello. */
  private epoch: string | null = null;
  /** Whether THIS connection's hello has been accepted: no y-sync or
   *  awareness traffic leaves or is answered before that, so a reconnect
   *  can never pour this doc's history into a different server session. */
  private accepted = false;

  constructor(
    url: string,
    doc: Y.Doc,
    awareness: Awareness,
    handlers: ProviderHandlers,
    socketFactory: SocketFactory = (target) =>
      new WebSocket(collabSocketUrl(target)),
  ) {
    this.url = url;
    this.doc = doc;
    this.awareness = awareness;
    this.handlers = handlers;
    this.socketFactory = socketFactory;
    this.doc.on("update", this.onDocUpdate);
    this.awareness.on("update", this.onAwarenessUpdate);
    this.connect();
  }

  get status(): CollabStatus {
    return this.statusValue;
  }

  flush(): void {
    this.sendControl({ kind: "flush" });
  }

  resolve(choice: "mine" | "theirs"): void {
    this.sendControl({ kind: "resolve", choice });
  }

  destroy(): void {
    this.closed = true;
    if (this.timer !== null) {
      clearTimeout(this.timer);
    }
    if (this.deadline !== null) {
      clearTimeout(this.deadline);
      this.deadline = null;
    }
    // The goodbye FIRST, while the awareness listener is still attached to
    // relay it; the server also nulls this client on close, so this is a
    // courtesy for the room's latency, not correctness.
    removeAwarenessStates(this.awareness, [this.doc.clientID], "destroy");
    this.doc.off("update", this.onDocUpdate);
    this.awareness.off("update", this.onAwarenessUpdate);
    this.socket?.close();
    this.socket = null;
  }

  private setStatus(status: CollabStatus): void {
    if (this.statusValue !== status) {
      this.statusValue = status;
      this.handlers.onStatus(status);
    }
  }

  private connect(): void {
    if (this.closed) {
      return;
    }
    const socket = this.socketFactory(this.url);
    this.socket = socket;
    this.synced = false;
    this.accepted = false; // every connection re-earns it from a hello
    socket.binaryType = "arraybuffer";
    // The first connect is the solo-fallback gate: no open within the
    // deadline means the editor should not wait on us.
    const deadline = this.everConnected
      ? null
      : setTimeout(() => {
          this.deadline = null;
          if (!this.everConnected) {
            this.closed = true;
            socket.close();
            this.setStatus("failed");
          }
        }, CONNECT_TIMEOUT_MS);
    this.deadline = deadline;
    socket.onopen = () => {
      if (deadline !== null) {
        clearTimeout(deadline);
        this.deadline = null;
      }
      this.everConnected = true;
      this.setStatus("connected");
      // The retry ladder is NOT reset here: an open socket is only a TCP
      // success. A daemon that accepts and drops (or a session closed right
      // after the upgrade with an ordinary code) would pin the wait at its
      // lowest rung and hammer the server several times a second forever.
      // The reset lives where a hello is accepted, so "success" means a
      // session this doc can actually use.
      // Deliberately NOTHING else: the y-sync handshake waits for the
      // server's hello (see greet), or a reconnect onto a restarted daemon
      // would answer the fresh session's SyncStep1 with this doc's whole
      // unrelated history and duplicate the text for the entire room.
    };
    socket.onmessage = (event) => {
      this.receive(new Uint8Array(event.data as ArrayBuffer));
    };
    socket.onclose = (event) => {
      if (deadline !== null) {
        clearTimeout(deadline);
        this.deadline = null;
      }
      this.socket = null;
      if (this.closed) {
        return;
      }
      if (event.code >= 4400 && event.code <= 4499) {
        // Permanent by convention: auth revoked, engram gone.
        this.closed = true;
        this.setStatus("failed");
        return;
      }
      this.setStatus(this.everConnected ? "reconnecting" : "connecting");
      const wait =
        Math.min(BACKOFF_CAP_MS, BACKOFF_BASE_MS * 2 ** this.retries) *
        (0.5 + Math.random() * 0.5);
      this.retries += 1;
      this.timer = setTimeout(() => {
        this.connect();
      }, wait);
    };
    socket.onerror = () => {
      // onclose follows; nothing to do here.
    };
  }

  /** Our side of the handshake, sent only once a hello is accepted: the
   *  y-websocket choreography of our SyncStep1, then our own presence. */
  private greet(): void {
    const encoder = encoding.createEncoder();
    encoding.writeVarUint(encoder, MESSAGE_SYNC);
    syncProtocol.writeSyncStep1(encoder, this.doc);
    this.send(encoding.toUint8Array(encoder));
    const local = this.awareness.getLocalState();
    if (local !== null) {
      const presence = encoding.createEncoder();
      encoding.writeVarUint(presence, MESSAGE_AWARENESS);
      encoding.writeVarUint8Array(
        presence,
        encodeAwarenessUpdate(this.awareness, [this.doc.clientID]),
      );
      this.send(encoding.toUint8Array(presence));
    }
  }

  private receive(frame: Uint8Array): void {
    if (this.closed) {
      // A frame the browser had already queued when this provider was torn
      // down. Answering it would fire onEpochGap or onControl into an owner
      // that has moved on, and make it rebuild a second time.
      return;
    }
    const decoder = decoding.createDecoder(frame);
    // Several protocol messages may share one WS frame.
    while (decoding.hasContent(decoder)) {
      const messageType = decoding.readVarUint(decoder);
      if (messageType === MESSAGE_CONTROL) {
        const payload = decoding.readVarUint8Array(decoder);
        let control: CollabControl;
        try {
          control = JSON.parse(
            new TextDecoder().decode(payload),
          ) as CollabControl;
        } catch {
          continue; // an unreadable control is dropped; the stream goes on
        }
        if (control.kind === "hello") {
          if (this.epoch !== null && control.epoch !== this.epoch) {
            // A different server session. Its SyncStep1 sits later in this
            // very frame; stop HERE, never answer it, and end this provider
            // for good - the owner rebuilds a fresh doc around the new
            // epoch, and the old text survives in the owner's draft.
            this.closed = true;
            this.socket?.close();
            this.socket = null;
            this.handlers.onEpochGap();
            return;
          }
          this.epoch = control.epoch;
          this.accepted = true;
          // A usable session is what earns the ladder's reset (see onopen).
          this.retries = 0;
          this.greet();
        }
        this.handlers.onControl(control);
        continue;
      }
      if (!this.accepted) {
        // No y-sync before the epoch handshake. The greeting always leads
        // with hello, so anything else this early is not for this doc;
        // stop the frame unanswered.
        return;
      }
      switch (messageType) {
        case MESSAGE_SYNC: {
          const reply = encoding.createEncoder();
          encoding.writeVarUint(reply, MESSAGE_SYNC);
          const syncType = syncProtocol.readSyncMessage(
            decoder,
            reply,
            this.doc,
            this,
          );
          if (encoding.length(reply) > 1) {
            this.send(encoding.toUint8Array(reply));
          }
          if (syncType === syncProtocol.messageYjsSyncStep2 && !this.synced) {
            this.synced = true;
            this.handlers.onSynced();
          }
          break;
        }
        case MESSAGE_AWARENESS: {
          applyAwarenessUpdate(
            this.awareness,
            decoding.readVarUint8Array(decoder),
            this,
          );
          break;
        }
        case MESSAGE_QUERY_AWARENESS: {
          const reply = encoding.createEncoder();
          encoding.writeVarUint(reply, MESSAGE_AWARENESS);
          encoding.writeVarUint8Array(
            reply,
            encodeAwarenessUpdate(this.awareness, [
              ...this.awareness.getStates().keys(),
            ]),
          );
          this.send(encoding.toUint8Array(reply));
          break;
        }
        default:
          // Auth (2) and anything newer: nothing to do, and the lib0 framing
          // means we cannot skip an unknown body reliably - stop this frame.
          return;
      }
    }
  }

  private onDocUpdate = (update: Uint8Array, origin: unknown): void => {
    if (origin === this) {
      return; // a remote update we just applied; never echo it back
    }
    if (!this.accepted) {
      // Local typing before (or during) the epoch handshake stays local;
      // the SyncStep1/SyncStep2 exchange after acceptance delivers it.
      return;
    }
    const encoder = encoding.createEncoder();
    encoding.writeVarUint(encoder, MESSAGE_SYNC);
    syncProtocol.writeUpdate(encoder, update);
    this.send(encoding.toUint8Array(encoder));
  };

  private onAwarenessUpdate = (
    changes: { added: number[]; updated: number[]; removed: number[] },
    origin: unknown,
  ): void => {
    if (origin === this || !this.accepted) {
      return;
    }
    const changed = [...changes.added, ...changes.updated, ...changes.removed];
    const encoder = encoding.createEncoder();
    encoding.writeVarUint(encoder, MESSAGE_AWARENESS);
    encoding.writeVarUint8Array(
      encoder,
      encodeAwarenessUpdate(this.awareness, changed),
    );
    this.send(encoding.toUint8Array(encoder));
  };

  private send(bytes: Uint8Array): void {
    if (this.socket !== null && this.socket.readyState === 1) {
      // lib0 types its buffers over `ArrayBufferLike`, which includes the
      // SharedArrayBuffer `send` refuses; every encoder here allocates a
      // plain ArrayBuffer, so the narrowing is a fact, not a hope.
      this.socket.send(bytes as Uint8Array<ArrayBuffer>);
    }
  }

  private sendControl(control: { kind: string; choice?: string }): void {
    const encoder = encoding.createEncoder();
    encoding.writeVarUint(encoder, MESSAGE_CONTROL);
    encoding.writeVarUint8Array(
      encoder,
      new TextEncoder().encode(JSON.stringify(control)),
    );
    this.send(encoding.toUint8Array(encoder));
  }
}
