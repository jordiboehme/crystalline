/**
 * The drawn-fence field with the verdict module stubbed: a fence is drawn
 * once its verdict settles ok, stays drawn while a new verdict is pending,
 * and flips to text on a refusal. The real verdicts are `field.real.test.ts`.
 */

import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { parsedState } from "../../test/parse";
import { baseExtensions } from "../setup";
import { drawnDiagrams } from "./draws";
import type { Verdict } from "./verdict";

const kit = vi.hoisted(() => {
  const settled = new Map<string, unknown>();
  const waiting = new Map<string, (verdict: unknown) => void>();
  return {
    settled,
    waiting,
    requested: [] as { body: string; dark: boolean }[],
    /** Answer at once with ok, and keep 256 verdicts like the real cache. */
    auto: false,
  };
});

vi.mock("./verdict", () => ({
  verdictFor: (body: string) => kit.settled.get(body),
  requestVerdict: (body: string, dark: boolean) => {
    kit.requested.push({ body, dark });
    if (kit.auto) {
      const verdict = { ok: true, kind: "flowchart", domIds: new Map() };
      kit.settled.delete(body);
      kit.settled.set(body, verdict);
      while (kit.settled.size > 256) {
        kit.settled.delete(kit.settled.keys().next().value as string);
      }
      return Promise.resolve(verdict);
    }
    return new Promise((resolve) => {
      kit.waiting.set(body, resolve);
    });
  },
}));

const closeDiagramIntent = vi.hoisted(() => vi.fn());
vi.mock("./intent", async (original) => ({
  ...(await original<typeof import("./intent")>()),
  closeDiagramIntent,
}));

// Imported after the mocks.
const { diagramExtension } = await import("./index");
const {
  verdictArrived,
  diagramField,
  carryFences,
  drawUnderCaret,
  POINTER_GRACE_MS,
  VERDICT_PACE_MS,
} = await import("./field");

const OK: Verdict = { ok: true, kind: "flowchart", domIds: new Map() };
const NO: Verdict = { ok: false, reason: "no" };

const SKELETON = "flowchart TD\n  A --> B";
const fence = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\``;

let view: EditorView | null = null;

/** The caret sits after the document (R52 holds a fence the caret is in). */
function mount(doc: string, dark = false, caret?: number): EditorView {
  const text = `${doc}\n\nend`;
  view = new EditorView({
    state: parsedState(
      EditorState.create({
        doc: text,
        selection: EditorSelection.cursor(caret ?? text.length),
        extensions: [...baseExtensions(dark), diagramExtension()],
      }),
    ),
    parent: document.body,
  });
  return view;
}

const flush = async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};

/** Past the verdict pace: a body that waited for it has been asked for. */
const paced = async () => {
  await new Promise((resolve) => setTimeout(resolve, VERDICT_PACE_MS + 10));
};

/** Settle `body`: cache it when final, resolve the waiting request. */
async function settle(body: string, verdict: Verdict, final = true) {
  if (final) kit.settled.set(body, verdict);
  kit.waiting.get(body)?.(verdict);
  kit.waiting.delete(body);
  await flush();
}

beforeEach(() => {
  kit.settled.clear();
  kit.waiting.clear();
  kit.requested.length = 0;
  kit.auto = false;
  closeDiagramIntent.mockClear();
});

afterEach(() => {
  view?.destroy();
  view = null;
});

