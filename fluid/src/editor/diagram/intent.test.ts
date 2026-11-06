/**
 * The intent on the real surface, with the verdict and mermaid's render stubbed:
 * how it maps, and that every way a diagram can go away closes it through one
 * path, once, with the right reason. `intent.marks-hidden.test.ts` and
 * `intent.marks-show.test.ts` run this file again in each marks mode.
 */

import { EditorState, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { cellMarksMode } from "../../test/cellMarksMode";
import type { MarkViewOptions } from "../../test/markKit";
import {
  cleanupMarkViews,
  markPeers,
  markView,
  remoteInsert,
  setRaw,
} from "../../test/markKit";
import { documentLanguage } from "../setup";
import { diagramField } from "./field";
import type { DiagramIntent, IntentCloseReason } from "./intent";
import {
  beforeClose,
  closeDiagramIntent,
  diagramIntent,
  noteIntentDraft,
  onIntentClosed,
  registerIntentReopen,
  setDiagramIntent,
} from "./intent";
import { diagramLayer, prefetchDiagrams } from "./loader";
import type { Verdict } from "./verdict";
import { forgetDrawings } from "./widget";

const kit = vi.hoisted(() => ({
  settled: new Map<string, unknown>(),
  waiting: new Map<string, (verdict: unknown) => void>(),
}));

vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: (id: string) => Promise.resolve({ svg: `<svg id="${id}"></svg>` }),
  },
}));

vi.mock("./verdict", () => ({
  verdictFor: (body: string) => kit.settled.get(body),
  requestVerdict: (body: string) =>
    new Promise((resolve) => {
      kit.waiting.set(body, resolve);
    }),
}));

const OK: Verdict = { ok: true, kind: "flowchart", domIds: new Map() };
const NO: Verdict = { ok: false, reason: "no" };
const BODY = "flowchart TD\n    A[First step] --> B[Next step]";
const OTHER = "flowchart TD\n    C --> D";
const fence = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\``;
const DOC = `Top\n\n${fence(BODY)}\n\nEnd\n`;
const FROM = DOC.indexOf("```");
const CANVAS = { kind: "canvas" } as const;

/** Runs CodeMirror's pending measure now, as a frame would. */
function measured(view: EditorView): void {
  (view as unknown as { measure(): void }).measure();
}

const flush = async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};

function modeOptions(): MarkViewOptions {
  const mode = cellMarksMode();
  return mode === "off" ? {} : { mode };
}

async function drawn(
  doc = DOC,
  options: MarkViewOptions = {},
  bodies = [BODY],
) {
  for (const body of bodies) {
    kit.settled.set(body, OK);
  }
  const view = markView(doc, { ...modeOptions(), ...options });
  await prefetchDiagrams();
  await vi.waitFor(() => {
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(
      bodies.length,
    );
  });
  await flush();
  return view;
}

function open(
  view: EditorView,
  mode: DiagramIntent["mode"] = "menu",
  anchor = view.state.doc.toString().indexOf("```"),
) {
  view.dispatch({
    effects: setDiagramIntent.of({ anchor, target: CANVAS, mode }),
  });
}

/** Every close of `view`, in order. */
function closes(view: EditorView): IntentCloseReason[] {
  const reasons: IntentCloseReason[] = [];
  onIntentClosed(view, (reason) => {
    reasons.push(reason);
  });
  return reasons;
}

const intentOf = (view: EditorView) => view.state.field(diagramIntent);

beforeEach(() => {
  kit.settled.clear();
  kit.waiting.clear();
  forgetDrawings();
});

afterEach(() => {
  cleanupMarkViews();
});

