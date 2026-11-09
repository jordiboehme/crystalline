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
import { applyEdits, NOTICES } from "../edits";
import { focusTarget } from "../focus";
import type { TargetKey } from "../intent";
import { diagramIntent } from "../intent";
import { prefetchDiagrams } from "../loader";
import { forgetDrawings } from "../widget";
import type { SeqTarget } from "./model";
import type { SeqOp } from "./ops";
import { compileSeqOp } from "./ops";
import { parseSequence } from "./parse";

vi.mock("mermaid", async (original) => {
  const real = (await original<{ default: object }>()).default;
  return {
    default: {
      ...real,
      render: (id: string, body: string) =>
        Promise.resolve({ svg: sequenceDrawing(id, body) }),
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
      ["Edit text", "Line", "Arrowhead", "Delete", "Remove block"],
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
      NOTICES.emptiesBlock,
    );
    expect(row("Remove block").getAttribute("aria-disabled")).toBeNull();
  });

  it("a message whose deactivation would be left open is offered disabled, with the sentence", async () => {
    const body =
      "sequenceDiagram\n    participant a\n    participant b\n    a->>+b: call\n    b-->>-a: done";
    const view = await drawn(page(body));
    openMenuOn(view, message("b", "a", "done"));
    expect(row("Delete").getAttribute("aria-disabled")).toBe("true");
    expect(row("Delete").getAttribute("aria-description")).toBe(
      NOTICES.leavesActive,
    );
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
