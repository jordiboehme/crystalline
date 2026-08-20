/**
 * The format menu's two verbs, and the question that decides whether the menu
 * is drawn at all: is the caret on an attachment image?
 *
 * Every rewrite is the target and nothing else - the alt text an author wrote
 * is theirs, and a verb that rebuilt the whole reference would quietly
 * normalize it. Placement and width compose rather than replace each other,
 * because a menu that dropped the width when the placement changed would make
 * the second click undo the first.
 */

import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it } from "vitest";

import { parsedState } from "../test/parse";
import {
  clearImageFormat,
  imageContextAt,
  imageContextListener,
  setImageAlign,
  setImageWidth,
} from "./imageVerbs";
import { baseExtensions } from "./setup";

/** A buffer with the caret at `at`, parsed far enough to answer about it. */
function editor(doc: string, at: number): EditorView {
  return new EditorView({
    state: parsedState(
      EditorState.create({
        doc,
        selection: EditorSelection.cursor(at),
        extensions: baseExtensions(false),
      }),
    ),
    parent: document.body,
  });
}

/** Where the caret has to be for the menu: inside the reference. */
function inside(doc: string, needle: string): number {
  return doc.indexOf(needle) + 2;
}

describe("imageContextAt", () => {
  it("finds the attachment image the caret sits on", () => {
    const doc = "text\n\n![Shot](assets/a.png#right)\n";
    const view = editor(doc, inside(doc, "![Shot]"));
    const context = imageContextAt(view.state);
    expect(context?.path).toBe("assets/a.png");
    expect(context?.format).toEqual({ align: "right" });
    expect(doc.slice(context?.targetFrom ?? 0, context?.targetTo ?? 0)).toBe(
      "assets/a.png#right",
    );
    view.destroy();
  });

  it("says nothing about prose, an external image or a document link", () => {
    for (const doc of [
      "just prose here\n",
      "![out](https://example.com/a.png)\n",
      "[deck](assets/deck.pdf)\n",
    ]) {
      const view = editor(doc, 3);
      expect(imageContextAt(view.state)).toBeNull();
      view.destroy();
    }
  });

  it("says nothing when the caret is elsewhere on the line", () => {
    const doc = "![Shot](assets/a.png) and then some prose\n";
    const view = editor(doc, doc.length - 4);
    expect(imageContextAt(view.state)).toBeNull();
    view.destroy();
  });
});