describe("the field", () => {
  it("is null at first, and takes an intent only where a fence is drawn", async () => {
    const view = await drawn();
    expect(intentOf(view)).toBeNull();
    open(view, "menu", 0);
    expect(intentOf(view)).toBeNull();
    open(view, "menu", FROM + 12);
    expect(intentOf(view)).toEqual({
      anchor: FROM,
      target: CANVAS,
      mode: "menu",
    });
  });

  it("maps the anchor through an insert above the fence", async () => {
    const view = await drawn();
    open(view);
    view.dispatch({ changes: { from: 0, insert: "More text\n\n" } });
    expect(intentOf(view)?.anchor).toBe(FROM + 11);
  });

  it("maps the anchor through a peer's insert, and keeps it through an edit of the body", async () => {
    const view = await drawn(DOC, { room: true });
    open(view);
    remoteInsert(view, 0, "ab");
    expect(intentOf(view)?.anchor).toBe(FROM + 2);
    const reasons = closes(view);
    remoteInsert(view, view.state.doc.toString().indexOf("Next"), "X");
    await flush();
    expect(intentOf(view)?.anchor).toBe(FROM + 2);
    expect(reasons).toEqual([]);
  });

  it("goes null when the fence is deleted, and tells the listeners gone, once", async () => {
    const view = await drawn();
    open(view);
    const reasons = closes(view);
    view.dispatch({
      changes: { from: FROM, to: DOC.indexOf("End") },
    });
    expect(intentOf(view)).toBeNull();
    expect(reasons).toEqual([]);
    await flush();
    expect(reasons).toEqual(["gone"]);
  });

  it("does not land on the next fence when its own is deleted", async () => {
    const doc = `Top\n\n${fence(BODY)}\n\n${fence(OTHER)}\n\nEnd`;
    const view = await drawn(doc, {}, [BODY, OTHER]);
    open(view);
    const reasons = closes(view);
    view.dispatch({
      changes: { from: FROM, to: doc.lastIndexOf("```mermaid") },
    });
    expect(view.state.field(diagramField).fences).toHaveLength(1);
    expect(intentOf(view)).toBeNull();
    await flush();
    expect(reasons).toEqual(["gone"]);
  });

  it("stays open when another fence is lost", async () => {
    const doc = `Top\n\n${fence(BODY)}\n\n${fence(OTHER)}\n\nEnd`;
    const view = await drawn(doc, {}, [BODY, OTHER]);
    const second = doc.lastIndexOf("```mermaid");
    open(view, "menu", second);
    const reasons = closes(view);
    view.dispatch({ changes: { from: FROM, to: second } });
    await flush();
    expect(reasons).toEqual([]);
    expect(intentOf(view)?.anchor).toBe(FROM);
  });

  it("closes gone when the fence's new body is refused", async () => {
    const view = await drawn();
    open(view);
    const reasons = closes(view);
    const at = DOC.indexOf("Next");
    view.dispatch({ changes: { from: at, insert: "X" } });
    await flush();
    expect(intentOf(view)).not.toBeNull();
    const body = BODY.replace("Next", "XNext");
    kit.settled.set(body, NO);
    kit.waiting.get(body)?.(NO);
    await flush();
    expect(intentOf(view)).toBeNull();
    expect(reasons).toEqual(["gone"]);
  });

  it("does not take the focus from another field when a peer removes the fence", async () => {
    const view = await drawn(DOC, { room: true });
    const outside = document.createElement("input");
    document.body.append(outside);
    outside.focus();
    open(view);
    view.dispatch({ changes: { from: FROM, to: DOC.indexOf("End") } });
    await flush();
    expect(document.activeElement).toBe(outside);
    outside.remove();
  });

  it("gives the focus back to the main view when the focused box goes", async () => {
    const view = await drawn();
    view.dom.querySelector<HTMLElement>(".cm-diagram")?.focus();
    open(view);
    view.dispatch({ changes: { from: FROM, to: DOC.indexOf("End") } });
    await flush();
    expect(document.activeElement).toBe(view.contentDOM);
  });
});

describe("closing by hand", () => {
  it("empties the field, tells once, and leaves the caret on the line after the fence", async () => {
    const view = await drawn();
    open(view);
    const reasons = closes(view);
    closeDiagramIntent(view, "user");
    expect(intentOf(view)).toBeNull();
    expect(reasons).toEqual(["user"]);
    expect(view.state.selection.main.head).toBe(DOC.lastIndexOf("```") + 4);
    expect(document.activeElement).toBe(view.contentDOM);
    await flush();
    expect(reasons).toEqual(["user"]);
  });

  it("moves neither the caret nor the focus with focusTo none", async () => {
    const view = await drawn();
    open(view);
    const head = view.state.selection.main.head;
    const outside = document.createElement("input");
    document.body.append(outside);
    outside.focus();
    closeDiagramIntent(view, "user", "none");
    expect(view.state.selection.main.head).toBe(head);
    expect(document.activeElement).toBe(outside);
    outside.remove();
  });

  it("does nothing when nothing is open", async () => {
    const view = await drawn();
    const reasons = closes(view);
    const hook = vi.fn();
    beforeClose(view, hook);
    closeDiagramIntent(view, "user");
    closeDiagramIntent(view, "gone");
    expect(reasons).toEqual([]);
    expect(hook).not.toHaveBeenCalled();
  });

  it("stops telling a listener that left", async () => {
    const view = await drawn();
    const listener = vi.fn();
    const leave = onIntentClosed(view, listener);
    leave();
    open(view);
    closeDiagramIntent(view, "user");
    expect(listener).not.toHaveBeenCalled();
  });

  it("may be called from a listener's own close without closing twice", async () => {
    const view = await drawn();
    open(view);
    const reasons: IntentCloseReason[] = [];
    onIntentClosed(view, (reason) => {
      reasons.push(reason);
      closeDiagramIntent(view, "user");
    });
    closeDiagramIntent(view, "user");
    expect(reasons).toEqual(["user"]);
  });
});

