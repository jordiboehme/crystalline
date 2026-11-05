/**
 * The drawn-fence field with the verdict module stubbed: a fence is drawn
 * once its verdict settles ok, stays drawn while a new verdict is pending,
 * and flips to text on a refusal. The real verdicts are `field.real.test.ts`.
 */

import { EditorState } from "@codemirror/state";
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
  };
});

vi.mock("./verdict", () => ({
  verdictFor: (body: string) => kit.settled.get(body),
  requestVerdict: (body: string, dark: boolean) => {
    kit.requested.push({ body, dark });
    return new Promise((resolve) => {
      kit.waiting.set(body, resolve);
    });
  },
}));

const closeDiagramIntent = vi.hoisted(() => vi.fn());
vi.mock("./intent", () => ({ closeDiagramIntent }));

// Imported after the mocks.
const { diagramExtension } = await import("./index");
const { verdictArrived, diagramField, carryFences } = await import("./field");

const OK: Verdict = { ok: true, kind: "flowchart", domIds: new Map() };
const NO: Verdict = { ok: false, reason: "no" };

const SKELETON = "flowchart TD\n  A --> B";
const fence = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\``;

let view: EditorView | null = null;

function mount(doc: string, dark = false): EditorView {
  view = new EditorView({
    state: parsedState(
      EditorState.create({
        doc,
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
    expect(v.state.doc.toString()).toBe(fence(SKELETON));
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
