/**
 * Every picture gesture is one undo step of its own, solo and in a room,
 * even when it comes right after typing: nothing in these tests closes the
 * room's capture window by hand.
 */

import { isolateHistory } from "@codemirror/commands";
import {
  Annotation,
  EditorSelection,
  EditorState,
  Transaction,
} from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import { bytes, cleanupMarkViews, markView, undoOnce } from "../test/markKit";
import { commandJudge } from "./marks/commandJudge";
import { dispatchGesture } from "./gesture";
import { setPictureAlign, setPictureSize } from "./pictures/verbs";
import { baseExtensions } from "./setup";

afterEach(cleanupMarkViews);

const DOC = "![a](assets/a.png)\n";

describe("dispatchGesture", () => {
  for (const room of [true, false]) {
    it(`makes typing, a size and a placement three undo steps ${room ? "in a room" : "solo"}`, () => {
      const view = markView(DOC, { room });
      view.dispatch({ selection: EditorSelection.cursor(0) });
      const states: string[] = [bytes(view.state)];
      for (const char of "abc") {
        const at = view.state.selection.main.head;
        view.dispatch({
          changes: { from: at, insert: char },
          selection: EditorSelection.cursor(at + 1),
          userEvent: "input.type",
        });
      }
      expect(bytes(view.state)).toBe(`abc${DOC}`);
      states.push(bytes(view.state));
      const started = performance.now();
      expect(setPictureSize(view, "small")).toBe(true);
      expect(bytes(view.state)).toBe("abc![a](assets/a.png#w=25%)\n");
      states.push(bytes(view.state));
      expect(setPictureAlign(view, "right")).toBe(true);
      expect(bytes(view.state)).toBe("abc![a](assets/a.png#right,w=25%)\n");
      expect(performance.now() - started).toBeLessThan(100);
      undoOnce(view);
      expect(bytes(view.state)).toBe(states[2]);
      undoOnce(view);
      expect(bytes(view.state)).toBe(states[1]);
      undoOnce(view);
      expect(bytes(view.state)).toBe(states[0]);
    });
  }

  it("judges a waiting composition before it dispatches", () => {
    const seen: string[] = [];
    const view = new EditorView({
      state: EditorState.create({
        doc: "x\n",
        extensions: [
          ...baseExtensions(false),
          commandJudge.of(() => seen.push("judge")),
          EditorView.updateListener.of((update) => {
            for (const tr of update.transactions) {
              const event = tr.annotation(Transaction.userEvent);
              if (event !== undefined) seen.push(event);
            }
          }),
        ],
      }),
      parent: document.body,
    });
    dispatchGesture(view, {
      changes: { from: 0, insert: "y" },
      userEvent: "input.format",
    });
    expect(seen).toEqual(["judge", "input.format"]);
    expect(view.state.doc.toString()).toBe("yx\n");
    view.destroy();
  });

  it("dispatches nothing when the judge changed the text under it", () => {
    const view = new EditorView({
      state: EditorState.create({
        doc: "x\n",
        extensions: [
          ...baseExtensions(false),
          commandJudge.of((v) => {
            if (v.state.doc.length === 2) {
              v.dispatch({ changes: { from: 0, insert: "j" } });
            }
          }),
        ],
      }),
      parent: document.body,
    });
    dispatchGesture(view, {
      changes: { from: 1, insert: "y" },
      userEvent: "input.format",
    });
    expect(view.state.doc.toString()).toBe("jx\n");
    view.destroy();
  });

  it("keeps the annotations the caller passed and isolates the step", () => {
    const tag = Annotation.define<string>();
    const read: unknown[] = [];
    const view = new EditorView({
      state: EditorState.create({
        doc: "x",
        extensions: [
          ...baseExtensions(false),
          EditorView.updateListener.of((update) => {
            for (const tr of update.transactions) {
              read.push(tr.annotation(tag), tr.annotation(isolateHistory));
            }
          }),
        ],
      }),
      parent: document.body,
    });
    dispatchGesture(view, {
      changes: { from: 0, insert: "y" },
      annotations: tag.of("mine"),
      userEvent: "input.format",
    });
    expect(read).toEqual(["mine", "full"]);
    view.destroy();
  });
});
