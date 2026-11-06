/**
 * The intent on the real surface, with the verdict and mermaid's render stubbed:
 * how it maps, and that every way a diagram can go away closes it through one
 * path, once, with the right reason. `intent.marks-hidden.test.ts` and
 * `intent.marks-show.test.ts` run this file again in each marks mode.
 */

import { syntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { cellMarksMode } from "../../test/cellMarksMode";
import type { MarkViewOptions } from "../../test/markKit";
import {
  cleanupMarkViews,
  markPeers,
  markView,
  remoteInsert,
  setCaret,
  setRaw,
  typeText,
} from "../../test/markKit";
import { parsedState, parsedView } from "../../test/parse";
import { documentLanguage } from "../setup";
import { diagramField } from "./field";
import type { DiagramIntent, IntentCloseReason } from "./intent";
import {
  beforeClose,
  closeDiagramIntent,
  diagramIntent,
  intentStale,
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

  it("keeps the intent for a line a peer inserts exactly at the fence's first mark", async () => {
    const view = await drawn();
    open(view);
    const reasons = closes(view);
    view.dispatch({ changes: { from: FROM, insert: "Peer\n\n" } });
    await flush();
    expect(intentOf(view)?.anchor).toBe(FROM + 6);
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

  it("does not follow the intent into another mode on the same target", async () => {
    const view = await drawn();
    open(view, "rename");
    noteIntentDraft(view, "Old");
    open(view, "menu");
    const seen: (string | undefined)[] = [];
    beforeClose(view, (_reason, intent) => {
      seen.push(intent.draft);
    });
    closeDiagramIntent(view, "user");
    expect(seen).toEqual([undefined]);
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

  it("does not take the focus to another fence that mounts", async () => {
    const doc = `${LONG}${fence(OTHER)}\n\nEnd`;
    kit.settled.set(OTHER, OK);
    const view = await drawn(doc, {}, [BODY]);
    const scroll = (pos: number) => {
      view.dispatch({ effects: EditorView.scrollIntoView(pos) });
      measured(view);
    };
    scroll(view.state.doc.length);
    await flush();
    const far = view.state.doc.toString().lastIndexOf("```mermaid");
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-diagram")).not.toBeNull();
      expect(
        view.state.field(diagramField).fences.map((f) => f.from),
      ).toContain(far);
    });
    open(view, "focus", far);
    (document.activeElement as HTMLElement | null)?.blur();
    scroll(0);
    await flush();
    expect(view.dom.querySelector(".cm-diagram")).not.toBeNull();
    expect(document.activeElement).toBe(document.body);
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

describe("leaving a diagram with no line to stand on (R74)", () => {
  const box = (view: EditorView, index = 0) => {
    const found = view.dom.querySelectorAll<HTMLElement>(".cm-diagram")[index];
    if (found === undefined) throw new Error("no box");
    return found;
  };

  /** A key pressed on the box, as a browser sends it. */
  function pressOn(target: HTMLElement, key: string) {
    target.dispatchEvent(
      new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
    );
  }

  it("keeps the focus on the box when the fence is last, and typing leaves its marks alone", async () => {
    const doc = `Top\n\n${fence(BODY)}`;
    const view = await drawn(doc);
    open(view);
    const reasons = closes(view);
    box(view).focus();
    closeDiagramIntent(view, "user");
    expect(reasons).toEqual(["user"]);
    expect(intentOf(view)).toBeNull();
    expect(document.activeElement).toBe(box(view));
    pressOn(box(view), "x");
    expect(view.state.doc.toString()).toBe(doc);
    typeText(view, "x");
    expect(view.state.doc.toString()).toBe(`x${doc}`);
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(1);
  });

  it("takes a caret off the closing mark of a last fence, to the line above", async () => {
    const doc = `Top\n\n${fence(BODY)}`;
    const view = await drawn(doc);
    setCaret(view, doc.length);
    open(view);
    closeDiagramIntent(view, "user");
    expect(view.state.selection.main.head).toBe(4);
    typeText(view, "x");
    expect(view.state.doc.toString()).toBe(`Top\nx\n${fence(BODY)}`);
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(1);
  });

  it("keeps the focus on the box when another fence follows at once, and typing leaves that fence alone", async () => {
    const doc = `Top\n\n${fence(BODY)}\n${fence(OTHER)}\n\nEnd`;
    const view = await drawn(doc, {}, [BODY, OTHER]);
    open(view);
    box(view).focus();
    closeDiagramIntent(view, "user");
    expect(document.activeElement).toBe(box(view));
    expect(view.state.selection.main.head).toBe(0);
    typeText(view, "x");
    expect(view.state.doc.toString()).toBe(`x${doc}`);
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(2);
  });

  it("treats any fence right below the same way, a code fence too", async () => {
    const code = "```ts\nconst a = 1;\n```";
    const doc = `Top\n\n${fence(BODY)}\n${code}\n\nEnd`;
    const view = await drawn(doc);
    open(view);
    closeDiagramIntent(view, "user");
    expect(view.state.selection.main.head).toBe(0);
    expect(document.activeElement).toBe(box(view));
  });

  it("does not stand on a next fence's mark when Raw comes on, focus in the main view", async () => {
    const doc = `Top\n\n${fence(BODY)}\n${fence(OTHER)}\n\nEnd`;
    const view = await drawn(doc, {}, [BODY, OTHER]);
    open(view);
    setRaw(view, true);
    await flush();
    expect(view.state.selection.main.head).toBe(0);
    expect(document.activeElement).toBe(view.contentDOM);
  });

  it("does not move the caret past the end when Raw comes on with the fence last", async () => {
    const doc = `Top\n\n${fence(BODY)}`;
    const view = await drawn(doc);
    open(view);
    setRaw(view, true);
    await flush();
    expect(view.state.selection.main.head).toBe(0);
    expect(document.activeElement).toBe(view.contentDOM);
  });
});

describe("where a close leaves the caret", () => {
  it("leaves it alone on gone, when another fence took the anchor's place", async () => {
    const doc = `Top\n\n${fence(BODY)}\n\n${fence(OTHER)}\n\nEnd`;
    const view = await drawn(doc, {}, [BODY, OTHER]);
    open(view);
    view.dispatch({
      changes: { from: FROM, to: doc.lastIndexOf("```mermaid") },
    });
    await flush();
    expect(view.state.selection.main.head).toBe(0);
  });

  it("leaves it alone on swap, where the anchor names a place in another document", async () => {
    const view = await drawn();
    open(view);
    view.setState(
      parsedState(
        EditorState.create({
          doc: DOC,
          selection: EditorSelection.cursor(1),
          extensions: [documentLanguage, diagramLayer()],
        }),
      ),
    );
    await flush();
    expect(view.state.selection.main.head).toBe(1);
  });
});

describe("a body edit with the parse behind (R75)", () => {
  /** Runs `edit` with a clock that jumps, so the parse budget is gone at once. */
  function withNoBudget(edit: () => void) {
    let now = Date.now();
    const clock = vi.spyOn(Date, "now").mockImplementation(() => {
      now += 1_000;
      return now;
    });
    try {
      edit();
    } finally {
      clock.mockRestore();
    }
  }

  it("holds the intent as stale, keeps the box, and closes nothing until the parse answers", async () => {
    const view = await drawn(DOC, { room: true });
    open(view, "rename");
    const dom = view.dom.querySelector(".cm-diagram");
    const reasons = closes(view);
    expect(intentStale(view.state)).toBe(false);
    withNoBudget(() => {
      remoteInsert(view, view.state.doc.toString().indexOf("Next"), "X");
    });
    expect(syntaxTree(view.state).length).toBeLessThan(view.state.doc.length);
    expect(intentOf(view)?.mode).toBe("rename");
    expect(intentStale(view.state)).toBe(true);
    expect(view.dom.querySelector(".cm-diagram")).toBe(dom);
    await flush();
    expect(reasons).toEqual([]);

    parsedView(view);
    expect(syntaxTree(view.state).length).toBe(view.state.doc.length);
    expect(intentStale(view.state)).toBe(false);
    expect(intentOf(view)?.mode).toBe("rename");
    expect(view.dom.querySelector(".cm-diagram")).toBe(dom);
    await flush();
    expect(reasons).toEqual([]);
  });

  it("lets the verdict decide once the parse has answered", async () => {
    const view = await drawn(DOC, { room: true });
    open(view);
    const reasons = closes(view);
    withNoBudget(() => {
      remoteInsert(view, view.state.doc.toString().indexOf("Next"), "X");
    });
    parsedView(view);
    await flush();
    const body = BODY.replace("Next", "XNext");
    await vi.waitFor(() => {
      expect(kit.waiting.has(body)).toBe(true);
    });
    kit.settled.set(body, NO);
    kit.waiting.get(body)?.(NO);
    await flush();
    expect(intentOf(view)).toBeNull();
    expect(reasons).toEqual(["gone"]);
  });

  it("closes as gone when the edit took the fence's first line", async () => {
    const view = await drawn();
    open(view);
    const reasons = closes(view);
    withNoBudget(() => {
      view.dispatch({ changes: { from: FROM, to: FROM + 3 } });
    });
    await flush();
    expect(intentOf(view)).toBeNull();
    expect(reasons).toEqual(["gone"]);
  });
});

describe("an intent opened on another fence", () => {
  const TWO = `Top\n\n${fence(BODY)}\n\n${fence(OTHER)}\n\nEnd`;

  it("closes the first as gone, with its draft, when its fence went in the same tick", async () => {
    const view = await drawn(TWO, {}, [BODY, OTHER]);
    open(view, "rename");
    noteIntentDraft(view, "New");
    const reasons = closes(view);
    const seen: [IntentCloseReason, string | undefined, string][] = [];
    beforeClose(view, (reason, intent) => {
      seen.push([reason, intent.draft, intent.mode]);
    });
    view.dispatch({
      changes: { from: FROM, to: TWO.lastIndexOf("```mermaid") },
    });
    open(view, "focus", FROM);
    expect(intentOf(view)?.mode).toBe("focus");
    await flush();
    expect(seen).toEqual([["gone", "New", "rename"]]);
    expect(reasons).toEqual(["gone"]);
    // The new one stands, and is not closed by the old one's microtask.
    expect(intentOf(view)?.mode).toBe("focus");
    closeDiagramIntent(view, "user");
    expect(reasons).toEqual(["gone", "user"]);
  });

  it("closes the first as user when its fence is still there", async () => {
    const view = await drawn(TWO, {}, [BODY, OTHER]);
    open(view, "rename");
    noteIntentDraft(view, "New");
    const reasons = closes(view);
    const drafts: (string | undefined)[] = [];
    beforeClose(view, (_reason, intent) => {
      drafts.push(intent.draft);
    });
    open(view, "focus", TWO.lastIndexOf("```mermaid"));
    await flush();
    expect(reasons).toEqual(["user"]);
    expect(drafts).toEqual(["New"]);
    expect(intentOf(view)?.mode).toBe("focus");
  });

  it("does not close when the mode changes on the same fence", async () => {
    const view = await drawn();
    open(view, "rename");
    const reasons = closes(view);
    open(view, "focus");
    await flush();
    expect(reasons).toEqual([]);
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
