/**
 * The fake socket and the frame builders both collab test files drive their
 * sessions with. Test-only: nothing in the app imports this module, and it
 * pulls in no test runner of its own, so it stays a plain module the type
 * checker and the linter see like any other.
 *
 * The "server" in these tests is a real second Y.Doc plus real y-protocols
 * calls, so what is exercised is interop with the actual codec rather than
 * with a mirror of our own encoder.
 */

import * as decoding from "lib0/decoding";
import * as encoding from "lib0/encoding";
import * as syncProtocol from "y-protocols/sync";
import type * as Y from "yjs";

import type { CollabHello } from "./provider";
import { MESSAGE_CONTROL, MESSAGE_SYNC } from "./provider";

export class FakeSocket {
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

/** The socket factory a provider under test is built with. */
export function fakeSocketFactory(url: string): WebSocket {
  return new FakeSocket(url) as unknown as WebSocket;
}

/** A server frame: the yrs side is byte-compatible with y-protocols. */
export function serverStep1(serverDoc: Y.Doc): Uint8Array {
  const encoder = encoding.createEncoder();
  encoding.writeVarUint(encoder, MESSAGE_SYNC);
  syncProtocol.writeSyncStep1(encoder, serverDoc);
  return encoding.toUint8Array(encoder);
}

export function controlFrame(control: object): Uint8Array {
  const encoder = encoding.createEncoder();
  encoding.writeVarUint(encoder, MESSAGE_CONTROL);
  encoding.writeVarUint8Array(
    encoder,
    new TextEncoder().encode(JSON.stringify(control)),
  );
  return encoding.toUint8Array(encoder);
}

export function helloFrame(
  epoch: string,
  overrides: Partial<CollabHello> = {},
): Uint8Array {
  return controlFrame({
    kind: "hello",
    epoch,
    separator: "\n",
    checksum: "c1",
    permalink: "alpha",
    save_state: "ok",
    ...overrides,
  });
}

/** Complete the epoch handshake the way the server's greeting does. */
export function accept(socket: FakeSocket, epoch = "e1"): void {
  socket.receive(helloFrame(epoch));
}

/**
 * Answer the client's SyncStep1 from a server-side doc, the way a real
 * session does: read the frame it sent, run it through y-protocols against
 * `serverDoc` and hand the reply back.
 */
export function answerStep1(socket: FakeSocket, serverDoc: Y.Doc): void {
  const step1 = socket.sent.find((frame) => frame[0] === MESSAGE_SYNC);
  if (!step1) {
    throw new Error("the client sent no SyncStep1 to answer");
  }
  const decoder = decoding.createDecoder(step1);
  decoding.readVarUint(decoder); // message type
  const reply = encoding.createEncoder();
  encoding.writeVarUint(reply, MESSAGE_SYNC);
  syncProtocol.readSyncMessage(decoder, reply, serverDoc, "test-server");
  socket.receive(encoding.toUint8Array(reply));
}

/** Every control frame the socket has been sent, decoded, in order. */
export function sentControls(socket: FakeSocket): { kind: string }[] {
  const controls: { kind: string }[] = [];
  for (const frame of socket.sent) {
    const decoder = decoding.createDecoder(frame);
    if (decoding.readVarUint(decoder) !== MESSAGE_CONTROL) {
      continue;
    }
    controls.push(
      JSON.parse(
        new TextDecoder().decode(decoding.readVarUint8Array(decoder)),
      ) as { kind: string },
    );
  }
  return controls;
}

export function concat(...frames: Uint8Array[]): Uint8Array {
  const total = frames.reduce((sum, frame) => sum + frame.length, 0);
  const out = new Uint8Array(total);
  let at = 0;
  for (const frame of frames) {
    out.set(frame, at);
    at += frame.length;
  }
  return out;
}
