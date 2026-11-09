/**
 * The menus of a drawn sequence diagram and its connect mode, on the real surface
 * and the real verdict, with only mermaid's render stubbed. Each item is compared
 * with what the op compiler makes of the same body, so a menu can only be as wrong
 * as the op tests allow. `actions.marks-hidden.test.ts` and
 * `actions.marks-show.test.ts` run this again.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EditorState, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

import { cellMarksMode } from "../../../test/cellMarksMode";
import type { MarkViewOptions } from "../../../test/markKit";
import {
  bytes,
  cleanupMarkViews,
  markPeers,
  markView,
  setCaret,
  undoOnce,
  undoSteps,
} from "../../../test/markKit";
import { MERMAID_STARTER_GROUPS, mermaidFence } from "../../mermaidStarters";
import { insertBlock } from "../../toolbar";
import { drawnFenceLines } from "../draws";
import { sequenceDrawing } from "../../../test/sequenceDrawing";
import { seqKind } from "../apply";
import { applyEdits, NOTICES } from "../edits";
import { focusTarget } from "../focus";
import { addWithName } from "../labelEditor";
import { roomUndo } from "../../roomUndo";
import type { TargetKey } from "../intent";
import { diagramIntent } from "../intent";
import { prefetchDiagrams } from "../loader";
import { forgetDrawings } from "../widget";
import type { SeqTarget } from "./model";
import type { SeqOp } from "./ops";
import { compileSeqOp } from "./ops";
import { parseSequence } from "./parse";

/** How long each render waits: a test may slow the drawing down behind the text. */
const kit = vi.hoisted(() => ({ renderMs: 0 }));

vi.mock("mermaid", async (original) => {
  const real = (await original<{ default: object }>()).default;
  return {
    default: {
      ...real,
      render: async (id: string, body: string) => {
        if (kit.renderMs > 0) {
          await new Promise((resolve) => setTimeout(resolve, kit.renderMs));
        }
        return { svg: sequenceDrawing(id, body) };
      },
    },
  };
});

const STARTER =
  "sequenceDiagram\n    participant caller as Caller\n    participant service as Service\n    caller->>service: Request\n    service-->>caller: Reply";
const fence = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\``;
const page = (body: string) => `Above\n${fence(body)}\nBelow\n`;

type MessageTarget = SeqTarget & { kind: "message" };
const message = (
  from: string,
  to: string,
  text: string,
  occurrence = 0,
): MessageTarget => ({ kind: "message", from, to, text, occurrence });
const CALLER: TargetKey = { kind: "participant", name: "caller" };
const REQUEST = message("caller", "service", "Request");
const REPLY = message("service", "caller", "Reply");
const CANVAS: TargetKey = { kind: "canvas" };

const flush = async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};

function modeOptions(): MarkViewOptions {
  const mode = cellMarksMode();
  return mode === "off" ? {} : { mode };
}

async function drawn(doc = page(STARTER)) {
  const view = markView(doc, modeOptions());
  await prefetchDiagrams();
  await vi.waitFor(() => {
    expect(view.dom.querySelector(".cm-diagram svg")).not.toBeNull();
  });
  await flush();
  await flush();
  view.contentDOM.focus();
  return view;
}

const text = (view: EditorView) => bytes(view.state);
const active = () => document.activeElement as HTMLElement;
const fromOf = (view: EditorView) => text(view).indexOf("```");
const menus = () => [...document.querySelectorAll('[role="menu"]')];
const fieldOf = (view: EditorView) =>
  view.dom.querySelector<HTMLTextAreaElement>(".cm-diagram-overlay textarea");
const noticeOf = (view: EditorView) =>
  view.dom.querySelector(".cm-diagram-notice")?.textContent ?? "";
const promptOf = (view: EditorView) =>
  view.dom.querySelector('.cm-diagram-overlay [role="status"]');

function press(key: string, options: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key,
    bubbles: true,
    cancelable: true,
    ...options,
  });
  active().dispatchEvent(event);
  return event;
}

function openMenuOn(view: EditorView, target: TargetKey) {
  focusTarget(view, fromOf(view), target);
  press("F10", { shiftKey: true });
  const menu = menus()[0];
  if (menu === undefined) throw new Error("no menu");
  return menu;
}

function row(label: string): HTMLElement {
  const found = [...document.querySelectorAll('[role^="menuitem"]')].find(
    (el) => el.textContent === label,
  );
  if (found === undefined) throw new Error(`no item ${label}`);
  return found as HTMLElement;
}

const choose = (...path: string[]) => {
  for (const label of path) {
    row(label).click();
  }
};

const labels = (menu: Element) =>
  [...menu.querySelectorAll(':scope > [role^="menuitem"]')].map(
    (el) => el.textContent,
  );

/** What the op compiler makes of `body`, on the page. */
function expected(body: string, op: SeqOp): string {
  const result = compileSeqOp(parseSequence(body), op);
  if (!("edits" in result)) throw new Error(`refused ${result.refused}`);
  return page(applyEdits(body, result.edits));
}

async function changed(view: EditorView, from: string) {
  await vi.waitFor(() => {
    expect(text(view)).not.toBe(from);
  });
}

