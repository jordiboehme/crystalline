/**
 * How a long document's footnotes are read off the page's thread: the version
 * check, the mapping of a late reply, one read in flight, the worker's end
 * with the view, and the in-thread read where there is no worker.
 */

import { EditorSelection, EditorState, Text } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";

import { parsedState } from "../test/parse";
import type { NotesWorker } from "./footnotes";
import { footnoteModel, footnoteScans, footnotes } from "./footnotes";
import { readNotes } from "./footnoteSyntax";
import { hiddenMarkLayerOn } from "./marks/mode";
import { baseExtensions } from "./setup";

const views: EditorView[] = [];

afterEach(() => {
  for (const view of views.splice(0)) {
    view.destroy();
  }
});

/** A document over the in-thread limit, with 100 notes. */
const LONG = (() => {
  const lines: string[] = [];
  for (let n = 0; n < 400; n += 1) {
    lines.push(
      n < 100 ? `Line ${n} has a note.[^${n}]` : `Plain line ${n} of prose.`,
    );
  }
  lines.push("");
  for (let n = 0; n < 100; n += 1) {
    lines.push(`[^${n}]: Note ${n}.`);
  }
  return lines.join("\n");
})();

/** A worker the test answers by hand. */
class FakeWorker implements NotesWorker {
  posts: { id: number; text: string }[] = [];
  terminated = false;
  onmessage: NotesWorker["onmessage"] = null;
  onerror: NotesWorker["onerror"] = null;

  postMessage(message: { id: number; text: string }) {
    this.posts.push(message);
  }

  terminate() {
    this.terminated = true;
  }

  /** Answers the post at `index` with the real read of its text. */
  reply(index: number) {
    const post = this.posts[index];
    if (!post) {
      throw new Error("no such post");
    }
    this.onmessage?.({
      data: {
        id: post.id,
        notes: readNotes(Text.of(post.text.split("\n"))),
      },
    });
  }
}

function mount(doc: string, worker: (() => NotesWorker | null) | null) {
  const view = new EditorView({
    state: parsedState(
      EditorState.create({
        doc,
        selection: EditorSelection.cursor(0),
        extensions: [
          ...baseExtensions(false),
          hiddenMarkLayerOn.of(true),
          footnotes(worker ? { worker } : {}),
        ],
      }),
    ),
    parent: document.body,
  });
  views.push(view);
  return view;
}

/** The references of the viewport drawn (jsdom draws the first screenful). */
const drawn = (view: EditorView) =>
  view.dom.querySelectorAll(".cm-footnote-ref").length;

/** The references the model holds, drawn or not. */
const known = (view: EditorView) => footnoteModel(view.state).refs.length;

function withWorker(doc = LONG) {
  const worker = new FakeWorker();
  const factory = vi.fn(() => worker);
  const view = mount(doc, factory);
  return { worker, factory, view };
}

describe("a long document read in a worker", () => {
  it("hands the text over at mount, reads nothing itself and shows text meanwhile", () => {
    const before = footnoteScans();
    const { worker, view } = withWorker();
    expect(footnoteScans()).toBe(before);
    expect(worker.posts).toHaveLength(1);
    expect(worker.posts[0]?.text).toBe(LONG);
    expect(drawn(view)).toBe(0);
    expect(view.dom.textContent).toContain("[^0]");
  });

  it("draws the numbers when the reply comes", () => {
    const { worker, view } = withWorker();
    worker.reply(0);
    expect(known(view)).toBe(100);
    expect(drawn(view)).toBeGreaterThan(0);
    expect(view.dom.textContent).not.toContain("[^0]");
    expect(footnoteModel(view.state).refs[0]).toEqual({
      label: "[^0]",
      number: 1,
    });
  });

  it("applies a reply no edit has touched and maps one that edits could not change", () => {
    const { worker, view } = withWorker();
    // A letter inside a word in the prose: no footnote can change,
    // but every offset in the reply moves by one.
    view.dispatch({
      changes: { from: LONG.indexOf("Plain line 200") + 3, insert: "X" },
      userEvent: "input.type",
    });
    worker.reply(0);
    const model = footnoteModel(view.state);
    expect(model.refs).toHaveLength(100);
    expect(model.refs.every((ref) => /^\[\^\d+\]$/.test(ref.label))).toBe(true);
    expect(model.notes[0]?.text).toBe("Note 0.");
    expect(drawn(view)).toBeGreaterThan(0);
  });

  it("drops a reply when an edit that can change footnotes came after it", async () => {
    const { worker, view } = withWorker();
    view.dispatch({
      changes: { from: 0, insert: "[^7]" },
      userEvent: "input.type",
    });
    worker.reply(0);
    expect(drawn(view)).toBe(0);
    // It asks again, after the pause, for the text as it is now.
    await vi.waitFor(() => {
      expect(worker.posts).toHaveLength(2);
    });
    expect(worker.posts[1]?.text.startsWith("[^7]Line 0")).toBe(true);
    worker.reply(1);
    expect(known(view)).toBe(101);
  });

  it("keeps one read in flight whatever is typed meanwhile", async () => {
    const { worker, view } = withWorker();
    for (const insert of ["[^1]", "[^2]", "[^3]"]) {
      view.dispatch({ changes: { from: 0, insert }, userEvent: "input.type" });
    }
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(worker.posts).toHaveLength(1);
  });

  it("ends the worker with the view and ignores a late reply", () => {
    const { worker, view } = withWorker();
    view.destroy();
    expect(worker.terminated).toBe(true);
    expect(() => worker.reply(0)).not.toThrow();
  });

  it("reads inside the thread, later, when the worker fails", async () => {
    const { worker, view } = withWorker();
    worker.onerror?.(new Error("no"));
    expect(worker.terminated).toBe(true);
    await vi.waitFor(() => {
      expect(known(view)).toBe(100);
    });
  });

  it("never starts a worker for a short document", () => {
    const factory = vi.fn(() => new FakeWorker());
    const view = mount("a[^n]\n\n[^n]: N", factory);
    expect(factory).not.toHaveBeenCalled();
    expect(drawn(view)).toBe(1);
  });
});

describe("a long document with no worker", () => {
  it("reads in the thread a tick after mount, not inside it", async () => {
    const before = footnoteScans();
    const view = mount(LONG, null);
    expect(footnoteScans()).toBe(before);
    expect(drawn(view)).toBe(0);
    await vi.waitFor(() => {
      expect(known(view)).toBe(100);
    });
    expect(footnoteScans()).toBe(before + 1);
  });

  it("falls back when the factory answers null", async () => {
    const view = mount(LONG, () => null);
    await vi.waitFor(() => {
      expect(known(view)).toBe(100);
    });
  });

  it("stops a pending read when the view is destroyed", async () => {
    const before = footnoteScans();
    const view = mount(LONG, null);
    view.destroy();
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(footnoteScans()).toBe(before);
  });
});