describe("the diagram field", () => {
  it("draws the skeleton once its verdict settles, not before", async () => {
    const v = mount(fence(SKELETON));
    expect(v.dom.querySelector(".cm-diagram")).toBeNull();
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    expect(kit.requested).toEqual([{ body: SKELETON, dark: false }]);
    await settle(SKELETON, OK);
    expect(v.dom.querySelector(".cm-diagram")).not.toBeNull();
    expect(v.state.facet(drawnDiagrams)).toEqual([0]);
    expect(v.contentDOM.textContent).not.toContain("flowchart TD");
    expect(v.state.doc.toString()).toBe(`${fence(SKELETON)}\n\nend`);
  });

  it("asks with the dark the surface was built with, and only once per body", () => {
    const v = mount(`${fence(SKELETON)}\n\n${fence(SKELETON)}`, true);
    expect(kit.requested).toEqual([{ body: SKELETON, dark: true }]);
    v.dispatch({ changes: { from: 0, insert: "x\n\n" } });
    expect(kit.requested).toHaveLength(1);
  });

  it("draws at once when the verdict is already known", () => {
    kit.settled.set(SKELETON, OK);
    const v = mount(fence(SKELETON));
    expect(v.dom.querySelector(".cm-diagram")).not.toBeNull();
    expect(kit.requested).toEqual([]);
  });

  it("keeps a drawn fence drawn while the verdict of its new body is pending", async () => {
    const v = mount(fence(SKELETON));
    await settle(SKELETON, OK);
    const at = v.state.doc.toString().indexOf("B");
    v.dispatch({ changes: { from: at, to: at + 1, insert: "C" } });
    expect(v.state.facet(drawnDiagrams)).toEqual([0]);
    expect(v.dom.querySelector(".cm-diagram")).not.toBeNull();
    expect(kit.requested.at(-1)?.body).toBe("flowchart TD\n  A --> C");
    expect(closeDiagramIntent).not.toHaveBeenCalled();
  });

  it("flips to text when the new verdict refuses, and closes the intent as gone", async () => {
    const v = mount(fence(SKELETON));
    await settle(SKELETON, OK);
    const at = v.state.doc.toString().indexOf("B");
    v.dispatch({ changes: { from: at, to: at + 1, insert: "C" } });
    await settle("flowchart TD\n  A --> C", NO);
    expect(v.dom.querySelector(".cm-diagram")).toBeNull();
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    expect(v.contentDOM.textContent).toContain("flowchart TD");
    expect(closeDiagramIntent).toHaveBeenCalledTimes(1);
    expect(closeDiagramIntent.mock.calls[0]?.[0]).toBe(v);
    expect(closeDiagramIntent.mock.calls[0]?.[1]).toBe("gone");
  });

  it("flips to text when an edit makes the kind one it does not draw", async () => {
    const v = mount(fence(SKELETON));
    await settle(SKELETON, OK);
    v.dispatch({ changes: { from: 11, to: 20, insert: "stateDiagram" } });
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
  });

  it("maps a drawn fence through an insert above it", async () => {
    const v = mount(fence(SKELETON));
    await settle(SKELETON, OK);
    v.dispatch({ changes: { from: 0, insert: "title\n\n" } });
    expect(v.state.facet(drawnDiagrams)).toEqual([7]);
  });

  it("never draws a fence that was not drawn while its verdict is pending", () => {
    const v = mount(`${fence(SKELETON)}\n\ntext`);
    v.dispatch({ changes: { from: v.state.doc.length, insert: " more" } });
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
  });

  it("draws nothing when the check could not run, and asks again on the next edit", async () => {
    const v = mount(fence(SKELETON));
    // mermaid failed to load: an answer, but no verdict, and not cached.
    await settle(SKELETON, NO, false);
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    expect(v.dom.querySelector(".cm-diagram")).toBeNull();
    expect(kit.requested).toHaveLength(1);
    v.dispatch({ changes: { from: v.state.doc.length, insert: "\n" } });
    expect(kit.requested).toHaveLength(2);
    await settle(SKELETON, OK);
    expect(v.state.facet(drawnDiagrams)).toEqual([0]);
  });

  it("draws the fences of a document one by one as their verdicts arrive", async () => {
    const other = "flowchart LR\n  X --> Y";
    const v = mount(`${fence(SKELETON)}\n\n${fence(other)}`);
    await settle(other, OK);
    expect(v.state.facet(drawnDiagrams)).toEqual([fence(SKELETON).length + 2]);
    await settle(SKELETON, OK);
    expect(v.state.facet(drawnDiagrams)).toHaveLength(2);
  });

  it("provides the drawn block as an atomic range", async () => {
    const v = mount(fence(SKELETON));
    await settle(SKELETON, OK);
    const field = v.state.field(diagramField);
    expect(field.fences).toEqual([
      {
        from: 0,
        to: fence(SKELETON).length,
        body: SKELETON,
        kind: "flowchart",
      },
    ]);
    const atomic = v.state.facet(EditorView.atomicRanges).map((f) => f(v));
    expect(atomic.some((set) => set.size > 0)).toBe(true);
  });

  it("ignores a verdictArrived for a body nobody holds", () => {
    const v = mount("plain");
    v.dispatch({ effects: verdictArrived.of("nothing") });
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
  });
});

