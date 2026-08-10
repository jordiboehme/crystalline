/**
 * The session hook on its own: what it does with a socket that never opens,
 * with a room that syncs, with the control channel's verdicts about saving,
 * and with the presence of other people.
 *
 * The "server" is a second Y.Doc driven through real y-protocols calls, so
 * what is exercised is the actual wire behavior rather than a mirror of our
 * own encoder.
 */

import { act, renderHook } from "@testing-library/react";
import * as encoding from "lib0/encoding";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Awareness, encodeAwarenessUpdate } from "y-protocols/awareness";
import * as Y from "yjs";

import type { CollabHello } from "./provider";
import { CONNECT_TIMEOUT_MS, MESSAGE_AWARENESS, TEXT_NAME } from "./provider";
import {
  FakeSocket,
  answerStep1,
  controlFrame,
  fakeSocketFactory,
  helloFrame,
  sentControls,
} from "./testSupport";
import type { CollabSession } from "./useCollabSession";
import {
  MERGE_NOTICE_MS,
  fileSpace,
  useCollabSession,
} from "./useCollabSession";

const SESSION_TEXT = "---\ntitle: A\n---\n\nbody\n";

/** The mounted hook, so the assertions can read what it last returned. */
let mounted: { result: { current: CollabSession } } | null = null;

function mount(enabled = true) {
  mounted = renderHook(() =>
    useCollabSession({
      domain: "eng",
      permalink: "alpha",
      account: "ada",
      displayName: "Ada Lovelace",
      enabled,
      socketFactory: fakeSocketFactory,
    }),
  );
}

function session(): CollabSession {
  if (!mounted) {
    throw new Error("the hook has not rendered");
  }
  return mounted.result.current;
}

function socketAt(index: number): FakeSocket {
  const socket = FakeSocket.instances[index];
  if (!socket) {
    throw new Error(`the hook opened no socket at index ${index}`);
  }
  return socket;
}

/** Mount the hook and take the room all the way to synced. */
function joinRoom(hello: Partial<CollabHello> = {}) {
  mount();
  const socket = socketAt(0);
  const server = new Y.Doc();
  server.getText(TEXT_NAME).insert(0, SESSION_TEXT);
  act(() => {
    socket.open();
    socket.receive(helloFrame("e1", hello));
  });
  act(() => {
    answerStep1(socket, server);
  });
  return { socket, server };
}

/** An awareness frame from somebody else in the room. */
function presenceFrame(name: string): Uint8Array {
  const otherDoc = new Y.Doc();
  const other = new Awareness(otherDoc);
  other.setLocalStateField("user", {
    name,
    color: "#f59e0b",
    colorLight: "#f59e0b33",
  });
  const encoder = encoding.createEncoder();
  encoding.writeVarUint(encoder, MESSAGE_AWARENESS);
  encoding.writeVarUint8Array(
    encoder,
    encodeAwarenessUpdate(other, [otherDoc.clientID]),
  );
  return encoding.toUint8Array(encoder);
}

beforeEach(() => {
  FakeSocket.instances = [];
  mounted = null;
  localStorage.clear();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("fileSpace", () => {
  it("re-separates LF session text into the file's own endings", () => {
    expect(fileSpace("a\nb\n", "\r\n")).toBe("a\r\nb\r\n");
    expect(fileSpace("a\nb\n", "\n")).toBe("a\nb\n");
  });
});

describe("useCollabSession", () => {
  it("falls back to solo when the first connect never lands", () => {
    mount();
    expect(session().mode).toBe("connecting");
    act(() => {
      vi.advanceTimersByTime(CONNECT_TIMEOUT_MS + 100);
    });
    expect(session().mode).toBe("solo");
    expect(session().ytext).toBeNull();
  });

  it("opens no socket at all when it is not enabled", () => {
    mount(false);
    expect(session().mode).toBe("solo");
    expect(FakeSocket.instances).toHaveLength(0);
  });

  it("goes collab once the room syncs, carrying the hello's terms", () => {
    joinRoom({ separator: "\r\n" });
    expect(session().mode).toBe("collab");
    expect(session().ytext?.toJSON()).toBe(SESSION_TEXT);
    expect(session().separator).toBe("\r\n");
    expect(session().epoch).toBe("e1");
    expect(session().permalink).toBe("alpha");
  });

  it("tracks the save lifecycle and asks the server to flush", () => {
    const { socket } = joinRoom();
    expect(session().saveState).toBe("ok");

    act(() => {
      session().ytext?.insert(0, "x");
    });
    expect(session().saveState).toBe("pending");

    act(() => {
      socket.receive(
        controlFrame({ kind: "saved", checksum: "c2", permalink: "alpha" }),
      );
    });
    expect(session().saveState).toBe("ok");
    expect(session().saveDetail).toBeNull();

    act(() => {
      socket.receive(controlFrame({ kind: "save-failed", detail: "why" }));
    });
    expect(session().saveState).toBe("failed");
    expect(session().saveDetail).toBe("why");

    act(() => {
      session().flush();
    });
    expect(sentControls(socket)).toContainEqual({ kind: "flush" });
  });

  it("follows a rename receipt to the engram's new permalink", () => {
    const { socket } = joinRoom();
    act(() => {
      socket.receive(
        controlFrame({
          kind: "saved",
          checksum: "c2",
          permalink: "sharper-alpha",
        }),
      );
    });
    expect(session().permalink).toBe("sharper-alpha");
  });

  it("lists who is in the room, the local user included", () => {
    const { socket } = joinRoom();
    act(() => {
      socket.receive(presenceFrame("Grace"));
    });
    const participants = session().participants;
    const grace = participants.find((one) => one.name === "Grace");
    const me = participants.find((one) => one.name === "Ada Lovelace");
    expect(grace).toBeDefined();
    expect(grace?.color).toMatch(/^#[0-9a-f]{6}$/);
    expect(grace?.self).toBe(false);
    expect(me?.self).toBe(true);
  });

  it("carries a conflict and an accepted deletion", () => {
    const { socket } = joinRoom();
    act(() => {
      socket.receive(
        controlFrame({
          kind: "conflict",
          conflict_kind: "edit",
          theirs: "their text",
          detail: "an agent rewrote this engram",
        }),
      );
    });
    expect(session().conflict).toEqual({
      kind: "edit",
      theirs: "their text",
      detail: "an agent rewrote this engram",
    });
    expect(session().saveState).toBe("conflict");
    expect(session().closed).toBe(false);

    act(() => {
      socket.receive(controlFrame({ kind: "closed", reason: "deleted" }));
    });
    expect(session().closed).toBe(true);
  });

  it("raises a merge notice that clears itself", () => {
    const { socket } = joinRoom();
    expect(session().mergeNotice).toBe(false);
    act(() => {
      socket.receive(controlFrame({ kind: "merged" }));
    });
    expect(session().mergeNotice).toBe(true);
    act(() => {
      vi.advanceTimersByTime(MERGE_NOTICE_MS + 100);
    });
    expect(session().mergeNotice).toBe(false);
  });

  it("asks the server to resolve a conflict the way the room chose", () => {
    const { socket } = joinRoom();
    act(() => {
      session().resolve("theirs");
    });
    expect(sentControls(socket)).toContainEqual({
      kind: "resolve",
      choice: "theirs",
    });
  });
});
