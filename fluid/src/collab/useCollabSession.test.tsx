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

import { ApiProblem } from "../api/client";
import type { EngramDetail } from "../api/engram";
import { fetchEngramDetail } from "../api/engram";
import { readDraft } from "../editor/drafts";
import type { CollabHello } from "./provider";
import {
  BACKOFF_CAP_MS,
  CONNECT_TIMEOUT_MS,
  MESSAGE_AWARENESS,
  TEXT_NAME,
} from "./provider";
import {
  FakeSocket,
  answerStep1,
  applyClientFrames,
  concat,
  controlFrame,
  fakeSocketFactory,
  helloFrame,
  sentControls,
  serverStep1,
} from "./testSupport";
import type { CollabSession } from "./useCollabSession";
import {
  MERGE_NOTICE_MS,
  fileSpace,
  useCollabSession,
} from "./useCollabSession";

// The detail read is the one request this hook makes: what a tab that joined
// DURING a conflict re-derives the missing conflict body from.
vi.mock("../api/engram", () => ({ fetchEngramDetail: vi.fn() }));
const detailMock = vi.mocked(fetchEngramDetail);

/** The engram read the derivation goes through, holding `content`. */
function detailOf(content: string): EngramDetail {
  return {
    domain: "eng",
    permalink: "alpha",
    title: "A",
    url: "crystalline://eng/alpha",
    path: "alpha.md",
    content,
    checksum: "c1",
    frontmatter: {
      type: null,
      status: null,
      tags: [],
      salience: null,
      validFrom: null,
      validTo: null,
      staleAfter: null,
      verified: [],
    },
    observations: [],
    relations: [],
    links: [],
    inboundCount: 0,
    inboundRefs: [],
  };
}

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
  detailMock.mockReset();
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

  it("re-derives the conflict for a tab that joined while one stood", async () => {
    // The Conflict broadcast predates this subscription: all the greeting
    // carries is that saving is suspended, so the body has to be found again
    // or the room's banner would open onto nothing.
    detailMock.mockResolvedValue(detailOf("THEIR text"));
    joinRoom({ save_state: "conflict" });
    expect(session().saveState).toBe("conflict");
    await act(async () => {
      await Promise.resolve();
    });
    expect(detailMock).toHaveBeenCalledWith("eng", "alpha");
    expect(session().conflict?.kind).toBe("edit");
    expect(session().conflict?.theirs).toBe("THEIR text");
    expect(session().conflict?.detail).toMatch(/changed outside the session/);
  });

  it("reads a joiner's conflict as a deletion when the engram is gone", async () => {
    detailMock.mockRejectedValue(new ApiProblem(404, "not found", "gone"));
    joinRoom({ save_state: "conflict" });
    await act(async () => {
      await Promise.resolve();
    });
    expect(session().conflict?.kind).toBe("deleted");
    expect(session().conflict?.theirs).toBeNull();
  });

  it("says their text is unknown rather than guessing when the read fails", async () => {
    detailMock.mockRejectedValue(new ApiProblem(500, "boom", "the disk went"));
    joinRoom({ save_state: "conflict" });
    await act(async () => {
      await Promise.resolve();
    });
    // Not a deletion: nothing was learned, so nothing is claimed. Both
    // resolutions stay available and the session text is still the session's.
    expect(session().conflict?.kind).toBe("edit");
    expect(session().conflict?.theirs).toBeNull();
  });

  it("re-derives nothing for a room that greeted it in good order", async () => {
    joinRoom();
    await act(async () => {
      await Promise.resolve();
    });
    expect(detailMock).not.toHaveBeenCalled();
    expect(session().conflict).toBeNull();
  });

  it("clears the conflict when the save the resolution re-armed lands", () => {
    // "Mine" is announced by its receipt: the server re-arms the flush and
    // the Saved control is the only word the room gets that it is over.
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
    expect(session().conflict).not.toBeNull();
    act(() => {
      session().resolve("mine");
      socket.receive(
        controlFrame({ kind: "saved", checksum: "c2", permalink: "alpha" }),
      );
    });
    expect(session().conflict).toBeNull();
    expect(session().saveState).toBe("ok");
    expect(session().saveDetail).toBeNull();
  });

  it("clears the conflict when the room converges on their version", () => {
    // "Theirs" sends no Saved at all: the document converges and the room is
    // told it merged. Without this the banner would stand forever.
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
      session().resolve("theirs");
    });
    act(() => {
      socket.receive(controlFrame({ kind: "merged" }));
    });
    expect(session().conflict).toBeNull();
    expect(session().saveState).toBe("ok");
  });

  it("a merged control in a healthy room leaves the save state alone", () => {
    const { socket } = joinRoom();
    act(() => {
      session().ytext?.insert(0, "x");
    });
    expect(session().saveState).toBe("pending");
    act(() => {
      socket.receive(controlFrame({ kind: "merged" }));
    });
    // Only a conflict-suspended room heals on a merge; a room that is merely
    // mid-save must not be told it is saved.
    expect(session().saveState).toBe("pending");
  });

  it("a derivation that settles after the room healed writes nothing", async () => {
    // The race the guard exists for: another participant resolves between the
    // greeting and this tab's read landing, and a late answer would otherwise
    // plant a conflict on a room that has none.
    let land: (detail: EngramDetail) => void = () => undefined;
    detailMock.mockReturnValue(
      new Promise<EngramDetail>((resolve) => {
        land = resolve;
      }),
    );
    const { socket } = joinRoom({ save_state: "conflict" });
    expect(session().saveState).toBe("conflict");
    act(() => {
      socket.receive(
        controlFrame({ kind: "saved", checksum: "c2", permalink: "alpha" }),
      );
    });
    expect(session().saveState).toBe("ok");
    await act(async () => {
      land(detailOf("THEIR text"));
      await Promise.resolve();
    });
    expect(session().conflict).toBeNull();
    expect(session().saveState).toBe("ok");
  });

  it("prefers the broadcast conflict over a re-derived one", async () => {
    detailMock.mockResolvedValue(detailOf("THEIR text"));
    const { socket } = joinRoom({ save_state: "conflict" });
    act(() => {
      socket.receive(
        controlFrame({
          kind: "conflict",
          conflict_kind: "edit",
          theirs: "the broadcast text",
          detail: "an agent rewrote this engram",
        }),
      );
    });
    await act(async () => {
      await Promise.resolve();
    });
    // The server's own account of the conflict wins; the re-derivation is
    // only there for the tab that never saw it.
    expect(session().conflict).toEqual({
      kind: "edit",
      theirs: "the broadcast text",
      detail: "an agent rewrote this engram",
    });
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

describe("useCollabSession reconnecting", () => {
  it("resyncs on the same epoch, keeping the buffer and delivering what was typed offline", () => {
    const { socket, server } = joinRoom();
    const ytext = session().ytext;
    expect(session().status).toBe("connected");

    act(() => {
      socket.dropWith(1006);
    });
    expect(session().status).toBe("reconnecting");
    // The author goes on typing into a document nobody else can see yet.
    act(() => {
      session().ytext?.insert(0, "offline ");
    });
    expect(session().mode).toBe("collab");

    act(() => {
      vi.advanceTimersByTime(BACKOFF_CAP_MS);
    });
    const retry = socketAt(1);
    act(() => {
      retry.open();
    });
    // Mute until the greeting: a reconnect that spoke y-sync first would
    // pour this doc's history into whatever session answered.
    expect(retry.sent).toHaveLength(0);
    expect(session().status).toBe("connected");

    act(() => {
      retry.receive(concat(helloFrame("e1"), serverStep1(server)));
    });
    act(() => {
      applyClientFrames(retry, server);
    });
    // The same session, so the same document: no rebuild, and the offline
    // edit rode out on the answer to the server's own SyncStep1.
    expect(server.getText(TEXT_NAME).toJSON()).toContain("offline ");
    expect(session().mode).toBe("collab");
    expect(session().ytext).toBe(ytext);
    expect(session().epoch).toBe("e1");
  });

  it("keeps a joined room editable through a long outage instead of forking to solo", () => {
    // Ambiguity 8, pinned: a mid-session drop never opens a second history
    // beside the room's. The author keeps typing; the status line says why.
    const { socket } = joinRoom();
    const ytext = session().ytext;
    act(() => {
      socket.dropWith(1006);
    });
    act(() => {
      vi.advanceTimersByTime(120_000);
    });
    expect(session().mode).toBe("collab");
    expect(session().ytext).toBe(ytext);
    expect(session().status).toBe("reconnecting");
  });

  it("rebuilds around a new epoch, keeping the pre-gap text as a draft first", () => {
    const { socket } = joinRoom({ separator: "\r\n" });
    act(() => {
      session().ytext?.insert(0, "mine\n");
    });
    const before = session().ytext;
    act(() => {
      socket.dropWith(1006);
    });
    act(() => {
      vi.advanceTimersByTime(BACKOFF_CAP_MS);
    });

    // The daemon restarted: a fresh session, seeded from the file.
    const fresh = new Y.Doc();
    fresh.getText(TEXT_NAME).insert(0, "fresh from the file\n");
    const retry = socketAt(1);
    act(() => {
      retry.open();
      retry.receive(concat(helloFrame("e2"), serverStep1(fresh)));
    });

    // The draft is written BEFORE anything is rebuilt: it is the only copy
    // of the pre-gap text, in the file's own line endings.
    const draft = readDraft("ada", "eng", "alpha");
    expect(draft?.content).toBe(fileSpace(`mine\n${SESSION_TEXT}`, "\r\n"));
    expect(draft?.baseChecksum).toBe("c1");
    expect(session().mode).not.toBe("collab");

    const rebuilt = socketAt(2);
    act(() => {
      rebuilt.open();
      rebuilt.receive(helloFrame("e2", { separator: "\r\n" }));
    });
    act(() => {
      answerStep1(rebuilt, fresh);
    });
    expect(session().epoch).toBe("e2");
    expect(session().mode).toBe("collab");
    expect(session().ytext).not.toBe(before);
    expect(session().ytext?.toJSON()).toBe("fresh from the file\n");
  });

  it("falls back to solo when sockets keep opening and dropping without a greeting", () => {
    // The first skeleton wedge: an open socket sets `everConnected`, so a
    // per-attempt deadline never fires again and the author would hold a
    // connecting skeleton forever with no escape but a reload.
    mount();
    const first = socketAt(0);
    act(() => {
      first.open();
      first.dropWith(1006);
    });
    expect(session().mode).toBe("connecting");
    act(() => {
      vi.advanceTimersByTime(1000);
    });
    const second = socketAt(1);
    act(() => {
      second.open();
      second.dropWith(1006);
    });
    act(() => {
      vi.advanceTimersByTime(CONNECT_TIMEOUT_MS);
    });
    expect(session().mode).toBe("solo");
    expect(session().ytext).toBeNull();
    // "failed" rather than a lingering "connecting" is what the surface's
    // quiet solo notice keys on: an attempt that is over, not one running.
    expect(session().status).toBe("failed");
  });

  it("falls back to solo when the session rebuilt around a new epoch never lands", () => {
    // The second skeleton wedge: after a gap the mode would have stayed
    // "collab" with no document to bind, which renders as a permanent
    // skeleton. The rebuild is bounded like any first connect.
    const { socket } = joinRoom();
    act(() => {
      socket.dropWith(1006);
    });
    act(() => {
      vi.advanceTimersByTime(BACKOFF_CAP_MS);
    });
    const retry = socketAt(1);
    act(() => {
      retry.open();
      retry.receive(helloFrame("e2"));
    });
    expect(session().mode).not.toBe("collab");
    // The rebuilt provider never gets a socket up.
    act(() => {
      vi.advanceTimersByTime(CONNECT_TIMEOUT_MS + 100);
    });
    expect(session().mode).toBe("solo");
    expect(session().ytext).toBeNull();
    expect(session().status).toBe("failed");
  });
});