describe("carryFences", () => {
  const drawn = {
    from: 100,
    to: 140,
    body: "flowchart TD\n A-->B",
    kind: "flowchart",
  };

  it("keeps a drawn fence the tree has not reached, at its mapped place", () => {
    const state = EditorState.create({ doc: "x".repeat(300) });
    const tr = state.update({ changes: { from: 0, insert: "yy" } });
    const carried = carryFences([drawn], tr, 50, []);
    expect(carried.map((f) => [f.from, f.to])).toEqual([[102, 142]]);
  });

  it("drops one the tree reaches, one a change touched and one a candidate already holds", () => {
    const state = EditorState.create({ doc: "x".repeat(300) });
    const plain = state.update({ changes: { from: 0, insert: "yy" } });
    expect(carryFences([drawn], plain, 500, [])).toEqual([]);
    const touched = state.update({ changes: { from: 110, insert: "z" } });
    expect(carryFences([drawn], touched, 50, [])).toEqual([]);
    const held = carryFences([drawn], plain, 50, [
      { ...drawn, from: 102, to: 142 },
    ]);
    expect(held).toEqual([]);
  });
});

describe("carryFences and a touched body (R75)", () => {
  const LINES = "```mermaid\nflowchart TD\n  A --> B\n```\n";
  const body = "flowchart TD\n  A --> B";

  it("keeps a fence whose body was edited, and drops one whose first line was", () => {
    const state = EditorState.create({ doc: `${LINES}${"x".repeat(300)}` });
    const drawn = { from: 0, to: LINES.length - 1, body, kind: "flowchart" };
    const inBody = state.update({ changes: { from: 15, insert: "z" } });
    expect(carryFences([drawn], inBody, 5, []).map((f) => f.to)).toEqual([
      drawn.to + 1,
    ]);
    const first = state.update({ changes: { from: 3, insert: "z" } });
    expect(carryFences([drawn], first, 5, [])).toEqual([]);
  });

  it("keeps a fence when a whole line goes in right before its first mark", () => {
    const state = EditorState.create({ doc: `${LINES}${"x".repeat(300)}` });
    const drawn = { from: 0, to: LINES.length - 1, body, kind: "flowchart" };
    const line = state.update({ changes: { from: 0, insert: "New line\n" } });
    expect(carryFences([drawn], line, 5, []).map((f) => f.from)).toEqual([9]);
  });

  it("drops a fence when text at its first mark leaves that line changed", () => {
    const state = EditorState.create({ doc: `${LINES}${"x".repeat(300)}` });
    const drawn = { from: 0, to: LINES.length - 1, body, kind: "flowchart" };
    const glued = state.update({ changes: { from: 0, insert: "x" } });
    expect(carryFences([drawn], glued, 5, [])).toEqual([]);
  });
});

describe("asking for verdicts ends", () => {
  const many = (prefix: string, count: number) =>
    Array.from({ length: count }, (_, i) =>
      fence(`flowchart TD\n  ${prefix}${String(i)} --> B`),
    ).join("\n\n");

  it("settles for 257 distinct fences: one request each, then quiet", async () => {
    kit.auto = true;
    const v = mount(many("L", 257));
    for (let turn = 0; turn < 5; turn += 1) {
      await flush();
    }
    expect(kit.requested).toHaveLength(257);
    await flush();
    expect(kit.requested).toHaveLength(257);
    // The first fences lost their verdicts to the cache but stay drawn.
    expect(v.state.facet(drawnDiagrams).length).toBeGreaterThan(200);
  });

  it("settles for two views that exceed the cache together", async () => {
    kit.auto = true;
    const one = mount(many("P", 130));
    const first = one;
    view = null;
    const two = mount(many("Q", 130));
    for (let turn = 0; turn < 5; turn += 1) {
      await flush();
    }
    expect(kit.requested).toHaveLength(260);
    await flush();
    expect(kit.requested).toHaveLength(260);
    first.destroy();
    two.destroy();
  });
});