/** Types `value` into the open field, as a person replacing its selection would. */
function typeInField(view: EditorView, value: string) {
  const field = fieldOf(view);
  if (field === null) throw new Error("no field");
  field.value = value;
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

beforeEach(() => {
  forgetDrawings();
  kit.renderMs = 0;
});

afterEach(() => {
  cleanupMarkViews();
  document.body.replaceChildren();
});

describe("what the menus hold", () => {
  it.each<[string, TargetKey, string, string[]]>([
    [
      "a participant",
      CALLER,
      "Participant actions",
      ["Rename", "Add participant", "Add message to...", "Delete"],
    ],
    [
      "a message",
      REQUEST,
      "Message actions",
      [
        "Edit text",
        "Move up",
        "Move down",
        "Line",
        "Arrowhead",
        "Delete",
        "Remove block",
      ],
    ],
    [
      "the canvas",
      CANVAS,
      "Diagram actions",
      ["Add participant", "Delete diagram"],
    ],
  ])("%s has its own menu", async (_n, target, name, items) => {
    const view = await drawn();
    const menu = openMenuOn(view, target);
    expect(menu.getAttribute("aria-label")).toBe(name);
    expect(labels(menu)).toEqual(items);
  });

  it("Line and Arrowhead say what the message has", async () => {
    const view = await drawn();
    openMenuOn(view, REPLY);
    row("Line").click();
    expect(row("Dotted").getAttribute("aria-checked")).toBe("true");
    expect(row("Solid").getAttribute("aria-checked")).toBe("false");
    row("Arrowhead").click();
    const heads = ["Arrow", "Open", "Cross", "Async", "Both ends"];
    expect(heads.map((h) => row(h).getAttribute("aria-checked"))).toEqual([
      "true",
      "false",
      "false",
      "false",
      "false",
    ]);
  });
});

describe("each item is the op's text and one undo step", () => {
  it.each<[string, TargetKey, string[], SeqOp]>([
    [
      "Line Dotted",
      REQUEST,
      ["Line", "Dotted"],
      { op: "arrowStyle", target: REQUEST, line: "dotted" },
    ],
    [
      "Arrowhead Cross",
      REQUEST,
      ["Arrowhead", "Cross"],
      { op: "arrowStyle", target: REQUEST, head: "cross" },
    ],
    [
      "Arrowhead Open",
      REQUEST,
      ["Arrowhead", "Open"],
      { op: "arrowStyle", target: REQUEST, head: "open" },
    ],
    [
      "Arrowhead Async",
      REQUEST,
      ["Arrowhead", "Async"],
      { op: "arrowStyle", target: REQUEST, head: "async" },
    ],
    [
      "Arrowhead Both ends",
      REQUEST,
      ["Arrowhead", "Both ends"],
      { op: "arrowStyle", target: REQUEST, head: "both" },
    ],
    [
      "Delete a message",
      REPLY,
      ["Delete"],
      { op: "deleteMessage", target: REPLY },
    ],
    [
      "Delete a participant",
      CALLER,
      ["Delete"],
      { op: "deleteParticipant", name: "caller" },
    ],
  ])("%s", async (_n, target, path, op) => {
    const view = await drawn();
    const before = text(view);
    const steps = undoSteps(view);
    openMenuOn(view, target);
    choose(...path);
    await changed(view, before);
    expect(text(view)).toBe(expected(STARTER, op));
    expect(undoSteps(view)).toBe(steps + 1);
    undoOnce(view);
    expect(text(view)).toBe(before);
    expect(menus()).toHaveLength(0);
  });

  it("Line Dotted on Request is exactly the dotted arrow", async () => {
    const view = await drawn();
    openMenuOn(view, REQUEST);
    choose("Line", "Dotted");
    await changed(view, page(STARTER));
    expect(text(view)).toBe(
      page(STARTER.replace("caller->>service", "caller-->>service")),
    );
  });

  it("Delete on Reply takes that line", async () => {
    const view = await drawn();
    openMenuOn(view, REPLY);
    choose("Delete");
    await changed(view, page(STARTER));
    expect(text(view)).toBe(
      page(STARTER.replace("\n    service-->>caller: Reply", "")),
    );
  });

  it("Rename on caller writes Client in place of Caller only", async () => {
    const view = await drawn();
    const steps = undoSteps(view);
    openMenuOn(view, CALLER);
    choose("Rename");
    await flush();
    const field = fieldOf(view);
    expect(field?.getAttribute("aria-label")).toBe("Participant name");
    expect(field?.value).toBe("Caller");
    expect(active()).toBe(field);
    typeInField(view, "Client");
    press("Enter");
    await changed(view, page(STARTER));
    expect(text(view)).toBe(page(STARTER.replace("as Caller", "as Client")));
    expect(undoSteps(view)).toBe(steps + 1);
  });

  it("Edit text on Request writes Ask in place of Request only", async () => {
    const view = await drawn();
    openMenuOn(view, REQUEST);
    choose("Edit text");
    await flush();
    const field = fieldOf(view);
    expect(field?.getAttribute("aria-label")).toBe("Message text");
    expect(field?.value).toBe("Request");
    typeInField(view, "Ask");
    press("Enter");
    await changed(view, page(STARTER));
    expect(text(view)).toBe(page(STARTER.replace(": Request", ": Ask")));
    // The stop stays on the message it renamed, under its new text.
    await vi.waitFor(() => {
      expect(view.state.field(diagramIntent)?.target).toEqual(
        message("caller", "service", "Ask"),
      );
    });
    await vi.waitFor(() => {
      expect(active().getAttribute("aria-label")).toBe(
        "Message from Caller to Service: Ask",
      );
    });
  });

  it.each<[string, TargetKey]>([
    ["a participant's", CALLER],
    ["the canvas's", CANVAS],
  ])(
    "Add participant from %s menu writes the declaration and opens the field on its name",
    async (_n, target) => {
      const view = await drawn();
      const steps = undoSteps(view);
      openMenuOn(view, target);
      choose("Add participant");
      await changed(view, page(STARTER));
      expect(text(view)).toBe(
        page(
          STARTER.replace(
            "as Service\n",
            "as Service\n    participant p1 as New participant\n",
          ),
        ),
      );
      expect(undoSteps(view)).toBe(steps + 1);
      await vi.waitFor(() => {
        expect(fieldOf(view)).not.toBeNull();
      });
      expect(fieldOf(view)?.getAttribute("aria-label")).toBe(
        "Participant name",
      );
      expect(fieldOf(view)?.value).toBe("New participant");
      expect(active()).toBe(fieldOf(view));
      press("Escape");
      await flush();
      expect(noticeOf(view)).toBe(NOTICES.keptParticipant);
      undoOnce(view);
      expect(text(view)).toBe(page(STARTER));
    },
  );

  it("Delete diagram takes the whole fence", async () => {
    const view = await drawn();
    openMenuOn(view, CANVAS);
    choose("Delete diagram");
    await flush();
    expect(text(view)).toBe("Above\nBelow\n");
    undoOnce(view);
    expect(text(view)).toBe(page(STARTER));
  });
});

describe("Add message to...", () => {
  it("from caller by keys: ArrowRight, Enter, Ping, Enter appends the message in two undo steps", async () => {
    const view = await drawn();
    const steps = undoSteps(view);
    focusTarget(view, fromOf(view), CALLER);
    press("F10", { shiftKey: true });
    expect(active().textContent).toBe("Rename");
    press("ArrowDown");
    press("ArrowDown");
    expect(active().textContent).toBe("Add message to...");
    press("Enter");
    await flush();
    expect(view.state.field(diagramIntent)?.mode).toBe("connect");
    expect(promptOf(view)?.textContent).toBe(
      "Choose the participant to send the message to. Press Escape to cancel.",
    );
    press("ArrowRight");
    expect(view.state.field(diagramIntent)?.target).toEqual({
      kind: "participant",
      name: "service",
    });
    press("Enter");
    await changed(view, page(STARTER));
    expect(text(view)).toBe(
      page(`${STARTER}\n    caller->>service: New message`),
    );
    await vi.waitFor(() => {
      expect(fieldOf(view)).not.toBeNull();
    });
    expect(fieldOf(view)?.getAttribute("aria-label")).toBe("Message text");
    expect(active()).toBe(fieldOf(view));
    typeInField(view, "Ping");
    press("Enter");
    await vi.waitFor(() => {
      expect(text(view)).toBe(page(`${STARTER}\n    caller->>service: Ping`));
    });
    expect(undoSteps(view)).toBe(steps + 2);
    undoOnce(view);
    expect(text(view)).toBe(
      page(`${STARTER}\n    caller->>service: New message`),
    );
    undoOnce(view);
    expect(text(view)).toBe(page(STARTER));
  });

  it("Enter on its own participant sends a message to itself", async () => {
    const view = await drawn();
    openMenuOn(view, CALLER);
    choose("Add message to...");
    await flush();
    press("Enter");
    await changed(view, page(STARTER));
    expect(text(view)).toBe(
      expected(STARTER, { op: "addMessage", from: "caller", to: "caller" }),
    );
    await vi.waitFor(() => {
      expect(fieldOf(view)).not.toBeNull();
    });
  });

  it("runs the sequence kind's own check", async () => {
    const view = await drawn();
    const own = seqKind.verify.bind(seqKind);
    seqKind.verify = () =>
      Promise.resolve({ ok: false, reason: "the sequence check says no" });
    try {
      openMenuOn(view, CALLER);
      choose("Add message to...");
      await flush();
      press("ArrowRight");
      press("Enter");
      await vi.waitFor(() => {
        expect(noticeOf(view)).toBe(NOTICES.invalid);
      });
      expect(text(view)).toBe(page(STARTER));
    } finally {
      seqKind.verify = own;
    }
  });

  it("Escape on the new message's text keeps the message, and says so once", async () => {
    const view = await drawn();
    openMenuOn(view, CALLER);
    choose("Add message to...");
    await flush();
    press("ArrowRight");
    press("Enter");
    await vi.waitFor(() => {
      expect(fieldOf(view)).not.toBeNull();
    });
    press("Escape");
    await flush();
    expect(fieldOf(view)).toBeNull();
    expect(noticeOf(view)).toBe(NOTICES.keptMessage);
    expect(text(view)).toBe(
      page(`${STARTER}\n    caller->>service: New message`),
    );
    expect(view.state.field(diagramIntent)?.target).toEqual(
      message("caller", "service", "New message"),
    );
  });

  it("Escape on a field that only renames says nothing", async () => {
    const view = await drawn();
    openMenuOn(view, CALLER);
    choose("Rename");
    await flush();
    press("Escape");
    await flush();
    expect(noticeOf(view)).toBe("");
  });

  it("to a participant that has ended says so and writes nothing", async () => {
    const body =
      "sequenceDiagram\n    participant host\n    participant worker\n    destroy worker\n    host-xworker: stop";
    const view = await drawn(page(body));
    openMenuOn(view, { kind: "participant", name: "host" });
    choose("Add message to...");
    await flush();
    press("ArrowRight");
    press("Enter");
    await vi.waitFor(() => {
      expect(noticeOf(view)).toBe(NOTICES.participantEnded);
    });
    expect(text(view)).toBe(page(body));
  });

  it("Escape ends it and writes nothing", async () => {
    const view = await drawn();
    openMenuOn(view, CALLER);
    choose("Add message to...");
    await flush();
    expect(view.state.field(diagramIntent)?.mode).toBe("connect");
    press("Escape");
    await flush();
    expect(view.state.field(diagramIntent)?.mode).toBe("focus");
    expect(view.state.field(diagramIntent)?.target).toEqual(CALLER);
    expect(text(view)).toBe(page(STARTER));
  });

  it("walks only the participants", async () => {
    const view = await drawn();
    openMenuOn(view, CALLER);
    choose("Add message to...");
    await flush();
    press("ArrowRight");
    press("ArrowRight");
    expect(view.state.field(diagramIntent)?.target).toEqual({
      kind: "participant",
      name: "service",
    });
    press("ArrowLeft");
    press("ArrowLeft");
    expect(view.state.field(diagramIntent)?.target).toEqual(CALLER);
  });
});

describe("a name the source cannot take", () => {
  it("a participant whose name has a space opens no field and says why", async () => {
    const body = "sequenceDiagram\n    Alice Smith->>Bob: hi";
    const view = await drawn(page(body));
    focusTarget(view, fromOf(view), {
      kind: "participant",
      name: "Alice Smith",
    });
    press("Enter");
    await flush();
    expect(fieldOf(view)).toBeNull();
    expect(noticeOf(view)).toBe(NOTICES.unsupported);
    expect(text(view)).toBe(page(body));
  });
});

describe("a delete the source cannot take", () => {
  it("a message alone in a block is offered disabled, with the sentence", async () => {
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    loop every minute\n        a->>b: ping\n    end";
    const view = await drawn(page(body));
    openMenuOn(view, message("a", "b", "ping"));
    expect(row("Delete").getAttribute("aria-disabled")).toBe("true");
    expect(row("Delete").getAttribute("aria-description")).toBe(
      "That would leave a block with nothing in it. Use Remove block first.",
    );
    expect(row("Remove block").getAttribute("aria-disabled")).toBeNull();
  });

  it("a message whose - closes an activate line is offered disabled, with the sentence", async () => {
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    activate b\n    b-->>-a: done\n    a->>b: more";
    const view = await drawn(page(body));
    openMenuOn(view, message("b", "a", "done"));
    expect(row("Delete").getAttribute("aria-disabled")).toBe("true");
    expect(row("Delete").getAttribute("aria-description")).toBe(
      "That would leave an activation open.",
    );
  });

  it("a participant whose only message there fills a block names Remove block on that message", async () => {
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    a->>b: hi\n    loop every minute\n        b->>a: ping\n    end";
    const view = await drawn(page(body));
    openMenuOn(view, { kind: "participant", name: "b" });
    expect(row("Delete").getAttribute("aria-disabled")).toBe("true");
    expect(row("Delete").getAttribute("aria-description")).toBe(
      "That would leave a block with nothing in it. Use Remove block on the message in that block first.",
    );
  });

  it("a + and its - go together, as one undo step", async () => {
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    a->>+b: call\n    b->>b: work\n    b-->>-a: done";
    const view = await drawn(page(body));
    const steps = undoSteps(view);
    openMenuOn(view, message("b", "a", "done"));
    expect(row("Delete").getAttribute("aria-disabled")).toBeNull();
    choose("Delete");
    await changed(view, page(body));
    expect(text(view)).toBe(
      page(
        "sequenceDiagram\n    participant a\n    participant b\n    a->>b: call\n    b->>b: work",
      ),
    );
    expect(undoSteps(view)).toBe(steps + 1);
    undoOnce(view);
    expect(text(view)).toBe(page(body));
  });

  it("Remove block takes the block's own lines, and then Delete takes the message", async () => {
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    loop every minute\n        a->>b: ping\n    end";
    const view = await drawn(page(body));
    const steps = undoSteps(view);
    openMenuOn(view, message("a", "b", "ping"));
    choose("Remove block");
    await changed(view, page(body));
    const removed = expected(body, { op: "removeBlock", block: 0 });
    expect(text(view)).toBe(removed);
    expect(undoSteps(view)).toBe(steps + 1);
    await flush();
    openMenuOn(view, message("a", "b", "ping"));
    expect(row("Delete").getAttribute("aria-disabled")).toBeNull();
    choose("Delete");
    await changed(view, removed);
    expect(text(view)).not.toContain("ping");
  });

  it("Remove block is disabled on a message in no block", async () => {
    const view = await drawn();
    openMenuOn(view, REQUEST);
    expect(row("Remove block").getAttribute("aria-disabled")).toBe("true");
    expect(row("Remove block").getAttribute("aria-description")).toBe(
      "This message is not in a block.",
    );
  });
});

describe("a message held by its place with a peer", () => {
  const TWINS =
    "sequenceDiagram\n    participant a\n    participant b\n    a->>b: hi\n    a->>b: hi";

  async function room(body: string) {
    const peers = markPeers(page(body), modeOptions());
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(peers.local.dom.querySelector(".cm-diagram svg")).not.toBeNull();
    });
    await flush();
    return peers;
  }

  const settle = async () => {
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 30));
    await flush();
  };

  it("a twin a peer adds above: Delete takes mine, not the new one", async () => {
    const { local, peer, sync } = await room(TWINS);
    openMenuOn(local, message("a", "b", "hi", 1));
    const at = text(peer).indexOf("    a->>b: hi");
    peer.dispatch({ changes: { from: at, insert: "    a->>b: hi\n" } });
    sync();
    await settle();
    expect(menus()).toHaveLength(1);
    row("Line").click();
    choose("Dotted");
    await vi.waitFor(() => {
      expect(text(local)).toContain("-->>");
    });
    expect(text(local)).toBe(
      page(
        "sequenceDiagram\n    participant a\n    participant b\n    a->>b: hi\n    a->>b: hi\n    a-->>b: hi",
      ),
    );
  });

  it("Escape on a new message a peer changed meanwhile says nothing, and the stop is the canvas", async () => {
    const { local, peer, sync } = await room(STARTER);
    openMenuOn(local, CALLER);
    choose("Add message to...");
    await flush();
    press("ArrowRight");
    press("Enter");
    await vi.waitFor(() => {
      expect(fieldOf(local)).not.toBeNull();
    });
    sync();
    await settle();
    const doc = text(peer);
    const arrow =
      doc.indexOf("caller->>service: New message") + "caller".length;
    peer.dispatch({
      changes: { from: arrow, to: arrow + 3, insert: "-->>" },
    });
    sync();
    await settle();
    press("Escape");
    await settle();
    expect(local.state.field(diagramIntent)?.target).toEqual(CANVAS);
    expect(noticeOf(local)).not.toBe(NOTICES.keptMessage);
  });

  it("two people adding a participant at once each get their own, not one declared twice", async () => {
    const { local, peer, sync } = await room(STARTER);
    await vi.waitFor(() => {
      expect(peer.dom.querySelector(".cm-diagram svg")).not.toBeNull();
    });
    await settle();
    await Promise.all([
      addWithName(local, fromOf(local), { op: "addParticipant" }),
      addWithName(peer, fromOf(peer), { op: "addParticipant" }),
    ]);
    sync();
    await settle();
    expect(text(peer)).toBe(text(local));
    const ids = [
      ...text(local).matchAll(/participant (p\d+) as New participant/g),
    ].map((m) => m[1]);
    // Each starts from its own room client: the two ids differ unless those two
    // numbers happen to meet, one time in a thousand, which this does not count on.
    const own = (view: EditorView) =>
      `p${String(1 + ((view.state.facet(roomUndo)?.doc.clientID ?? 0) % 1000))}`;
    expect(ids.sort()).toEqual([own(local), own(peer)].sort());
    if (own(local) !== own(peer)) {
      expect(new Set(ids).size).toBe(2);
      const model = parseSequence(
        text(local).slice(
          text(local).indexOf("sequenceDiagram"),
          text(local).lastIndexOf("\n```"),
        ),
      );
      expect(model.order).toHaveLength(4);
    }
  });

  it("the Delete key on a stop holds its message through the verify", async () => {
    // Twins by ends and text, told apart here only by their arrows.
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    a->>b: hi\n    a-->>b: hi";
    const { local, peer, sync } = await room(body);
    focusTarget(local, fromOf(local), message("a", "b", "hi", 1));
    press("Delete");
    const at = text(peer).indexOf("    a->>b: hi");
    peer.dispatch({ changes: { from: at, insert: "    a->>b: hi\n" } });
    sync();
    await vi.waitFor(() => {
      expect(text(local)).not.toContain("-->>");
    });
    expect(text(local)).toBe(
      page(
        "sequenceDiagram\n    participant a\n    participant b\n    a->>b: hi\n    a->>b: hi",
      ),
    );
  });

  it("Edit text on a twin a peer added one above lands on mine", async () => {
    const { local, peer, sync } = await room(TWINS);
    openMenuOn(local, message("a", "b", "hi", 1));
    choose("Edit text");
    await flush();
    const at = text(peer).indexOf("    a->>b: hi");
    peer.dispatch({ changes: { from: at, insert: "    a->>b: hi\n" } });
    sync();
    await settle();
    typeInField(local, "yo");
    press("Enter");
    await vi.waitFor(() => {
      expect(text(local)).toContain("yo");
    });
    expect(text(local)).toBe(
      page(
        "sequenceDiagram\n    participant a\n    participant b\n    a->>b: hi\n    a->>b: hi\n    a->>b: yo",
      ),
    );
  });

  it("a field on a message a peer rewrote in place says so and writes nothing", async () => {
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    a->>b: hi\n    a->>b: hello";
    const { local, peer, sync } = await room(body);
    openMenuOn(local, message("a", "b", "hi"));
    choose("Edit text");
    await flush();
    const doc = text(peer);
    const first = doc.indexOf(": hi") + 2;
    const second = doc.indexOf(": hello") + 2;
    peer.dispatch({
      changes: [
        { from: first, to: first + 2, insert: "hello" },
        { from: second, to: second + 5, insert: "hi" },
      ],
    });
    sync();
    await settle();
    const now = text(local);
    typeInField(local, "yo");
    press("Enter");
    await settle();
    expect(text(local)).toBe(now);
    expect(noticeOf(local)).toBe(NOTICES.messageChanged);
    expect(fieldOf(local)?.getAttribute("aria-invalid")).toBe("true");
  });

  it("a stop on a message a peer rewrote in place moves to its sender, and the next Delete does nothing", async () => {
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    a->>b: hi\n    a->>b: hello";
    const { local, peer, sync } = await room(body);
    focusTarget(local, fromOf(local), message("a", "b", "hi"));
    await settle();
    const doc = text(peer);
    const first = doc.indexOf(": hi") + 2;
    const second = doc.indexOf(": hello") + 2;
    peer.dispatch({
      changes: [
        { from: first, to: first + 2, insert: "hello" },
        { from: second, to: second + 5, insert: "hi" },
      ],
    });
    sync();
    await settle();
    expect(local.state.field(diagramIntent)?.target).toEqual({
      kind: "participant",
      name: "a",
    });
    expect(noticeOf(local)).toBe(NOTICES.stale);
    const now = text(local);
    press("Delete");
    await settle();
    expect(text(local)).toBe(now);
  });
});

