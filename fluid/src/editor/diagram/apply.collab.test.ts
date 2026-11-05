/**
 * The apply layer with two peers: an op and a peer's edit in one fence, both
 * before sync and between compile and write. Run on the surface-built peers
 * (`markPeers`) and on the grid's room (`cellKit.roomMain`, whose relay can be
 * held). `apply.collab.marks-hidden.test.ts` and `apply.collab.marks-show.test.ts`
 * run this file again with the marks layer on.
 */

import type { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, test } from "vitest";

import { cleanupCells, roomMain } from "../../test/cellKit";
import { cellMarksMode } from "../../test/cellMarksMode";
import { bytes, cleanupMarkViews, markPeers } from "../../test/markKit";
import { diagramAnchors } from "../diagramAnchors";
import { roomUndo } from "../roomUndo";
import { flowKind, runDiagramOp } from "./apply";
import type { Verdict } from "./verdict";

afterEach(() => {
  cleanupMarkViews();
  cleanupCells();
});

const BODY = "flowchart TD\n    A[First step] --> B[Next step]";
const DOC = `Intro\n\n\`\`\`mermaid\n${BODY}\n\`\`\`\n`;
const ANCHOR = DOC.indexOf("```");
const NOTE = "    %% peer note\n";
const NOTE_AT = DOC.indexOf("    A[First");
const CHILD = "    A --> C[New box]\n";
const OK: Verdict = { ok: true, kind: "flowchart", domIds: new Map() };

interface Room {
  local: EditorView;
  /** The room's text as the peer holds it. */
  peerText: () => string;
  /** A peer's insert. What reaches the local view is up to `sync`. */
  peerInsert: (at: number, text: string) => void;
  peerDelete: (at: number, length: number) => void;
  /** Holds both ways until `sync`. */
  hold: () => void;
  sync: () => void;
  undo: () => void;
}

function surfaceRoom(): Room {
  const mode = cellMarksMode();
  const { local, peer, sync } = markPeers(
    DOC,
    mode === "off" ? { raw: true } : { mode },
  );
  return {
    local,
    peerText: () => bytes(peer.state),
    peerInsert: (at, text) => {
      peer.dispatch({ changes: { from: at, insert: text } });
    },
    peerDelete: (at, length) => {
      peer.dispatch({ changes: { from: at, to: at + length } });
    },
    hold: () => undefined,
    sync,
    undo: () => {
      local.state.facet(roomUndo)?.undo();
    },
  };
}

function cellKitRoom(): Room {
  const room = roomMain(DOC, [diagramAnchors]);
  return {
    local: room.main,
    peerText: () => room.rtext.toJSON(),
    peerInsert: (at, text) => {
      room.rtext.insert(at, text);
    },
    peerDelete: (at, length) => {
      room.rtext.delete(at, length);
    },
    hold: room.hold,
    sync: room.release,
    undo: () => {
      room.undoManager.undo();
    },
  };
}

const stubbed = (verify: () => Verdict) => ({
  ...flowKind,
  verify: () => Promise.resolve(verify()),
});

describe.each([
  ["markPeers", surfaceRoom],
  ["roomMain", cellKitRoom],
])("two peers, %s", (_name, build) => {
  test("an op and a peer's new line in one fence, before sync, both stay", async () => {
    const room = build();
    room.hold();
    room.peerInsert(NOTE_AT, NOTE);
    // Real mermaid: the local op knows nothing of the note.
    const outcome = await runDiagramOp(room.local, ANCHOR, flowKind, {
      op: "addChild",
      id: "A",
    });
    expect(outcome).toEqual({
      applied: true,
      created: { kind: "node", id: "C" },
    });
    room.sync();
    const both = `Intro\n\n\`\`\`mermaid\nflowchart TD\n${NOTE}    A[First step] --> B[Next step]\n${CHILD}\`\`\`\n`;
    expect(bytes(room.local.state)).toBe(both);
    expect(room.peerText()).toBe(both);

    room.undo();
    const withoutChild = both.replace(CHILD, "");
    expect(bytes(room.local.state)).toBe(withoutChild);
    room.sync();
    expect(room.peerText()).toBe(withoutChild);
  });

  test("a peer's edit in the fence between compile and write is survived", async () => {
    const room = build();
    let first = true;
    const kind = stubbed(() => {
      if (first) {
        first = false;
        room.peerInsert(NOTE_AT, NOTE);
        room.sync();
      }
      return OK;
    });
    await expect(
      runDiagramOp(room.local, ANCHOR, kind, { op: "addChild", id: "A" }),
    ).resolves.toMatchObject({ applied: true });
    room.sync();
    const both = `Intro\n\n\`\`\`mermaid\nflowchart TD\n${NOTE}    A[First step] --> B[Next step]\n${CHILD}\`\`\`\n`;
    expect(bytes(room.local.state)).toBe(both);
    expect(room.peerText()).toBe(both);
  });

  test("a peer's edit above the fence between compile and write moves the write", async () => {
    const room = build();
    const kind = stubbed(() => {
      room.peerInsert(0, "Hello\n\n");
      room.sync();
      return OK;
    });
    await runDiagramOp(room.local, ANCHOR, kind, {
      op: "rename",
      id: "B",
      text: "Done",
    });
    room.sync();
    const both = `Hello\n\n${DOC.replace("Next step", "Done")}`;
    expect(bytes(room.local.state)).toBe(both);
    expect(room.peerText()).toBe(both);
  });

  test("a peer changing the fence twice gives stale, and the peer's text stands", async () => {
    const room = build();
    let at = DOC.indexOf("First");
    const kind = stubbed(() => {
      room.peerInsert(at, "x");
      at += 1;
      room.sync();
      return OK;
    });
    await expect(
      runDiagramOp(room.local, ANCHOR, kind, {
        op: "rename",
        id: "B",
        text: "Done",
      }),
    ).resolves.toEqual({ applied: false, refused: "stale" });
    const both = DOC.replace("First", "xxFirst");
    expect(bytes(room.local.state)).toBe(both);
    expect(room.peerText()).toBe(both);
    expect(room.local.state.field(diagramAnchors).size).toBe(0);
  });

  test("a peer deleting the fence between compile and write leaves it gone", async () => {
    const room = build();
    const kind = stubbed(() => {
      room.peerDelete(ANCHOR, DOC.length - ANCHOR);
      room.sync();
      return OK;
    });
    await expect(
      runDiagramOp(room.local, ANCHOR, kind, {
        op: "rename",
        id: "B",
        text: "Done",
      }),
    ).resolves.toEqual({ applied: false, refused: "gone" });
    expect(bytes(room.local.state)).toBe("Intro\n\n");
  });
});
