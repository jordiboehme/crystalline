/**
 * The diagram layer inside surface-built views (`markKit.markView`, the way
 * both screens build theirs), with the verdict module stubbed: the hand-over
 * from the read-only preview, a remote edit in a room, and the field's carry
 * past the parse reach (D58).
 */

import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import type { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { cleanupMarkViews, markView, remoteInsert } from "../../test/markKit";
import { drawnDiagrams } from "../diagram/draws";
import { verdictArrived } from "./field";
import { prefetchDiagrams } from "./loader";
import type { Verdict } from "./verdict";

vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn(() => Promise.resolve({ svg: "<svg></svg>" })),
  },
}));

const kit = vi.hoisted(() => ({
  settled: new Map<string, unknown>(),
  waiting: new Map<string, (verdict: unknown) => void>(),
  requested: [] as string[],
}));

vi.mock("./verdict", () => ({
  verdictFor: (body: string) => kit.settled.get(body),
  requestVerdict: (body: string) => {
    kit.requested.push(body);
    return new Promise((resolve) => {
      kit.waiting.set(body, resolve);
    });
  },
}));

const OK: Verdict = { ok: true, kind: "flowchart", domIds: new Map() };
const NO: Verdict = { ok: false, reason: "no" };
const BODY = "flowchart TD\n  A --> B";
const fence = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\``;

const flush = async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};

async function settle(body: string, verdict: Verdict) {
  kit.settled.set(body, verdict);
  kit.waiting.get(body)?.(verdict);
  kit.waiting.delete(body);
  await flush();
}

/** A view with the chunk in (the loader sees the fence and imports it). */
async function ready(view: EditorView): Promise<void> {
  await prefetchDiagrams();
  await vi.waitFor(() => {
    expect(view.state.facet(drawnDiagrams)).toBeDefined();
    expect(view.dom.querySelector(".cm-mermaid-preview")).not.toBeNull();
  });
  await vi.waitFor(() => {
    expect(kit.requested.length).toBeGreaterThan(0);
  });
}

beforeEach(() => {
  kit.settled.clear();
  kit.waiting.clear();
  kit.requested.length = 0;
});

afterEach(() => {
  cleanupMarkViews();
});

describe("the preview hands over to the drawn diagram", () => {
  it("shows the preview until the verdict is ok, then the diagram box, then the text again on a refusal", async () => {
    const view = markView(`Top\n\n${fence(BODY)}\n\nEnd`);
    await ready(view);
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(0);
    expect(view.dom.querySelectorAll(".cm-mermaid-preview")).toHaveLength(1);
    await settle(BODY, OK);
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(1);
    expect(view.dom.querySelectorAll(".cm-mermaid-preview")).toHaveLength(0);
    const at = view.state.doc.toString().indexOf("B\n```");
    view.dispatch({ changes: { from: at, to: at + 1, insert: "C" } });
    const changed = "flowchart TD\n  A --> C";
    // Pending: still the box, no preview.
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(1);
    expect(view.dom.querySelectorAll(".cm-mermaid-preview")).toHaveLength(0);
    await settle(changed, NO);
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(0);
    expect(view.dom.querySelectorAll(".cm-mermaid-preview")).toHaveLength(1);
  });
});

describe("in a room", () => {
  it("keeps a drawn fence drawn when a peer edits its body, and asks for the new body", async () => {
    const view = markView(`Top\n\n${fence(BODY)}\n\nEnd`, { room: true });
    await ready(view);
    await settle(BODY, OK);
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(1);
    const at = view.state.doc.toString().indexOf("B\n```");
    remoteInsert(view, at, "X");
    expect(view.state.doc.toString()).toContain("A --> XB");
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(1);
    expect(kit.requested).toContain("flowchart TD\n  A --> XB");
    await settle("flowchart TD\n  A --> XB", OK);
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(1);
  });
});

describe("past the parse reach (D58)", () => {
  // The document before the change is tiny. The change inserts about 1.5 MB of
  // prose above the fence, which no machine parses inside the 20 ms slice the
  // new state's tree gets, so the tree stops short of the fence whatever the
  // load: a slower machine only stops shorter.
  it(
    "keeps a drawn fence drawn at its mapped place while the new tree stops short of it",
    { timeout: 120_000 },
    async () => {
      const text = `Top\n\n${fence(BODY)}\n\nEnd`;
      const view = markView(text);
      await prefetchDiagrams();
      await vi.waitFor(() => {
        expect(kit.requested.length).toBeGreaterThan(0);
      });
      await settle(BODY, OK);
      expect(view.state.facet(drawnDiagrams)).toEqual([5]);
      const other = "Other words for another paragraph.\n\n".repeat(43000);
      view.dispatch({ changes: { from: 3, insert: other } });
      const moved = 5 + other.length;
      expect(syntaxTree(view.state).length).toBeLessThan(moved);
      expect(view.state.facet(drawnDiagrams)).toEqual([moved]);
      // The parse catches up: the tree decides again and agrees.
      ensureSyntaxTree(view.state, view.state.doc.length, 100_000);
      view.dispatch({ effects: verdictArrived.of(BODY) });
      expect(syntaxTree(view.state).length).toBe(view.state.doc.length);
      expect(view.state.facet(drawnDiagrams)).toEqual([moved]);
    },
  );
});