describe("a fence under the caret (R52)", () => {
  it("is not newly drawn while the caret is in it, and is drawn when the caret leaves", async () => {
    const doc = `Top\n\n${fence(SKELETON)}\n\nBelow`;
    const inside = doc.indexOf("B\n```") + 1;
    const v = mount(doc, false, inside);
    await settle(SKELETON, OK);
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    expect(v.contentDOM.textContent).toContain("flowchart TD");
    v.dispatch({ changes: { from: inside, insert: "C" } });
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    v.dispatch({ selection: EditorSelection.cursor(v.state.doc.length) });
    // The edited body has no verdict yet; the old one settled for the old body.
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    await settle("flowchart TD\n  A --> BC", OK);
    expect(v.state.facet(drawnDiagrams)).toEqual([5]);
  });

  it("is never drawn while a line is typed into it key by key", async () => {
    kit.auto = true;
    const v = mount("```mermaid\n\n```", false, 11);
    const typed = "graph LR\n  A --> B\n  B --> C";
    const states: number[] = [];
    for (const char of typed) {
      const at = v.state.selection.main.head;
      v.dispatch({
        changes: { from: at, insert: char },
        selection: EditorSelection.cursor(at + 1),
      });
      await flush();
      states.push(v.state.facet(drawnDiagrams).length);
    }
    expect(states.every((n) => n === 0)).toBe(true);
    expect(v.contentDOM.textContent).toContain("graph LR");
    // The keys came faster than the pace: the last body is asked for at its end.
    await paced();
    // The caret leaves: now it draws.
    v.dispatch({ selection: EditorSelection.cursor(v.state.doc.length) });
    expect(v.state.facet(drawnDiagrams)).toEqual([0]);
  });

  it("is drawn under the caret when the starter hook says so", async () => {
    const v = mount(fence(SKELETON), false, 14);
    v.dispatch({ effects: drawUnderCaret.of(0) });
    await settle(SKELETON, OK);
    expect(v.state.facet(drawnDiagrams)).toEqual([0]);
    // And it follows the fence through an insert above.
    v.dispatch({ changes: { from: 0, insert: "x\n\n" } });
    expect(v.state.facet(drawnDiagrams)).toEqual([3]);
  });
});

describe("a removed fence passes on nothing", () => {
  it("does not draw an unchecked fence that took the place of a drawn one", async () => {
    const other = "flowchart LR\n  X --> Y";
    const v = mount(`${fence(SKELETON)}\n\n${fence(other)}`);
    await settle(SKELETON, OK);
    expect(v.state.facet(drawnDiagrams)).toEqual([0]);
    v.dispatch({ changes: { from: 0, to: fence(SKELETON).length + 2 } });
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    expect(closeDiagramIntent).not.toHaveBeenCalled();
    await flush();
    expect(closeDiagramIntent).toHaveBeenCalledTimes(1);
  });

  it("does not draw an unchecked fence when the whole document is replaced", async () => {
    const v = mount(fence(SKELETON));
    await settle(SKELETON, OK);
    v.dispatch({
      changes: {
        from: 0,
        to: v.state.doc.length,
        insert: fence("graph TD\n  P --> Q"),
      },
    });
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
  });

  it("keeps a fence for an insert at its start and an edit inside its body", async () => {
    const v = mount(fence(SKELETON));
    await settle(SKELETON, OK);
    v.dispatch({ changes: { from: 0, insert: "a\n\n" } });
    expect(v.state.facet(drawnDiagrams)).toEqual([3]);
    v.dispatch({
      changes: { from: v.state.doc.toString().indexOf("B"), insert: "z" },
    });
    expect(v.state.facet(drawnDiagrams)).toEqual([3]);
  });
});

