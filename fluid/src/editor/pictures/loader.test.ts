/**
 * The lazy hand-over of the picture tools: the chunk is asked for only when
 * the tree holds an image or a file is dragged in, once for every view, and
 * what it brings survives the Raw flip. A failed import leaves the pictures
 * drawn and selectable by a press, with no keys.
 */

import { syntaxTree } from "@codemirror/language";
import { Compartment, EditorState } from "@codemirror/state";
import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { Tree } from "@lezer/common";
import { afterEach, describe, expect, it, vi } from "vitest";

import { parsedState, parsedView } from "../../test/parse";
import { imagePreviews } from "../imagePreviews";
import { baseExtensions, RAW_MONO } from "../setup";
import { pictureToolsWith } from "./loader";

const PICTURE = "a ![x](assets/p.png) b";

/** A stand-in for the lazy chunk whose extension leaves a visible mark. */
function chunk() {
  const marker: Extension = EditorView.contentAttributes.of({
    "data-picture-tools": "on",
  });
  return { pictureToolsExtension: () => marker };
}

const views: EditorView[] = [];

afterEach(() => {
  while (views.length > 0) views.pop()?.destroy();
});

function mount(doc: string, extension: Extension): EditorView {
  const view = new EditorView({
    state: parsedState(
      EditorState.create({
        doc,
        extensions: [...baseExtensions(false), extension],
      }),
    ),
    parent: document.body,
  });
  views.push(view);
  return view;
}

const flush = async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};

const tools = (view: EditorView) =>
  view.contentDOM.getAttribute("data-picture-tools");

/** A `dragenter` as a browser sends one for a file dragged in from outside. */
function dragFiles(view: EditorView): void {
  const event = new Event("dragenter", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "dataTransfer", {
    value: { types: ["Files"], files: [] },
  });
  view.contentDOM.dispatchEvent(event);
}

