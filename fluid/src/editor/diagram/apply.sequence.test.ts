/**
 * Sequence ops through the apply layer: `seqKind` on the real surface, Raw.
 * The tests that name `seqKind` as it is run real mermaid, which is the only
 * layer that sees a `-` left with no `+`. `verify` is stubbed where a test
 * acts between compile and write.
 *
 * A message is named by its occurrence and a block by its number, and a peer's
 * edit during verify moves both. `heldSeqOp` reads them again on the second
 * pass and answers `stale` where it cannot tell which one was meant (R94).
 */

import type { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, test } from "vitest";

import { bytes, cleanupMarkViews, markView } from "../../test/markKit";
import { diagramAnchors, trackAnchor } from "../diagramAnchors";
import S05 from "../fixtures/diagrams/sequence/s05-activation.mmd?raw";
import { runDiagramOp, seqKind } from "./apply";
import type { SeqCreated, SeqTarget } from "./sequence/model";
import { type SeqOp, heldSeqOp } from "./sequence/ops";
import { parseSequence } from "./sequence/parse";
import type { Verdict } from "./verdict";

afterEach(cleanupMarkViews);

const fenced = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\`\n`;

const STARTER =
  "sequenceDiagram\n    participant caller as Caller\n    participant service as Service\n    caller->>service: Request\n    service-->>caller: Reply";

const OK: Verdict = { ok: true, kind: "sequence", domIds: new Map() };

type MessageTarget = SeqTarget & { kind: "message" };

function message(
  from: string,
  to: string,
  text: string,
  occurrence = 0,
): MessageTarget {
  return { kind: "message", from, to, text, occurrence };
}

/** `seqKind` with a `verify` that says ok and lets `peer` act on its first call. */
function withPeer(view: () => EditorView, peer: (view: EditorView) => void) {
  let first = true;
  return {
    ...seqKind,
    verify: () => {
      if (first) {
        first = false;
        peer(view());
      }
      return Promise.resolve(OK);
    },
  };
}

/** Deletes the first `text` of the document. */
function deleteText(view: EditorView, text: string): void {
  const at = bytes(view.state).indexOf(text);
  if (at < 0) {
    throw new Error(`no ${text}`);
  }
  view.dispatch({ changes: { from: at, to: at + text.length } });
}

/** Inserts `text` at the start of the body's line that reads `line`. */
function insertBefore(view: EditorView, line: string, text: string): void {
  const at = bytes(view.state).indexOf(line);
  if (at < 0) {
    throw new Error(`no line ${line}`);
  }
  view.dispatch({ changes: { from: at, insert: text } });
}

describe("seqKind", () => {
  test("an add names what it made, typed", async () => {
    const view = markView(fenced(STARTER), { raw: true });
    const outcome = await runDiagramOp(view, 0, seqKind, {
      op: "addParticipant",
    });
    if (!outcome.applied) {
      throw new Error("refused");
    }
    // No cast: `seqKind` says what `created` holds.
    const made: SeqCreated | undefined = outcome.created;
    expect(made).toEqual({ kind: "participant", name: "p1" });
    expect(bytes(view.state)).toBe(
      fenced(
        STARTER.replace(
          "as Service\n",
          "as Service\n    participant p1 as New participant\n",
        ),
      ),
    );
  });

  test("a message's add names its target, occurrence counted", async () => {
    const view = markView(fenced(STARTER), { raw: true });
    const outcome = await runDiagramOp(view, 0, seqKind, {
      op: "addMessage",
      from: "service",
      to: "caller",
    });
    expect(outcome).toEqual({
      applied: true,
      created: message("service", "caller", "New message"),
    });
    expect(bytes(view.state)).toBe(
      fenced(`${STARTER}\n    service->>caller: New message`),
    );
  });

  // seqKind keeps verify: the op's own check cannot see a `-` with no `+`.
  test("s05: deleting a + message refuses invalid and writes nothing", async () => {
    const body = S05.replace(/\n$/, "");
    const view = markView(fenced(body), { raw: true });
    await expect(
      runDiagramOp(view, 0, seqKind, {
        op: "deleteMessage",
        target: message("client", "router", "call"),
      }),
    ).resolves.toEqual({ applied: false, refused: "invalid" });
    expect(bytes(view.state)).toBe(fenced(body));
  });

  test("an activation its deactivation now misses refuses invalid", async () => {
    const body =
      "sequenceDiagram\n  participant a\n  participant b\n  a->>+b: x\n  b-->>-b: y";
    const view = markView(fenced(body), { raw: true });
    await expect(
      runDiagramOp(view, 0, seqKind, { op: "deleteParticipant", name: "a" }),
    ).resolves.toEqual({ applied: false, refused: "invalid" });
    expect(bytes(view.state)).toBe(fenced(body));
  });

  test("a running composition is stale, and nothing is written", async () => {
    const view = markView(fenced(STARTER), { raw: true });
    Object.defineProperty(view, "composing", {
      configurable: true,
      get: () => true,
    });
    await expect(
      runDiagramOp(view, 0, seqKind, { op: "addParticipant" }),
    ).resolves.toEqual({ applied: false, refused: "stale" });
    expect(bytes(view.state)).toBe(fenced(STARTER));
  });
});