describe("the Sequence starter", () => {
  it("opens the field on Caller once its verdict settles, and the name typed lands", async () => {
    const view = markView("Intro\n", modeOptions());
    await prefetchDiagrams();
    view.contentDOM.focus();
    const starter = MERMAID_STARTER_GROUPS[0]?.starters.find(
      (s) => s.label === "Sequence",
    );
    if (starter === undefined) throw new Error("no starter");
    setCaret(view, 5);
    const { lines, select } = mermaidFence(starter);
    insertBlock(view, lines, select);
    await vi.waitFor(() => {
      expect(fieldOf(view)).not.toBeNull();
    });
    const field = fieldOf(view);
    expect(field?.getAttribute("aria-label")).toBe("Participant name");
    expect(field?.value).toBe("Caller");
    expect(active()).toBe(field);
    expect(view.state.facet(drawnFenceLines)).toHaveLength(1);
    typeInField(view, "Client");
    press("Enter");
    await vi.waitFor(() => {
      expect(text(view)).toContain("    participant caller as Client\n");
    });
  });
});

describe("a key with the box away", () => {
  const LONG = `${page(STARTER)}${"Line of text\n\n".repeat(8000)}`;
  const awayNow = async (view: EditorView) => {
    view.dispatch({
      effects: EditorView.scrollIntoView(view.state.doc.length),
    });
    (view as unknown as { measure(): void }).measure();
    expect(view.dom.querySelector(".cm-diagram")).toBeNull();
    (document.activeElement as HTMLElement | null)?.blur();
    await flush();
  };
  const back = async (view: EditorView) => {
    (view as unknown as { measure(): void }).measure();
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-diagram")).not.toBeNull();
    });
    await flush();
  };
  const lineOf = (view: EditorView, record: number) =>
    view.dom.querySelector(`[data-et="message"][data-id="i${String(record)}"]`);

  it.each(["Delete", "Enter"])(
    "%s on a message brings the stop back and does not act",
    async (key) => {
      const view = await drawn(LONG);
      focusTarget(view, fromOf(view), REPLY);
      await awayNow(view);
      const event = new KeyboardEvent("keydown", {
        key,
        bubbles: true,
        cancelable: true,
      });
      document.body.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(true);
      await back(view);
      expect(active()).toBe(lineOf(view, 1));
      expect(text(view)).toBe(LONG);
      expect(fieldOf(view)).toBeNull();
    },
  );

  it("Alt+ArrowUp on a message brings the stop back and moves nothing", async () => {
    const view = await drawn(LONG);
    focusTarget(view, fromOf(view), REPLY);
    await awayNow(view);
    const event = new KeyboardEvent("keydown", {
      key: "ArrowUp",
      altKey: true,
      bubbles: true,
      cancelable: true,
    });
    document.body.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    await back(view);
    expect(active()).toBe(lineOf(view, 1));
    await flush();
    expect(text(view)).toBe(LONG);
  });

  it("Add message to... waiting: Enter brings it back on its participant, still waiting", async () => {
    const view = await drawn(LONG);
    openMenuOn(view, CALLER);
    choose("Add message to...");
    await flush();
    await awayNow(view);
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        bubbles: true,
        cancelable: true,
      }),
    );
    await back(view);
    expect(view.state.field(diagramIntent)?.mode).toBe("connect");
    expect(promptOf(view)).not.toBeNull();
    expect(text(view)).toBe(LONG);
  });
});