describe("pictureToolsWith", () => {
  it("never calls the import for a state without an image", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const loader = pictureToolsWith(load);
    mount("just prose\n\nand [a link](x)", loader.extension());
    await flush();
    expect(load).not.toHaveBeenCalled();
  });

  it("does not count an image inside a code fence", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const loader = pictureToolsWith(load);
    mount("```\n![x](assets/p.png)\n```", loader.extension());
    await flush();
    expect(load).not.toHaveBeenCalled();
  });

  it("calls the import once for two views and reconfigures both", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const loader = pictureToolsWith(load);
    const one = mount(PICTURE, loader.extension());
    const two = mount(PICTURE, loader.extension());
    expect(loader.compartment.get(one.state)).toEqual([]);
    await flush();
    expect(load).toHaveBeenCalledTimes(1);
    for (const view of [one, two]) {
      expect(tools(view)).toBe("on");
    }
  });

  it("loads on a dragenter carrying files on a page with no picture", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const loader = pictureToolsWith(load);
    const view = mount("plain", loader.extension());
    await flush();
    expect(load).not.toHaveBeenCalled();
    // Dragged text is no reason to load.
    const text = new Event("dragenter", { bubbles: true, cancelable: true });
    Object.defineProperty(text, "dataTransfer", {
      value: { types: ["text/plain"], files: [] },
    });
    view.contentDOM.dispatchEvent(text);
    await flush();
    expect(load).not.toHaveBeenCalled();
    dragFiles(view);
    await flush();
    expect(load).toHaveBeenCalledTimes(1);
    expect(tools(view)).toBe("on");
  });

  it("picks the tools up when an image is typed later", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const loader = pictureToolsWith(load);
    const view = mount("plain", loader.extension());
    await flush();
    view.dispatch({ changes: { from: 5, insert: " ![x](assets/p.png)" } });
    parsedView(view);
    await flush();
    expect(load).toHaveBeenCalledTimes(1);
    expect(tools(view)).toBe("on");
  });

  it("starts a view built after the load with the tools already in", async () => {
    const loader = pictureToolsWith(() => Promise.resolve(chunk()));
    await loader.prefetch();
    const view = mount("plain", loader.extension());
    expect(tools(view)).toBe("on");
  });

  it("keeps the tools when Raw is flipped on and off again", async () => {
    const loader = pictureToolsWith(() => Promise.resolve(chunk()));
    const preview = new Compartment();
    const view = mount(PICTURE, preview.of([loader.extension()]));
    await flush();
    expect(tools(view)).toBe("on");
    view.dispatch({ effects: preview.reconfigure([RAW_MONO]) });
    expect(tools(view)).toBeNull();
    view.dispatch({ effects: preview.reconfigure([loader.extension()]) });
    expect(tools(view)).toBe("on");
  });

  it("keeps drawing and selecting by a press when the import fails", async () => {
    vi.useFakeTimers();
    try {
      const load = vi.fn(() => Promise.reject(new Error("offline")));
      const loader = pictureToolsWith(load);
      const view = mount(PICTURE, [imagePreviews("eng"), loader.extension()]);
      await vi.advanceTimersByTimeAsync(40_000);
      expect(load).toHaveBeenCalledTimes(3);
      expect(tools(view)).toBeNull();
      const box = view.contentDOM.querySelector(".cm-picture");
      expect(box).not.toBeNull();
      box?.dispatchEvent(
        new MouseEvent("mousedown", {
          bubbles: true,
          cancelable: true,
          button: 0,
        }),
      );
      box?.dispatchEvent(
        new MouseEvent("mouseup", { bubbles: true, button: 0 }),
      );
      expect(view.state.selection.main.from).toBe(2);
      expect(view.state.selection.main.to).toBe(20);
      await expect(loader.prefetch()).resolves.toBeUndefined();
    } finally {
      vi.useRealTimers();
    }
  });

  it("loads when only a later parse step reveals an image", () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const loader = pictureToolsWith(load);
    const doc = `${"Paragraph with **bold** text.\n\n".repeat(5000)}![x](assets/p.png)`;
    // A clock that jumps at every look stops the first parse short, as a
    // busy page does (the technique of marks/parse.test.ts).
    let ahead = 0;
    const base = Date.now();
    const clock = vi.spyOn(Date, "now").mockImplementation(() => {
      ahead += 1e6;
      return base + ahead;
    });
    const view = new EditorView({
      state: EditorState.create({
        doc,
        extensions: [...baseExtensions(false), loader.extension()],
      }),
      parent: document.body,
    });
    clock.mockRestore();
    views.push(view);
    expect(syntaxTree(view.state).length).toBeLessThan(doc.length / 2);
    expect(load).not.toHaveBeenCalled();
    // Only the tree moves on: no change of the text or the viewport.
    parsedView(view);
    expect(load).toHaveBeenCalledTimes(1);
  });

  it("gives the tools to a view built before the load once a picture arrives", async () => {
    const loader = pictureToolsWith(() => Promise.resolve(chunk()));
    const view = mount("plain text", loader.extension());
    await loader.prefetch();
    await flush();
    expect(tools(view)).toBeNull();
    view.dispatch({ changes: { from: 0, insert: "x" } });
    view.dispatch({ changes: { from: 0, insert: "![x](assets/p.png) " } });
    parsedView(view);
    await flush();
    expect(tools(view)).toBe("on");
  });

  it("walks no tree once the tools are in", async () => {
    const loader = pictureToolsWith(() => Promise.resolve(chunk()));
    await loader.prefetch();
    const walks = (extensions: Extension[]) => {
      const view = mount("plain text", extensions);
      const spy = vi.spyOn(Tree.prototype, "iterate");
      for (let i = 0; i < 5; i += 1) {
        view.dispatch({ changes: { from: 0, insert: "x" } });
      }
      const count = spy.mock.calls.length;
      spy.mockRestore();
      return count;
    };
    expect(walks([loader.extension()])).toBe(walks([]));
  });
});
