/**
 * The lazy hand-over: the chunk is asked for only when a view holds a
 * top-level, closed mermaid fence of an editable kind, once for every view,
 * and what it brings survives the Raw flip.
 */

import { Compartment, EditorState } from "@codemirror/state";
import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it, vi } from "vitest";

import { parsedState } from "../../test/parse";
import { baseExtensions, RAW_MONO } from "../setup";
import { diagramLoaded } from "./draws";
import { diagramLayerWith } from "./loader";

const FLOW = "```mermaid\nflowchart TD\n  A --> B\n```";

/** A stand-in for the lazy chunk whose extension leaves a visible mark. */
function chunk() {
  const marker: Extension = EditorView.contentAttributes.of({
    "data-diagram": "on",
  });
  return { diagramExtension: () => [marker, diagramLoaded.of(true)] };
}

function mount(doc: string, extension: Extension): EditorView {
  return new EditorView({
    state: parsedState(
      EditorState.create({
        doc,
        extensions: [...baseExtensions(false), extension],
      }),
    ),
    parent: document.body,
  });
}

const flush = async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};

describe("diagramLayerWith", () => {
  it.each([
    ["prose", "just prose\n\nand more"],
    ["a state diagram", "```mermaid\nstateDiagram-v2\n  [*] --> A\n```"],
    [
      "a sequence diagram in 4a",
      "```mermaid\nsequenceDiagram\n  A->>B: hi\n```",
    ],
    ["an elk flowchart", "```mermaid\nflowchart-elk TD\n  A --> B\n```"],
    ["an unclosed fence", "```mermaid\nflowchart TD\n  A --> B"],
    [
      "a flowchart in a blockquote",
      "> ```mermaid\n> flowchart TD\n>   A --> B\n> ```",
    ],
    [
      "a flowchart in a list item",
      "- ```mermaid\n  flowchart TD\n    A --> B\n  ```",
    ],
    ["a flowchart in another language's fence", "```js\nflowchart TD\n```"],
  ])("never calls the import for %s", async (_name, doc) => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const layer = diagramLayerWith(load);
    const view = mount(doc, layer.extension());
    await flush();
    expect(load).not.toHaveBeenCalled();
    view.destroy();
  });

  it("calls the import once for two views and reconfigures both", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const layer = diagramLayerWith(load);
    const one = mount(FLOW, layer.extension());
    const two = mount(`${FLOW}\n\nmore`, layer.extension());
    expect(layer.compartment.get(one.state)).toEqual([]);
    await flush();
    expect(load).toHaveBeenCalledTimes(1);
    for (const view of [one, two]) {
      expect(view.contentDOM.getAttribute("data-diagram")).toBe("on");
      expect(view.state.facet(diagramLoaded)).toBe(true);
    }
    one.destroy();
    two.destroy();
  });

  it("starts a view built after the load with the layer already in", async () => {
    const layer = diagramLayerWith(() => Promise.resolve(chunk()));
    await layer.prefetch();
    const view = mount("plain", layer.extension());
    expect(view.contentDOM.getAttribute("data-diagram")).toBe("on");
    view.destroy();
  });

  it("picks the layer up when a flowchart fence appears later", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const layer = diagramLayerWith(load);
    const view = mount("plain", layer.extension());
    await flush();
    expect(load).not.toHaveBeenCalled();
    view.dispatch({ changes: { from: 5, insert: `\n\n${FLOW}` } });
    await flush();
    expect(load).toHaveBeenCalledTimes(1);
    expect(view.contentDOM.getAttribute("data-diagram")).toBe("on");
    view.destroy();
  });

  it("picks it up when a fence is typed one key at a time, last line first", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const layer = diagramLayerWith(load);
    const view = mount("Intro\n\n", layer.extension());
    // The body first, then the marks around it: the closing mark is what makes it.
    const steps = ["flowchart TD\n  A --> B\n", "```\n"];
    for (const text of steps) {
      for (const letter of text) {
        view.dispatch({
          changes: { from: view.state.doc.length, insert: letter },
        });
      }
    }
    view.dispatch({ changes: { from: 7, insert: "```mermaid\n" } });
    await flush();
    expect(load).toHaveBeenCalledTimes(1);
    view.destroy();
  });

  it("picks it up from a parse step that reaches a fence below the first parse", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const layer = diagramLayerWith(load);
    const filler = "A paragraph of filler text.\n\n".repeat(4000);
    const view = new EditorView({
      state: EditorState.create({
        doc: `${filler}${FLOW}\n`,
        extensions: [...baseExtensions(false), layer.extension()],
      }),
      parent: document.body,
    });
    expect(load).not.toHaveBeenCalled();
    const { forceParsing } = await import("@codemirror/language");
    forceParsing(view, view.state.doc.length, 5_000);
    await flush();
    expect(load).toHaveBeenCalledTimes(1);
    view.destroy();
  });

  it("asks nothing for typing in a page with no fence", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const layer = diagramLayerWith(load);
    const view = mount("Intro\n\nA line", layer.extension());
    for (const letter of "more words") {
      view.dispatch({
        changes: { from: view.state.doc.length, insert: letter },
      });
    }
    await flush();
    expect(load).not.toHaveBeenCalled();
    view.destroy();
  });

  it("keeps the layer when Raw is flipped on and off again", async () => {
    const layer = diagramLayerWith(() => Promise.resolve(chunk()));
    const preview = new Compartment();
    const view = mount(FLOW, preview.of([layer.extension()]));
    await flush();
    expect(view.contentDOM.getAttribute("data-diagram")).toBe("on");
    view.dispatch({ effects: preview.reconfigure([RAW_MONO]) });
    expect(view.contentDOM.getAttribute("data-diagram")).toBeNull();
    view.dispatch({ effects: preview.reconfigure([layer.extension()]) });
    expect(view.contentDOM.getAttribute("data-diagram")).toBe("on");
    view.destroy();
  });

  it("stays quiet while a failed import waits, and an edit is no new try", async () => {
    vi.useFakeTimers();
    try {
      const load = vi.fn(() => Promise.reject(new Error("offline")));
      const layer = diagramLayerWith(load);
      const view = mount(FLOW, layer.extension());
      await vi.advanceTimersByTimeAsync(0);
      expect(load).toHaveBeenCalledTimes(1);
      expect(view.state.facet(diagramLoaded)).toBe(false);
      view.dispatch({ changes: { from: view.state.doc.length, insert: "\n" } });
      await vi.advanceTimersByTimeAsync(0);
      expect(load).toHaveBeenCalledTimes(1);
      view.destroy();
    } finally {
      vi.useRealTimers();
    }
  });

  it("tries again after 5 s and after 30 s, and loads on a later try", async () => {
    vi.useFakeTimers();
    try {
      let failing = 2;
      const load = vi.fn(() => {
        if (failing > 0) {
          failing -= 1;
          return Promise.reject(new Error("offline"));
        }
        return Promise.resolve(chunk());
      });
      const layer = diagramLayerWith(load);
      const view = mount(FLOW, layer.extension());
      await vi.advanceTimersByTimeAsync(0);
      expect(load).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(4_999);
      expect(load).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(1);
      expect(load).toHaveBeenCalledTimes(2);
      await vi.advanceTimersByTimeAsync(29_999);
      expect(load).toHaveBeenCalledTimes(2);
      await vi.advanceTimersByTimeAsync(1);
      expect(load).toHaveBeenCalledTimes(3);
      expect(view.state.facet(diagramLoaded)).toBe(true);
      view.destroy();
    } finally {
      vi.useRealTimers();
    }
  });

  it("gives up quietly after three failures, draws nothing, and a later edit tries again", async () => {
    vi.useFakeTimers();
    try {
      let failing = true;
      const load = vi.fn(() =>
        failing
          ? Promise.reject(new Error("offline"))
          : Promise.resolve(chunk()),
      );
      const layer = diagramLayerWith(load);
      const view = mount(FLOW, layer.extension());
      await vi.advanceTimersByTimeAsync(40_000);
      expect(load).toHaveBeenCalledTimes(3);
      expect(view.state.facet(diagramLoaded)).toBe(false);
      expect(view.contentDOM.getAttribute("data-diagram")).toBeNull();
      // Nothing keeps trying by itself.
      await vi.advanceTimersByTimeAsync(120_000);
      expect(load).toHaveBeenCalledTimes(3);
      // A failed import is not a verdict: the next edit asks again.
      failing = false;
      view.dispatch({ changes: { from: view.state.doc.length, insert: "\n" } });
      await vi.advanceTimersByTimeAsync(0);
      expect(load).toHaveBeenCalledTimes(4);
      expect(view.state.facet(diagramLoaded)).toBe(true);
      await expect(layer.prefetch()).resolves.toBeUndefined();
      view.destroy();
    } finally {
      vi.useRealTimers();
    }
  });
});