describe("a page that takes no writes", () => {
  it("draws and walks, and opens no menu or field", async () => {
    const view = await drawn();
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    await flush();
    focusTarget(view, fromOf(view), CALLER);
    expect(active().getAttribute("aria-label")).toBe("Participant: Caller");
    expect(active().hasAttribute("aria-haspopup")).toBe(false);
    press("F10", { shiftKey: true });
    press("Enter");
    press("Delete");
    await flush();
    expect(menus()).toHaveLength(0);
    expect(fieldOf(view)).toBeNull();
    expect(text(view)).toBe(page(STARTER));
  });
});

describe("Move up and Move down", () => {
  const TWELVE = `sequenceDiagram\n    participant a\n    participant b\n${Array.from(
    { length: 12 },
    (_, i) => `    a->>b: m${String(i + 1)}`,
  ).join("\n")}`;
  const lineOf = (view: EditorView, text: string) =>
    [...view.dom.querySelectorAll('[data-et="message"]')].find(
      (el) => el.getAttribute("aria-label")?.endsWith(`: ${text}`) === true,
    );
  const altKey = (key: string, options: KeyboardEventInit = {}) =>
    press(key, { altKey: true, ...options });
  /** The redraw after a move: the drawing shows the text again. */
  const redrawn = async (view: EditorView, text: string) => {
    await vi.waitFor(() => {
      expect(lineOf(view, text)).toBe(active());
    });
    await flush();
  };

  it("the items are greyed at the top and the bottom, with the reason", async () => {
    const view = await drawn();
    openMenuOn(view, REQUEST);
    expect(row("Move up").getAttribute("aria-disabled")).toBe("true");
    expect(row("Move up").getAttribute("aria-description")).toBe(
      "This message is already at the top.",
    );
    expect(row("Move down").getAttribute("aria-disabled")).toBeNull();
    press("Escape");
    await flush();
    openMenuOn(view, REPLY);
    expect(row("Move down").getAttribute("aria-description")).toBe(
      "This message is already at the bottom.",
    );
  });

  it("Move down is one undo step, and the stop goes with the message", async () => {
    const view = await drawn();
    const steps = undoSteps(view);
    openMenuOn(view, REQUEST);
    choose("Move down");
    await changed(view, page(STARTER));
    expect(text(view)).toBe(
      page(
        "sequenceDiagram\n    participant caller as Caller\n    participant service as Service\n    service-->>caller: Reply\n    caller->>service: Request",
      ),
    );
    expect(undoSteps(view)).toBe(steps + 1);
    await redrawn(view, "Request");
    expect(view.state.field(diagramIntent)?.target).toEqual(REQUEST);
    undoOnce(view);
    expect(text(view)).toBe(page(STARTER));
  });

  it("Alt+ArrowUp moves the message on the stop, and the stop follows", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), REPLY);
    const event = altKey("ArrowUp");
    expect(event.defaultPrevented).toBe(true);
    await changed(view, page(STARTER));
    expect(text(view)).toBe(
      page(
        "sequenceDiagram\n    participant caller as Caller\n    participant service as Service\n    service-->>caller: Reply\n    caller->>service: Request",
      ),
    );
    await redrawn(view, "Reply");
    expect(view.state.field(diagramIntent)?.target).toEqual(REPLY);
  });

  it("twelve messages: the last goes to second place by ten presses, one undo each", async () => {
    const view = await drawn(page(TWELVE));
    const steps = undoSteps(view);
    focusTarget(view, fromOf(view), message("a", "b", "m12"));
    for (let i = 0; i < 10; i++) {
      const before = text(view);
      altKey("ArrowUp");
      await changed(view, before);
      await redrawn(view, "m12");
    }
    const lines = Array.from(
      { length: 12 },
      (_, i) => `    a->>b: m${String(i + 1)}`,
    );
    expect(text(view)).toBe(
      page(
        `sequenceDiagram\n    participant a\n    participant b\n${[lines[0], lines[11], ...lines.slice(1, 11)].join("\n")}`,
      ),
    );
    expect(undoSteps(view)).toBe(steps + 10);
    // One more press: second place to the top, and then it stops there.
    altKey("ArrowUp");
    await vi.waitFor(() => {
      expect(text(view)).toContain("participant b\n    a->>b: m12");
    });
    await redrawn(view, "m12");
    const top = text(view);
    altKey("ArrowUp");
    await flush();
    await flush();
    expect(text(view)).toBe(top);
    expect(noticeOf(view)).toBe(NOTICES.atTop);
  });

  it("eleven presses with no wait: the last of thirteen goes to second place, exact", async () => {
    const lines = Array.from(
      { length: 13 },
      (_, i) => `    a->>b: m${String(i + 1)}`,
    );
    const body = `sequenceDiagram\n    participant a\n    participant b\n${lines.join("\n")}`;
    const view = await drawn(page(body));
    const steps = undoSteps(view);
    focusTarget(view, fromOf(view), message("a", "b", "m13"));
    for (let i = 0; i < 11; i++) {
      altKey("ArrowUp");
    }
    const want = page(
      `sequenceDiagram\n    participant a\n    participant b\n${[lines[0], lines[12], ...lines.slice(1, 12)].join("\n")}`,
    );
    await vi.waitFor(
      () => {
        expect(text(view)).toBe(want);
      },
      { timeout: 10_000 },
    );
    await flush();
    await flush();
    expect(text(view)).toBe(want);
    expect(noticeOf(view)).toBe("");
    expect(undoSteps(view)).toBe(steps + 11);
    expect(view.state.field(diagramIntent)?.target).toEqual(
      message("a", "b", "m13"),
    );
  });

  it("eleven presses while each drawing lags behind the text: each press still moves", async () => {
    const lines = Array.from(
      { length: 13 },
      (_, i) => `    a->>b: m${String(i + 1)}`,
    );
    const body = `sequenceDiagram\n    participant a\n    participant b\n${lines.join("\n")}`;
    const view = await drawn(page(body));
    kit.renderMs = 200;
    focusTarget(view, fromOf(view), message("a", "b", "m13"));
    for (let i = 0; i < 11; i++) {
      altKey("ArrowUp");
    }
    const want = page(
      `sequenceDiagram\n    participant a\n    participant b\n${[lines[0], lines[12], ...lines.slice(1, 12)].join("\n")}`,
    );
    await vi.waitFor(
      () => {
        expect(text(view)).toBe(want);
      },
      { timeout: 10_000 },
    );
    expect(noticeOf(view)).toBe("");
  });

  it("presses made before a peer's move was said are all dropped, and a later one moves", async () => {
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    a->>b: m1\n    a->>b: m2\n    a->>b: m3";
    const peers = markPeers(page(body), modeOptions());
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(peers.local.dom.querySelector(".cm-diagram svg")).not.toBeNull();
    });
    await flush();
    const { local, peer, sync } = peers;
    focusTarget(local, fromOf(local), message("a", "b", "m3"));
    await flush();
    const at = text(peer).indexOf("a->>b: m3") + 1;
    peer.dispatch({ changes: { from: at, to: at + 3, insert: "-->>" } });
    sync();
    await vi.waitFor(() => {
      expect(local.state.field(diagramIntent)?.target).toEqual({
        kind: "participant",
        name: "a",
      });
    });
    // The person comes back to the message by its line and presses twice, quickly.
    focusTarget(local, fromOf(local), message("a", "b", "m2"));
    await flush();
    const now = text(local);
    altKey("ArrowUp");
    altKey("ArrowUp");
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(text(local)).toBe(now);
    expect(noticeOf(local)).toBe(NOTICES.stale);
    altKey("ArrowUp");
    await vi.waitFor(() => {
      expect(text(local)).toContain("m2\n    a->>b: m1");
    });
  });

  it("in a room, a peer's rewrite of the moved line between two presses: the next press only reminds", async () => {
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    a->>b: m1\n    a->>b: m2\n    a->>b: m3";
    const peers = markPeers(page(body), modeOptions());
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(peers.local.dom.querySelector(".cm-diagram svg")).not.toBeNull();
    });
    await flush();
    const { local, peer, sync } = peers;
    focusTarget(local, fromOf(local), message("a", "b", "m3"));
    altKey("ArrowUp");
    await vi.waitFor(() => {
      expect(text(local)).toContain("m3\n    a->>b: m2");
    });
    sync();
    await flush();
    // The peer rewrites that line's arrow: the stop cannot tell it is the same.
    const at = text(peer).indexOf("a->>b: m3") + 1;
    peer.dispatch({ changes: { from: at, to: at + 3, insert: "-->>" } });
    sync();
    await vi.waitFor(() => {
      expect(noticeOf(local)).toBe(NOTICES.stale);
    });
    const now = text(local);
    altKey("ArrowUp");
    await flush();
    await flush();
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(text(local)).toBe(now);
    expect(noticeOf(local)).toBe(NOTICES.stale);
  });

  it("a held Alt+ArrowUp moves once, also after the drawing caught up", async () => {
    const view = await drawn(page(TWELVE));
    focusTarget(view, fromOf(view), message("a", "b", "m12"));
    altKey("ArrowUp");
    await vi.waitFor(() => {
      expect(text(view)).toContain("m12\n    a->>b: m11");
    });
    await redrawn(view, "m12");
    altKey("ArrowUp", { repeat: true });
    altKey("ArrowUp", { repeat: true });
    await flush();
    await flush();
    expect(text(view)).toContain("a->>b: m10\n    a->>b: m12\n    a->>b: m11");
  });

  it("past its twin, the stop stays on the moved message, now the second", async () => {
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    a-->>b: hi\n    a->>b: hi";
    const view = await drawn(page(body));
    focusTarget(view, fromOf(view), message("a", "b", "hi", 0));
    altKey("ArrowDown");
    await changed(view, page(body));
    expect(text(view)).toBe(
      page(
        "sequenceDiagram\n    participant a\n    participant b\n    a->>b: hi\n    a-->>b: hi",
      ),
    );
    await vi.waitFor(() => {
      expect(view.state.field(diagramIntent)?.target).toEqual(
        message("a", "b", "hi", 1),
      );
    });
  });

  it("Alt+ArrowUp on a participant is not the diagram's", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), CALLER);
    const event = altKey("ArrowUp");
    expect(event.defaultPrevented).toBe(false);
    await flush();
    expect(text(view)).toBe(page(STARTER));
  });

  it("an activation the move would break refuses with the sentence", async () => {
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    a->>+b: ask\n    b-->>-a: back";
    const view = await drawn(page(body));
    openMenuOn(view, message("b", "a", "back"));
    expect(row("Move up").getAttribute("aria-description")).toBe(
      "That would end an activation before it starts.",
    );
    press("Escape");
    await flush();
    focusTarget(view, fromOf(view), message("b", "a", "back"));
    altKey("ArrowUp");
    await vi.waitFor(() => {
      expect(noticeOf(view)).toBe(NOTICES.breaksActivation);
    });
    expect(text(view)).toBe(page(body));
  });

  it("with a peer typing in another line meanwhile, the right message moves", async () => {
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    a->>b: hi\n    b->>a: x\n    a->>b: hi";
    const peers = markPeers(page(body), modeOptions());
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(peers.local.dom.querySelector(".cm-diagram svg")).not.toBeNull();
    });
    await flush();
    const { local, peer, sync } = peers;
    focusTarget(local, fromOf(local), message("a", "b", "hi", 1));
    altKey("ArrowUp");
    // The press runs once the presses before it are done (none here): a microtask on.
    await Promise.resolve();
    await Promise.resolve();
    // The peer adds a twin at the top while the move is checked.
    const at = text(peer).indexOf("    a->>b: hi");
    peer.dispatch({ changes: { from: at, insert: "    a->>b: hi\n" } });
    sync();
    await vi.waitFor(() => {
      expect(text(local)).toContain("a->>b: hi\n    b->>a: x\n```");
    });
    sync();
    expect(text(local)).toBe(
      page(
        "sequenceDiagram\n    participant a\n    participant b\n    a->>b: hi\n    a->>b: hi\n    a->>b: hi\n    b->>a: x",
      ),
    );
    expect(text(peer)).toBe(text(local));
  });
});
