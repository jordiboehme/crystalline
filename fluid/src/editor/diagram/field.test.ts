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
vi.mock("./intent", () => ({ closeDiagramIntent }));

// Imported after the mocks.
const { diagramExtension } = await import("./index");
const { verdictArrived, diagramField, carryFences, drawUnderCaret } =
  await import("./field");

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
