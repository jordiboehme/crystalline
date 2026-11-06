/**
 * Mermaid failing to load is no verdict about a fence: what is on screen stays,
 * and a later render tries again. Forced with a mock that fails until told not to.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

import { cleanupMarkViews, markView } from "../../test/markKit";
import { prefetchDiagrams } from "./loader";
import { hostOf } from "./widget";

const load = vi.hoisted(() => ({ fail: true }));

vi.mock("mermaid", () => {
  if (load.fail) {
    throw new Error("Failed to fetch dynamically imported module");
  }
  return {
    default: {
      initialize: () => undefined,
      render: (id: string) =>
        Promise.resolve({ svg: `<svg id="${id}"></svg>` }),
    },
  };
});

vi.mock("./verdict", () => ({
  verdictFor: () => ({ ok: true, kind: "flowchart", domIds: new Map() }),
  requestVerdict: () => new Promise(() => {}),
}));

afterEach(() => {
  cleanupMarkViews();
  vi.useRealTimers();
});

describe("a failed import", () => {
  it("holds the box's height, and an edit after the failure draws", async () => {
    const doc = "Top\n\n```mermaid\nflowchart TD\n    A --> B\n```\n\nEnd";
    const view = markView(doc);
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-diagram")).not.toBeNull();
    });
    await new Promise((resolve) => setTimeout(resolve, 50));
    const box = view.dom.querySelector<HTMLElement>(".cm-diagram");
    expect(box?.querySelector("svg")).toBeNull();
    expect(Number.parseFloat(box?.style.minHeight ?? "")).toBeGreaterThan(48);
    expect(hostOf(view, 5)?.renderedBody()).toBe("");
    load.fail = false;
    view.dispatch({
      changes: { from: doc.indexOf("B\n"), insert: "X" },
    });
    await vi.waitFor(() => {
      expect(box?.querySelector("svg")).not.toBeNull();
    });
    expect(hostOf(view, 5)?.renderedBody()).toContain("XB");
    expect(view.dom.querySelector(".cm-diagram")).toBe(box);
  });
});