describe("heldSeqOp (R94)", () => {
  const TWINS =
    "sequenceDiagram\n  participant a\n  participant b\n  a->>b: hi\n  b->>a: ok\n  a->>b: hi";

  async function run(
    body: string,
    op: SeqOp,
    peer: (view: EditorView) => void,
    held = true,
  ) {
    const view = markView(fenced(body), { raw: true });
    const kind = withPeer(() => view, peer);
    const outcome = await runDiagramOp(
      view,
      0,
      kind,
      held ? heldSeqOp(op) : op,
    );
    return { outcome, text: bytes(view.state) };
  }

  test("with no change the op stands as it was given, a twin's occurrence too", async () => {
    const { outcome, text } = await run(
      TWINS,
      { op: "renameMessage", target: message("a", "b", "hi", 1), text: "yo" },
      () => undefined,
    );
    expect(outcome).toEqual({ applied: true });
    expect(text).toBe(fenced(TWINS.replace(/hi$/, "yo")));
  });

  test("a peer's twin above a twin is stale, where the plain op lands on another line", async () => {
    const op: SeqOp = {
      op: "renameMessage",
      target: message("a", "b", "hi", 1),
      text: "yo",
    };
    const peer = (view: EditorView) =>
      insertBefore(view, "  a->>b: hi", "  a->>b: hi\n");
    const plain = await run(TWINS, op, peer, false);
    // The plain op renames the line that was the first `hi`: the wrong one.
    expect(plain.outcome).toEqual({ applied: true });
    expect(plain.text).toBe(
      fenced(TWINS.replace("  a->>b: hi\n", "  a->>b: hi\n  a->>b: yo\n")),
    );
    const held = await run(TWINS, op, peer);
    expect(held.outcome).toEqual({ applied: false, refused: "stale" });
    expect(held.text).toBe(
      fenced(TWINS.replace("  a->>b: hi\n", "  a->>b: hi\n  a->>b: hi\n")),
    );
  });

  test("a sole message that gets a twin is stale", async () => {
    const body = "sequenceDiagram\n  a->>b: hi\n  b->>a: ok";
    const { outcome, text } = await run(
      body,
      { op: "deleteMessage", target: message("b", "a", "ok") },
      (view) => insertBefore(view, "  a->>b: hi", "  b->>a: ok\n"),
    );
    expect(outcome).toEqual({ applied: false, refused: "stale" });
    expect(text).toBe(fenced(body.replace("  a->>b", "  b->>a: ok\n  a->>b")));
  });

  test("a sole message stays itself through a peer's edit elsewhere", async () => {
    const body =
      "sequenceDiagram\n  participant a\n  participant b\n  a->>b: hi\n  b->>a: ok";
    const { outcome, text } = await run(
      body,
      {
        op: "arrowStyle",
        target: message("b", "a", "ok"),
        line: "dotted",
      },
      (view) => insertBefore(view, "  b->>a: ok", "  a->>b: more\n"),
    );
    expect(outcome).toEqual({ applied: true });
    expect(text).toBe(
      fenced(body.replace("  b->>a: ok", "  a->>b: more\n  b-->>a: ok")),
    );
  });

  test("a message whose text a peer changed is stale, not gone", async () => {
    const body = "sequenceDiagram\n  a->>b: hi\n  b->>a: ok";
    const { outcome } = await run(
      body,
      { op: "renameMessage", target: message("b", "a", "ok"), text: "yes" },
      (view) => {
        const at = bytes(view.state).indexOf("ok");
        view.dispatch({ changes: { from: at, to: at + 2, insert: "fine" } });
      },
    );
    expect(outcome).toEqual({ applied: false, refused: "stale" });
  });

  test("a message a peer deleted with its pair is gone", async () => {
    const body = "sequenceDiagram\n  a->>b: hi\n  b->>a: ok";
    const { outcome } = await run(
      body,
      { op: "renameMessage", target: message("b", "a", "ok"), text: "yes" },
      (view) => {
        deleteText(view, "\n  b->>a: ok");
      },
    );
    expect(outcome).toEqual({ applied: false, refused: "gone" });
  });

  test("a message whose sender a peer renamed is stale, as its text is left", async () => {
    const body =
      "sequenceDiagram\n  participant a as Alice\n  participant b\n  a->>b: hi";
    const { outcome } = await run(
      body,
      { op: "renameMessage", target: message("a", "b", "hi"), text: "yes" },
      (view) => {
        const doc = bytes(view.state);
        const decl = doc.indexOf("participant a ") + "participant ".length;
        const line = doc.indexOf("  a->>b") + 2;
        view.dispatch({
          changes: [
            { from: decl, to: decl + 1, insert: "al" },
            { from: line, to: line + 1, insert: "al" },
          ],
        });
      },
    );
    expect(outcome).toEqual({ applied: false, refused: "stale" });
  });

  const LOOP =
    "sequenceDiagram\n  participant a\n  participant b\n  loop every minute\n    a->>b: ping\n  end";

  test("a block a peer put above is not the one removed", async () => {
    const op: SeqOp = { op: "removeBlock", block: 0 };
    const peer = (view: EditorView) =>
      insertBefore(view, "  loop every", "  opt first\n    b->>a: x\n  end\n");
    const plain = await run(LOOP, op, peer, false);
    // The plain op takes the peer's opt.
    expect(plain.outcome).toEqual({ applied: true });
    expect(plain.text).toContain("  loop every minute\n");
    const held = await run(LOOP, op, peer);
    expect(held.outcome).toEqual({ applied: true });
    expect(held.text).toBe(
      fenced(
        LOOP.replace(
          "  loop every minute\n    a->>b: ping\n  end",
          "  opt first\n    b->>a: x\n  end\n    a->>b: ping",
        ),
      ),
    );
  });

  test("a block number that was not there when read stays gone", async () => {
    const twice = `${LOOP}\n  opt later\n    b->>a: pong\n  end`;
    const view = markView(fenced(twice), { raw: true });
    const outcome = await runDiagramOp(
      view,
      0,
      { ...seqKind, verify: () => Promise.resolve(OK) },
      heldSeqOp({ op: "removeBlock", block: 1 }, parseSequence(LOOP)),
    );
    expect(outcome).toEqual({ applied: false, refused: "gone" });
    expect(bytes(view.state)).toBe(fenced(twice));
  });

  test("a block a peer put above in the same words is stale", async () => {
    const { outcome } = await run(
      LOOP,
      { op: "removeBlock", block: 0 },
      (view) =>
        insertBefore(
          view,
          "  loop every",
          "  loop every minute\n    b->>a: x\n  end\n",
        ),
    );
    expect(outcome).toEqual({ applied: false, refused: "stale" });
  });

  test("a block whose title a peer changed is stale", async () => {
    const { outcome } = await run(
      LOOP,
      { op: "removeBlock", block: 0 },
      (view) => {
        const at = bytes(view.state).indexOf("minute");
        view.dispatch({ changes: { from: at, to: at + 6, insert: "hour" } });
      },
    );
    expect(outcome).toEqual({ applied: false, refused: "stale" });
  });

  test("one of two twins a peer deleted is stale, as either may be left", async () => {
    const { outcome } = await run(
      TWINS,
      { op: "renameMessage", target: message("a", "b", "hi", 1), text: "yo" },
      (view) => {
        deleteText(view, "\n  a->>b: hi");
      },
    );
    expect(outcome).toEqual({ applied: false, refused: "stale" });
  });

  test("one of two blocks in the same words a peer deleted is stale", async () => {
    const twice = `${LOOP}\n  loop every minute\n    b->>a: pong\n  end`;
    const { outcome } = await run(
      twice,
      { op: "removeBlock", block: 1 },
      (view) =>
        deleteText(view, "  loop every minute\n    a->>b: ping\n  end\n"),
    );
    expect(outcome).toEqual({ applied: false, refused: "stale" });
  });

  test("a body read before the run counts as the first reading", async () => {
    const body = "sequenceDiagram\n  a->>b: hi\n  b->>a: ok";
    const view = markView(fenced(`${body}\n  b->>a: ok`), { raw: true });
    const outcome = await runDiagramOp(
      view,
      0,
      { ...seqKind, verify: () => Promise.resolve(OK) },
      heldSeqOp(
        { op: "deleteMessage", target: message("b", "a", "ok") },
        parseSequence(body),
      ),
    );
    expect(outcome).toEqual({ applied: false, refused: "stale" });
  });
});

