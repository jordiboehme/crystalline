/**
 * The lazy hand-over: the chunk is asked for only when a table the grid draws
 * shows up, once for every view, and what it brings survives the Raw flip.
 */

import { Compartment, EditorState } from "@codemirror/state";
import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it, vi } from "vitest";

import { parsedState } from "../../test/parse";
import { reflectionAdapter } from "../adapter";
import { previewConfig } from "../surface";
import { baseExtensions, RAW_MONO } from "../setup";
import { cellPreviewLayers, gridLoaded } from "./draws";
import { tableGridWith } from "./loader";

const TABLE = "| a | b |\n| - | - |\n| 1 | 2 |";

/** A stand-in for the lazy chunk whose extension leaves a visible mark. */
function chunk() {
  const marker: Extension = EditorView.contentAttributes.of({
    "data-grid": "on",
  });
  return { gridExtension: () => [marker, gridLoaded.of(true)] };
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

describe("tableGridWith", () => {
  it("never calls the import for a state without a table", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const grid = tableGridWith(load);
    const view = mount("just prose\n\nand more", grid.extension());
    await flush();
    expect(load).not.toHaveBeenCalled();
    view.destroy();
  });

  it("does not count a table in a blockquote", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const grid = tableGridWith(load);
    const view = mount("> | a |\n> | - |\n> | 1 |", grid.extension());
    await flush();
    expect(load).not.toHaveBeenCalled();
    view.destroy();
  });

  it("calls the import once for two views and reconfigures both", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const grid = tableGridWith(load);
    const one = mount(TABLE, grid.extension());
    const two = mount(TABLE, grid.extension());
    expect(grid.compartment.get(one.state)).toEqual([]);
    await flush();
    expect(load).toHaveBeenCalledTimes(1);
    for (const view of [one, two]) {
      expect(grid.compartment.get(view.state)).not.toEqual([]);
      expect(view.contentDOM.getAttribute("data-grid")).toBe("on");
      expect(view.state.facet(gridLoaded)).toBe(true);
    }
    one.destroy();
    two.destroy();
  });

  it("starts a view built after the load with the grid already in", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const grid = tableGridWith(load);
    await grid.prefetch();
    const view = mount("plain", grid.extension());
    expect(view.contentDOM.getAttribute("data-grid")).toBe("on");
    view.destroy();
  });

  it("picks the grid up in a view that appeared before the load and sees a table later", async () => {
    const load = vi.fn(() => Promise.resolve(chunk()));
    const grid = tableGridWith(load);
    const view = mount("plain", grid.extension());
    await grid.prefetch();
    expect(view.contentDOM.getAttribute("data-grid")).toBeNull();
    view.dispatch({ changes: { from: 5, insert: `\n\n${TABLE}` } });
    await flush();
    expect(view.contentDOM.getAttribute("data-grid")).toBe("on");
    view.destroy();
  });

  it("keeps the grid when Raw is flipped on and off again", async () => {
    const grid = tableGridWith(() => Promise.resolve(chunk()));
    const preview = new Compartment();
    const view = mount(TABLE, preview.of([grid.extension()]));
    await flush();
    expect(view.contentDOM.getAttribute("data-grid")).toBe("on");
    view.dispatch({ effects: preview.reconfigure([RAW_MONO]) });
    expect(view.contentDOM.getAttribute("data-grid")).toBeNull();
    view.dispatch({ effects: preview.reconfigure([grid.extension()]) });
    expect(view.contentDOM.getAttribute("data-grid")).toBe("on");
    view.destroy();
  });

  it("stays quiet and keeps the preview when the import rejects", async () => {
    const load = vi.fn(() => Promise.reject(new Error("offline")));
    const grid = tableGridWith(load);
    const view = mount(TABLE, grid.extension());
    await flush();
    expect(load).toHaveBeenCalled();
    expect(view.state.facet(gridLoaded)).toBe(false);
    await expect(grid.prefetch()).resolves.toBeUndefined();
    view.destroy();
  });
});

describe("previewConfig", () => {
  it("hands the cell editor the same prose layers the main view gets", () => {
    const adapter = reflectionAdapter();
    const state = EditorState.create({
      doc: "x",
      extensions: [
        ...baseExtensions(false),
        previewConfig(false, false, [], adapter),
      ],
    });
    // elementPreviews, livePreview and the adapter's own layers
    expect(state.facet(cellPreviewLayers).length).toBe(
      2 + adapter.previewLayers.length,
    );
  });

  it("provides no layers in Raw", () => {
    const state = EditorState.create({
      doc: "x",
      extensions: [
        ...baseExtensions(false),
        previewConfig(true, false, [], reflectionAdapter()),
      ],
    });
    expect(state.facet(cellPreviewLayers)).toEqual([]);
  });
});
