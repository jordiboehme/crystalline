/**
 * A read that throws in the thread leaves the next relevant edit to try again,
 * instead of the view never reading once more.
 */

import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";

import { parsedState } from "../test/parse";

const control = vi.hoisted(() => ({ fail: true }));

vi.mock("./footnoteSyntax", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./footnoteSyntax")>();
  return {
    ...actual,
    readNotes: (doc: Parameters<typeof actual.readNotes>[0]) => {
      if (control.fail) {
        throw new Error("a read that fails");
      }
      return actual.readNotes(doc);
    },
  };
});

const { footnoteModel, footnotes } = await import("./footnotes");
const { baseExtensions } = await import("./setup");
const { hiddenMarkLayerOn } = await import("./marks/mode");

const views: EditorView[] = [];

afterEach(() => {
  for (const view of views.splice(0)) {
    view.destroy();
  }
});

const LONG = `${"Plain prose line.\n".repeat(400)}\nA note.[^1]\n\n[^1]: One.\n`;

describe("a read that throws", () => {
  it("is tried again on the next edit that can change footnotes", async () => {
    control.fail = true;
    const view = new EditorView({
      state: parsedState(
        EditorState.create({
          doc: LONG,
          selection: EditorSelection.cursor(0),
          extensions: [
            ...baseExtensions(false),
            hiddenMarkLayerOn.of(true),
            footnotes(),
          ],
        }),
      ),
      parent: document.body,
    });
    views.push(view);
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(footnoteModel(view.state).refs).toEqual([]);
    control.fail = false;
    view.dispatch({
      changes: { from: 0, insert: "[^1]" },
      userEvent: "input.type",
    });
    await vi.waitFor(() => {
      expect(footnoteModel(view.state).refs.length).toBeGreaterThan(0);
    });
  });
});