describe("a pointer selection (R58)", () => {
  const press = (v: EditorView) =>
    v.contentDOM.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
  const fire = (type: string, target: EventTarget = document) =>
    target.dispatchEvent(new Event(type, { bubbles: true }));

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  function held() {
    const doc = `Top\n\n${fence(SKELETON)}\n\nBelow`;
    const inside = doc.indexOf("B\n```") + 1;
    const v = mount(doc, false, inside);
    kit.settled.set(SKELETON, OK);
    v.dispatch({ effects: verdictArrived.of(SKELETON) });
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    return v;
  }

  it("keeps a fence held while the pointer selects, and draws a grace after mouse up", () => {
    const v = held();
    press(v);
    v.dispatch({ selection: EditorSelection.cursor(0) });
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    // A hand move during the press selects, and still draws nothing.
    v.dispatch({ selection: EditorSelection.range(0, 2) });
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    fire("mouseup");
    vi.advanceTimersByTime(POINTER_GRACE_MS - 1);
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(v.state.facet(drawnDiagrams)).toEqual([5]);
  });

  it("keeps the hold through a double click: a second press inside the grace", () => {
    const v = held();
    press(v);
    v.dispatch({ selection: EditorSelection.cursor(0) });
    fire("mouseup");
    vi.advanceTimersByTime(300);
    press(v);
    v.dispatch({ selection: EditorSelection.range(0, 3) });
    fire("mouseup");
    vi.advanceTimersByTime(POINTER_GRACE_MS - 1);
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(v.state.facet(drawnDiagrams)).toEqual([5]);
  });

  it("a mouse up outside the editor counts too (the listener is on the document)", () => {
    const v = held();
    press(v);
    v.dispatch({ selection: EditorSelection.cursor(0) });
    document.body.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    vi.advanceTimersByTime(POINTER_GRACE_MS);
    expect(v.state.facet(drawnDiagrams)).toEqual([5]);
  });

  it.each([
    ["pointercancel", document],
    ["dragend", document],
    ["contextmenu", document],
    ["blur", window],
  ])("a %s ends the press at once, so a hold cannot stick", (type, target) => {
    const v = held();
    press(v);
    v.dispatch({ selection: EditorSelection.cursor(0) });
    fire(type, target);
    expect(v.state.facet(drawnDiagrams)).toEqual([5]);
  });

  it("ends the press on a mouse move that shows no button down (a missed mouse up)", () => {
    const v = held();
    press(v);
    v.dispatch({ selection: EditorSelection.cursor(0) });
    document.dispatchEvent(new MouseEvent("mousemove", { buttons: 0 }));
    vi.advanceTimersByTime(POINTER_GRACE_MS);
    expect(v.state.facet(drawnDiagrams)).toEqual([5]);
  });

  it("does not hold a keyboard move", () => {
    const v = held();
    v.dispatch({ selection: EditorSelection.cursor(0) });
    expect(v.state.facet(drawnDiagrams)).toEqual([5]);
  });

  it("does not draw a fence whose verdict arrives during the press until the release", () => {
    const v = mount(`Top\n\n${fence(SKELETON)}\n\nBelow`, false, 0);
    press(v);
    kit.settled.set(SKELETON, OK);
    v.dispatch({ effects: verdictArrived.of(SKELETON) });
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    fire("mouseup");
    vi.advanceTimersByTime(POINTER_GRACE_MS);
    expect(v.state.facet(drawnDiagrams)).toEqual([5]);
  });

  it("stops listening and drops its timer when the view is destroyed", () => {
    const v = held();
    press(v);
    fire("mouseup");
    v.destroy();
    expect(() => {
      fire("mouseup");
      vi.advanceTimersByTime(POINTER_GRACE_MS);
    }).not.toThrow();
  });
});