describe("heldSeqOp with a place in the text", () => {
  const TWINS =
    "sequenceDiagram\n  participant a\n  participant b\n  a->>b: hi\n  b->>a: ok\n  a->>b: hi";

  /**
   * Runs `op` held at the arrow of the `nth` line that reads `line` (from 0), a
   * ticket of `diagramAnchors` standing inside that token as a hold puts it.
   */
  async function runAt(
    body: string,
    line: string,
    nth: number,
    op: SeqOp,
    peer: (view: EditorView) => void,
  ) {
    const doc = fenced(body);
    const view = markView(doc, { raw: true });
    let at = -1;
    for (let i = 0; i <= nth; i++) {
      at = doc.indexOf(line, at + 1);
    }
    const arrow = doc.indexOf("-", at);
    const ticket = 900_001;
    view.dispatch({ effects: trackAnchor.of({ ticket, pos: arrow + 1 }) });
    const place = (bodyFrom: number) => {
      const pos = view.state.field(diagramAnchors).get(ticket);
      return pos === undefined ? null : pos - bodyFrom;
    };
    const outcome = await runDiagramOp(
      view,
      0,
      withPeer(() => view, peer),
      heldSeqOp(op, parseSequence(body), place),
    );
    return { outcome, text: bytes(view.state) };
  }

  test("a peer's twin above a twin: the held one is renamed, not stale", async () => {
    const { outcome, text } = await runAt(
      TWINS,
      "  a->>b: hi",
      1,
      { op: "renameMessage", target: message("a", "b", "hi", 1), text: "yo" },
      (view) => insertBefore(view, "  a->>b: hi", "  a->>b: hi\n"),
    );
    expect(outcome).toEqual({ applied: true });
    expect(text).toBe(
      fenced(
        "sequenceDiagram\n  participant a\n  participant b\n  a->>b: hi\n  a->>b: hi\n  b->>a: ok\n  a->>b: yo",
      ),
    );
  });

  test("two texts a peer swapped in place are stale, where the words would take the other line", async () => {
    const body =
      "sequenceDiagram\n  participant a\n  participant b\n  a->>b: hi\n  a->>b: hello";
    const swap = (view: EditorView) => {
      const doc = bytes(view.state);
      const first = doc.indexOf(": hi") + 2;
      const second = doc.indexOf(": hello") + 2;
      view.dispatch({
        changes: [
          { from: first, to: first + 2, insert: "hello" },
          { from: second, to: second + 5, insert: "hi" },
        ],
      });
    };
    const op: SeqOp = {
      op: "deleteMessage",
      target: message("a", "b", "hi"),
    };
    const plain = markView(fenced(body), { raw: true });
    const words = {
      outcome: await runDiagramOp(
        plain,
        0,
        withPeer(() => plain, swap),
        heldSeqOp(op),
      ),
    };
    // By the words alone the line that now reads `hi` goes: the second one.
    expect(words.outcome).toEqual({ applied: true });
    const held = await runAt(body, "  a->>b: hi", 0, op, swap);
    expect(held.outcome).toEqual({ applied: false, refused: "stale" });
    expect(held.text).toBe(
      fenced(
        body
          .replace(": hello", ": HI")
          .replace(": hi", ": hello")
          .replace(": HI", ": hi"),
      ),
    );
  });

  test("a held message a peer deleted is stale while its pair has another", async () => {
    const { outcome } = await runAt(
      TWINS,
      "  a->>b: hi",
      0,
      { op: "deleteMessage", target: message("a", "b", "hi") },
      (view) => deleteText(view, "\n  a->>b: hi"),
    );
    expect(outcome).toEqual({ applied: false, refused: "stale" });
  });

  test("a held message a peer deleted is gone with no pair or text left", async () => {
    const body =
      "sequenceDiagram\n  participant a\n  participant b\n  a->>b: hi\n  b->>a: ok";
    const { outcome } = await runAt(
      body,
      "  a->>b: hi",
      0,
      { op: "deleteMessage", target: message("a", "b", "hi") },
      (view) => deleteText(view, "\n  a->>b: hi"),
    );
    expect(outcome).toEqual({ applied: false, refused: "gone" });
  });

  const LOOP =
    "sequenceDiagram\n  participant a\n  participant b\n  loop every minute\n    a->>b: ping\n  end";

  test("a peer's block above in the same words: the held block is removed, not stale", async () => {
    const doc = fenced(LOOP);
    const view = markView(doc, { raw: true });
    const ticket = 900_002;
    view.dispatch({
      effects: trackAnchor.of({ ticket, pos: doc.indexOf("loop") + 1 }),
    });
    const outcome = await runDiagramOp(
      view,
      0,
      withPeer(
        () => view,
        (v) =>
          insertBefore(
            v,
            "  loop every",
            "  loop every minute\n    b->>a: x\n  end\n",
          ),
      ),
      heldSeqOp(
        { op: "removeBlock", block: 0 },
        parseSequence(LOOP),
        (from) => {
          const pos = view.state.field(diagramAnchors).get(ticket);
          return pos === undefined ? null : pos - from;
        },
      ),
    );
    expect(outcome).toEqual({ applied: true });
    expect(bytes(view.state)).toBe(
      fenced(
        "sequenceDiagram\n  participant a\n  participant b\n  loop every minute\n    b->>a: x\n  end\n    a->>b: ping",
      ),
    );
  });
});