describe("Raw", () => {
  it("closes an open menu with raw, once, focus in the main view", async () => {
    const view = await drawn();
    open(view);
    const reasons = closes(view);
    view.dom.querySelector<HTMLElement>(".cm-diagram")?.focus();
    setRaw(view, true);
    expect(view.state.field(diagramIntent, false)).toBeUndefined();
    await flush();
    expect(reasons).toEqual(["raw"]);
    expect(document.activeElement).toBe(view.contentDOM);
    expect(view.state.selection.main.head).toBe(DOC.lastIndexOf("```") + 4);
  });

  it("hands beforeClose the last draft, read from beside the field", async () => {
    const view = await drawn();
    open(view, "rename");
    noteIntentDraft(view, "New");
    const seen: [IntentCloseReason, DiagramIntent][] = [];
    beforeClose(view, (reason, intent) => {
      // The apply layer writes from here: the update has ended, and the document
      // and `diagramAnchors` are all it needs.
      view.dispatch({ changes: { from: 0, insert: "x" } });
      seen.push([reason, intent]);
    });
    setRaw(view, true);
    await flush();
    expect(seen).toEqual([
      ["raw", { anchor: FROM, target: CANVAS, mode: "rename", draft: "New" }],
    ]);
    expect(view.state.doc.toString().startsWith("xTop")).toBe(true);
  });

  it("does nothing with no intent open, and Raw off opens none", async () => {
    const view = await drawn();
    const reasons = closes(view);
    const outside = document.createElement("input");
    document.body.append(outside);
    outside.focus();
    setRaw(view, true);
    await flush();
    setRaw(view, false);
    await flush();
    expect(reasons).toEqual([]);
    expect(document.activeElement).toBe(outside);
    expect(intentOf(view)).toBeNull();
    outside.remove();
  });
});

describe("a draft", () => {
  it("is the field's when the field has one", async () => {
    const view = await drawn();
    view.dispatch({
      effects: setDiagramIntent.of({
        anchor: FROM,
        target: CANVAS,
        mode: "rename",
        draft: "Abc",
      }),
    });
    const seen: (string | undefined)[] = [];
    beforeClose(view, (_reason, intent) => {
      seen.push(intent.draft);
    });
    closeDiagramIntent(view, "user");
    expect(seen).toEqual(["Abc"]);
  });

  it("does not follow the intent to another target", async () => {
    const view = await drawn();
    open(view, "rename");
    noteIntentDraft(view, "Old");
    view.dispatch({
      effects: setDiagramIntent.of({
        anchor: FROM,
        target: { kind: "node", id: "B" },
        mode: "rename",
      }),
    });
    const seen: (string | undefined)[] = [];
    beforeClose(view, (_reason, intent) => {
      seen.push(intent.draft);
    });
    closeDiagramIntent(view, "user");
    expect(seen).toEqual([undefined]);
  });
});

describe("setState", () => {
  it("closes with swap, once, focus in the main view", async () => {
    const view = await drawn();
    open(view);
    const reasons = closes(view);
    view.dom.querySelector<HTMLElement>(".cm-diagram")?.focus();
    view.setState(EditorState.create({ doc: "Other text" }));
    await flush();
    expect(reasons).toEqual(["swap"]);
    expect(document.activeElement).toBe(view.contentDOM);
    expect(view.state.doc.toString()).toBe("Other text");
  });

  it("closes with swap when the new state has the layer too, and opens nothing there", async () => {
    const view = await drawn();
    open(view);
    const reasons = closes(view);
    const { state } = view;
    view.setState(
      EditorState.create({
        doc: state.doc,
        selection: { anchor: state.doc.length },
        extensions: [documentLanguage, diagramLayer()],
      }),
    );
    await flush();
    expect(reasons).toEqual(["swap"]);
    expect(view.state.field(diagramIntent)).toBeNull();
    expect(view.dom.querySelector(".cm-diagram")).not.toBeNull();
    closeDiagramIntent(view, "user");
    expect(reasons).toEqual(["swap"]);
  });
});