describe("the verdict pace (F5)", () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  /** Types `text` one key at a time at `at`, `gap` ms apart. */
  function type(v: EditorView, at: number, text: string, gap: number) {
    for (const [i, char] of [...text].entries()) {
      v.dispatch({ changes: { from: at + i, insert: char } });
      vi.advanceTimersByTime(gap);
    }
  }

  const bodies = () => kit.requested.map((r) => r.body);

  /** `settle` with an ok verdict, on the faked clock. */
  async function settleNow(body: string) {
    kit.settled.set(body, OK);
    kit.waiting.get(body)?.(OK);
    kit.waiting.delete(body);
    await vi.advanceTimersByTimeAsync(0);
  }

  it("asks for the first body of a burst at once and for its last at the end, none between", () => {
    const v = mount(fence(SKELETON));
    const at = v.state.doc.toString().indexOf("B") + 1;
    type(v, at, "CDEF", 50);
    expect(bodies()).toEqual([SKELETON, "flowchart TD\n  A --> BC"]);
    vi.advanceTimersByTime(VERDICT_PACE_MS);
    expect(bodies()).toEqual([
      SKELETON,
      "flowchart TD\n  A --> BC",
      "flowchart TD\n  A --> BCDEF",
    ]);
    // Nothing more is owed.
    vi.advanceTimersByTime(VERDICT_PACE_MS * 5);
    expect(kit.requested).toHaveLength(3);
  });

  it("asks at once again after a quiet spell", () => {
    const v = mount(fence(SKELETON));
    const at = v.state.doc.toString().indexOf("B") + 1;
    v.dispatch({ changes: { from: at, insert: "C" } });
    vi.advanceTimersByTime(VERDICT_PACE_MS);
    v.dispatch({ changes: { from: at + 1, insert: "D" } });
    expect(bodies().at(-1)).toBe("flowchart TD\n  A --> BCD");
  });

  it("never lets a late verdict of an older body draw over the newer one", async () => {
    const v = mount(fence(SKELETON));
    await settleNow(SKELETON);
    const at = v.state.doc.toString().indexOf("B") + 1;
    vi.advanceTimersByTime(VERDICT_PACE_MS);
    type(v, at, "CD", 50);
    vi.advanceTimersByTime(VERDICT_PACE_MS);
    const older = "flowchart TD\n  A --> BC";
    const newer = "flowchart TD\n  A --> BCD";
    expect(bodies().slice(-2)).toEqual([older, newer]);
    await settleNow(newer);
    expect(v.state.field(diagramField).fences.map((f) => f.body)).toEqual([
      newer,
    ]);
    await settleNow(older);
    expect(v.state.field(diagramField).fences.map((f) => f.body)).toEqual([
      newer,
    ]);
  });

  it("asks nothing once the view is gone", () => {
    const v = mount(fence(SKELETON));
    const at = v.state.doc.toString().indexOf("B") + 1;
    type(v, at, "CD", 50);
    const asked = kit.requested.length;
    v.destroy();
    view = null;
    vi.advanceTimersByTime(VERDICT_PACE_MS * 2);
    expect(kit.requested).toHaveLength(asked);
  });
});

describe("a verdict the cache evicted", () => {
  it("is asked for again when an edit brings its body back", async () => {
    const v = mount(fence(SKELETON));
    await settle(SKELETON, OK);
    expect(v.state.facet(drawnDiagrams)).toEqual([0]);
    const at = v.state.doc.toString().indexOf("B");
    v.dispatch({ changes: { from: at, to: at + 1, insert: "C" } });
    await settle("flowchart TD\n  A --> C", NO);
    // 300 other bodies push the old verdicts out.
    for (let i = 0; i < 300; i += 1) {
      kit.settled.set(`other ${String(i)}`, OK);
      kit.settled.delete(`other ${String(i - 256)}`);
    }
    kit.settled.delete(SKELETON);
    kit.settled.delete("flowchart TD\n  A --> C");
    const asks = () => kit.requested.filter((r) => r.body === SKELETON).length;
    expect(asks()).toBe(1);
    // Undo to the old body, past the pace of the edit before it.
    await paced();
    v.dispatch({ changes: { from: at, to: at + 1, insert: "B" } });
    expect(asks()).toBe(2);
  });
});

describe("a sequence diagram", () => {
  const STARTER =
    "sequenceDiagram\n    participant caller as Caller\n    participant service as Service\n    caller->>service: Request\n    service-->>caller: Reply";

  it("draws the starter once its verdict settles, not before", async () => {
    const v = mount(fence(STARTER));
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    expect(kit.requested).toEqual([{ body: STARTER, dark: false }]);
    await settle(STARTER, { ok: true, kind: "sequence", domIds: new Map() });
    expect(v.state.facet(drawnDiagrams)).toEqual([0]);
    expect(v.contentDOM.textContent).not.toContain("sequenceDiagram");
    expect(v.state.field(diagramField).fences[0]?.kind).toBe("sequenceDiagram");
  });

  it("flips to text when its verdict refuses", async () => {
    const v = mount(fence(STARTER));
    await settle(STARTER, NO);
    expect(v.state.facet(drawnDiagrams)).toEqual([]);
    expect(v.contentDOM.textContent).toContain("sequenceDiagram");
  });
});