describe("the format verbs", () => {
  it("writes a placement into a bare target", () => {
    const doc = "![Shot](assets/a.png)\n";
    const view = editor(doc, inside(doc, "![Shot]"));
    expect(setImageAlign(view, "right")).toBe(true);
    expect(view.state.doc.toString()).toBe("![Shot](assets/a.png#right)\n");
    view.destroy();
  });

  it("clears the fragment when the placement goes back to centered", () => {
    const doc = "![Shot](assets/a.png#left)\n";
    const view = editor(doc, inside(doc, "![Shot]"));
    expect(setImageAlign(view, "center")).toBe(true);
    expect(view.state.doc.toString()).toBe("![Shot](assets/a.png)\n");
    view.destroy();
  });

  it("keeps the width when the placement changes, and the placement when the width does", () => {
    const doc = "![Shot](assets/a.png#w=50%)\n";
    const view = editor(doc, inside(doc, "![Shot]"));
    setImageAlign(view, "left");
    expect(view.state.doc.toString()).toBe(
      "![Shot](assets/a.png#left,w=50%)\n",
    );
    setImageWidth(view, "25%");
    expect(view.state.doc.toString()).toBe(
      "![Shot](assets/a.png#left,w=25%)\n",
    );
    view.destroy();
  });

  it("rewrites the target and leaves the alt text alone", () => {
    const doc = "![A picture (mine)](assets/a.png)\n";
    const view = editor(doc, inside(doc, "![A"));
    setImageAlign(view, "full");
    expect(view.state.doc.toString()).toBe(
      "![A picture (mine)](assets/a.png#full)\n",
    );
    view.destroy();
  });

  it("keeps the ./ the author wrote rather than normalizing it away", () => {
    const doc = "![Shot](./assets/a.png)\n";
    const view = editor(doc, inside(doc, "![Shot]"));
    expect(setImageAlign(view, "right")).toBe(true);
    expect(view.state.doc.toString()).toBe("![Shot](./assets/a.png#right)\n");
    view.destroy();
  });

  it("leaves a title clause standing beside the target it rewrites", () => {
    const doc = '![Shot](assets/a.png "Q3 deck")\n';
    const view = editor(doc, inside(doc, "![Shot]"));
    expect(setImageAlign(view, "left")).toBe(true);
    expect(view.state.doc.toString()).toBe(
      '![Shot](assets/a.png#left "Q3 deck")\n',
    );
    view.destroy();
  });

  it("refuses where there is no image to format", () => {
    const view = editor("plain prose\n", 3);
    expect(setImageAlign(view, "left")).toBe(false);
    expect(setImageWidth(view, "50%")).toBe(false);
    expect(view.state.doc.toString()).toBe("plain prose\n");
    view.destroy();
  });

  it("clears a width when asked for none", () => {
    const doc = "![Shot](assets/a.png#right,w=75%)\n";
    const view = editor(doc, inside(doc, "![Shot]"));
    expect(setImageWidth(view, null)).toBe(true);
    expect(view.state.doc.toString()).toBe("![Shot](assets/a.png#right)\n");
    view.destroy();
  });

  it("Centered means as it was: the whole fragment goes, width and all", () => {
    const doc = "![Shot](assets/a.png#left,w=25%)\n";
    const view = editor(doc, inside(doc, "![Shot]"));
    expect(clearImageFormat(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("![Shot](assets/a.png)\n");
    view.destroy();
  });

  it("rewrites the image the caret is on when a line carries two", () => {
    const doc = "![a](assets/a.png) ![b](assets/b.png)\n";
    const view = editor(doc, doc.indexOf("![b]") + 2);
    setImageAlign(view, "left");
    expect(view.state.doc.toString()).toBe(
      "![a](assets/a.png) ![b](assets/b.png#left)\n",
    );
    view.destroy();
  });
});

describe("imageContextListener", () => {
  /** A buffer built with the listener on it, reporting into `seen`. */
  function watched(doc: string, at: number, seen: boolean[]): EditorView {
    return new EditorView({
      state: parsedState(
        EditorState.create({
          doc,
          selection: EditorSelection.cursor(at),
          extensions: [
            ...baseExtensions(false),
            imageContextListener((onImage) => seen.push(onImage)),
          ],
        }),
      ),
      parent: document.body,
    });
  }

  it("answers for the state the buffer mounted in, before anything moves", () => {
    // A buffer can open with the caret already inside a reference, and a menu
    // that waited for a keystroke to notice would simply not be there.
    const doc = "![Shot](assets/a.png)\n";
    const seen: boolean[] = [];
    const view = watched(doc, doc.indexOf("![Shot]") + 2, seen);
    expect(seen).toEqual([true]);
    view.destroy();
  });

  it("says nothing at mount when the caret is in prose", () => {
    const seen: boolean[] = [];
    const view = watched("plain prose\n", 3, seen);
    expect(seen).toEqual([false]);
    view.destroy();
  });

  it("reports crossings only, not every move", () => {
    const doc = "![Shot](assets/a.png) and prose\n";
    const seen: boolean[] = [];
    const view = watched(doc, 2, seen);
    view.dispatch({ selection: { anchor: doc.length - 4 } });
    view.dispatch({ selection: { anchor: doc.length - 6 } });
    view.dispatch({ selection: { anchor: 3 } });
    expect(seen).toEqual([true, false, true]);
    view.destroy();
  });
});