describe("the view going away", () => {
  it("closes with destroy, once, and takes no focus", async () => {
    const view = await drawn();
    open(view);
    const reasons = closes(view);
    const hook = vi.fn();
    beforeClose(view, hook);
    const outside = document.createElement("input");
    document.body.append(outside);
    outside.focus();
    view.destroy();
    await flush();
    expect(reasons).toEqual(["destroy"]);
    expect(hook).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(outside);
    outside.remove();
  });
});

describe("a fence coming back", () => {
  const LONG = `${DOC}${"Line of text\n\n".repeat(8000)}`;

  /** Scrolls far enough that CodeMirror drops the box, as a person would. */
  function away(view: EditorView) {
    const end = view.state.doc.length;
    view.dispatch({ effects: EditorView.scrollIntoView(end) });
    measured(view);
    expect(view.dom.querySelector(".cm-diagram")).toBeNull();
  }

  function back(view: EditorView) {
    view.dispatch({ effects: EditorView.scrollIntoView(0) });
    measured(view);
    const box = view.dom.querySelector<HTMLElement>(".cm-diagram");
    if (box === null) throw new Error("no box");
    return box;
  }

  it("keeps the intent while the box is gone, and tells no listener", async () => {
    const view = await drawn(LONG);
    open(view);
    const reasons = closes(view);
    away(view);
    await flush();
    expect(intentOf(view)?.mode).toBe("menu");
    back(view);
    await flush();
    expect(intentOf(view)?.mode).toBe("menu");
    expect(reasons).toEqual([]);
  });

  it("hands the mode to its reopener with the new box's host", async () => {
    const view = await drawn(LONG);
    open(view, "connect");
    const reopen = vi.fn();
    registerIntentReopen("connect", reopen);
    away(view);
    await flush();
    expect(reopen).not.toHaveBeenCalled();
    const again = back(view);
    await flush();
    expect(reopen).toHaveBeenCalledTimes(1);
    const [main, intent, host] = reopen.mock.calls[0] as [
      EditorView,
      DiagramIntent,
      { container: HTMLElement },
    ];
    expect(main).toBe(view);
    expect(intent.mode).toBe("connect");
    expect(host.container).toBe(again);
  });

  it("focuses the canvas when no later task claimed the mode and the focus went with the box", async () => {
    const view = await drawn(LONG);
    open(view, "focus");
    view.dom.querySelector<HTMLElement>(".cm-diagram")?.focus();
    away(view);
    await flush();
    const again = back(view);
    await flush();
    expect(document.activeElement).toBe(again);
  });

  it("leaves the focus alone when it found another place", async () => {
    const view = await drawn(LONG);
    open(view, "focus");
    view.dom.querySelector<HTMLElement>(".cm-diagram")?.focus();
    away(view);
    const outside = document.createElement("input");
    document.body.append(outside);
    outside.focus();
    back(view);
    await flush();
    expect(document.activeElement).toBe(outside);
    outside.remove();
  });

  it("leaves a box with no intent alone", async () => {
    const view = await drawn(LONG);
    view.contentDOM.focus();
    away(view);
    view.contentDOM.blur();
    back(view);
    await flush();
    expect(document.activeElement).toBe(document.body);
  });
});

describe("a page that takes no writes", () => {
  it("has the focus mode only: a menu, a rename or a connect falls back", async () => {
    const view = await drawn();
    view.dispatch({
      effects: setDiagramIntent.of({
        anchor: FROM,
        target: CANVAS,
        mode: "rename",
        draft: "x",
      }),
    });
    expect(intentOf(view)?.mode).toBe("rename");
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    expect(intentOf(view)).toEqual({
      anchor: FROM,
      target: CANVAS,
      mode: "focus",
    });
    open(view, "menu");
    expect(intentOf(view)?.mode).toBe("focus");
  });
});

describe("two views", () => {
  it("keeps each view's intent and listeners to itself", async () => {
    kit.settled.set(BODY, OK);
    const { local, peer, sync } = markPeers(DOC, modeOptions());
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(local.dom.querySelector(".cm-diagram")).not.toBeNull();
      expect(peer.dom.querySelector(".cm-diagram")).not.toBeNull();
    });
    open(local);
    const mine = closes(local);
    const theirs = closes(peer);
    expect(intentOf(peer)).toBeNull();
    // The peer removes the fence: my intent goes, once, through the room.
    peer.dispatch({ changes: { from: FROM, to: DOC.indexOf("End") } });
    sync();
    await flush();
    expect(intentOf(local)).toBeNull();
    expect(mine).toEqual(["gone"]);
    expect(theirs).toEqual([]);
  });
});
