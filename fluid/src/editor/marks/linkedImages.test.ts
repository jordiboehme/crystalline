/**
 * Images inside links and images with no alt text (D39, D40).
 *
 * A linked image `[![alt](src)](href)` reads as its alt text, linked: its
 * marks hide, and the keys walk its letters. `[[Target]]` stays the wikilink
 * chip's, and `[[t](u)]` is a link inside literal brackets.
 *
 * An image pair is never dropped as empty: the last alt letter leaves
 * `![](src)`, and an image with no alt text is one atom that a single press
 * removes, as a wikilink chip, and that the caret steps over in one press.
 */

import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { beforeAll, afterEach, describe, expect, it } from "vitest";

import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  undoOnce,
} from "../../test/markKit";
import { parsedState } from "../../test/parse";
import { copyHtmlOf } from "../copyHtml";
import { linkAt, linkChanges } from "../elementVerbs";
import { imagePreviews } from "../imagePreviews";
import { baseExtensions } from "../setup";
import { marksOnLine } from "./hiddenMarks";
import { prefetchPictureTools } from "../pictures/loader";

beforeAll(async () => {
  await prefetchPictureTools();
});

afterEach(cleanupMarkViews);

const LINKED = "a [![alt](src)](href) b";

/** The marks of line 1 as `[from, to)` pairs. */
function spans(view: EditorView, line = 1): [number, number][] {
  return marksOnLine(parsedState(view.state), line).map((mark) => [
    mark.from,
    mark.to,
  ]);
}

/** Where the caret stops after each press of `key`, `times` times. */
function walk(view: EditorView, key: string, times: number): number[] {
  const out: number[] = [];
  for (let i = 0; i < times; i += 1) {
    keys(view, key);
    out.push(caret(view));
  }
  return out;
}

/** The text after each press of `key`, `times` times. */
function presses(view: EditorView, key: string, times: number): string[] {
  const out: string[] = [];
  for (let i = 0; i < times; i += 1) {
    keys(view, key);
    out.push(bytes(view.state));
  }
  return out;
}

describe("a linked image (D39)", () => {
  it("draws its alt text, with every mark hidden", () => {
    const view = markView(LINKED);
    expect(view.contentDOM.textContent).toBe("a alt b");
    expect(spans(view)).toEqual([
      [2, 3],
      [3, 5],
      [8, 14],
      [14, 21],
    ]);
  });

  it("draws its source where the line shows its marks", () => {
    const view = markView(LINKED, { mode: "show" });
    expect(view.contentDOM.textContent).toBe(LINKED);
  });

  it("walks the arrows one visible character at a time", () => {
    const view = markView(LINKED);
    setCaret(view, 0);
    expect(walk(view, "ArrowRight", 7)).toEqual([1, 2, 6, 7, 21, 22, 23]);
    expect(walk(view, "ArrowLeft", 7)).toEqual([22, 21, 7, 6, 2, 1, 0]);
  });

  it("takes the alt letters one Backspace at a time", () => {
    const view = markView(LINKED);
    setCaret(view, 21);
    expect(presses(view, "Backspace", 2)).toEqual([
      "a [![al](src)](href) b",
      "a [![a](src)](href) b",
    ]);
  });

  it("takes the alt letters one Delete at a time", () => {
    const view = markView(LINKED);
    setCaret(view, 0);
    expect(presses(view, "Delete", 4)).toEqual([
      " [![alt](src)](href) b",
      "[![alt](src)](href) b",
      "[![lt](src)](href) b",
      "[![t](src)](href) b",
    ]);
  });

  it("copies as its link with the alt text as its words", () => {
    const view = markView("[![the alt](a.png)](https://x)");
    expect(
      copyHtmlOf(view.state, [EditorSelection.range(0, view.state.doc.length)]),
    ).toBe('<p><a href="https://x">the alt</a></p>');
  });

  it("opens the outer link's popover, and a new address keeps the image", () => {
    const view = markView(LINKED);
    const link = linkAt(parsedState(view.state), 6);
    expect(link).toMatchObject({
      from: 2,
      to: 21,
      text: { from: 3, to: 14 },
      address: { from: 16, to: 20 },
    });
    const changes = linkChanges(view.state, 2, { address: "https://y" });
    expect(changes).not.toBeNull();
    if (changes) view.dispatch({ changes });
    expect(bytes(view.state)).toBe("a [![alt](src)](https://y) b");
  });

  it("still draws the picture under the line", () => {
    const view = new EditorView({
      state: parsedState(
        EditorState.create({
          doc: "[![Shot](assets/a.png)](https://x)\n",
          selection: EditorSelection.cursor(0),
          extensions: [...baseExtensions(false), imagePreviews("eng")],
        }),
      ),
      parent: document.body,
    });
    expect(view.dom.querySelectorAll(".cm-image-preview img")).toHaveLength(1);
    view.destroy();
  });

  it("is undone in one step after a delete", () => {
    const view = markView(LINKED);
    setCaret(view, 21);
    keys(view, "Backspace");
    undoOnce(view);
    expect(bytes(view.state)).toBe(LINKED);
  });
});

describe("brackets around a link (D39)", () => {
  it("draws a link inside literal brackets as the page does", () => {
    const view = markView("see [[t](u)] here");
    expect(view.contentDOM.textContent).toBe("see [t] here");
    expect(spans(view)).toEqual([
      [5, 6],
      [7, 11],
    ]);
  });

  it("leaves [[Target]] to the wikilink chip", () => {
    const view = markView("see [[Target]] here");
    expect(spans(view)).toEqual([]);
    expect(linkAt(parsedState(view.state), 8)).toBeNull();
  });
});

type Mode = "hidden" | "show";

const CASES = (["hidden", "show"] as Mode[]).flatMap((mode) =>
  [false, true].map((room) => ({ mode, room })),
);

describe.each(CASES)(
  "an image is never dropped as empty (D40), $mode mode, room $room",
  ({ mode, room }) => {
    it("leaves ![](src) when Backspace takes the last alt letter", () => {
      const view = markView("x ![alt](src) y", { mode, room });
      // After the alt text: past the closing marks where they hide.
      setCaret(view, mode === "hidden" ? 13 : 7);
      presses(view, "Backspace", 3);
      expect(bytes(view.state)).toBe("x ![](src) y");
      undoOnce(view);
      expect(bytes(view.state)).not.toBe("x ![](src) y");
    });

    it("leaves ![](src) when Delete takes the last alt letter", () => {
      const view = markView("x ![a](src) y", { mode, room });
      setCaret(view, 4);
      keys(view, "Delete");
      expect(bytes(view.state)).toBe("x ![](src) y");
    });

    it("keeps the outer link of a linked image too", () => {
      const view = markView("a [![a](s)](h) b", { mode, room });
      setCaret(view, mode === "hidden" ? 14 : 6);
      keys(view, "Backspace");
      expect(bytes(view.state)).toBe("a [![](s)](h) b");
    });

    it("leaves ![](src) when a selection takes the whole alt text", () => {
      const view = markView("x ![alt](src) y", { mode, room });
      view.dispatch({ selection: EditorSelection.range(4, 7) });
      keys(view, "Backspace");
      expect(bytes(view.state)).toBe("x ![](src) y");
    });
  },
);

describe.each([false, true])(
  "an image with no alt text is one atom (D40), room %s",
  (room) => {
    it("draws as one piece, never as its source", () => {
      const view = markView("x ![](src) y", { room });
      expect(view.contentDOM.textContent).toBe("x  y");
      expect(view.contentDOM.querySelectorAll(".cm-image-atom")).toHaveLength(
        1,
      );
      expect(spans(view)).toEqual([]);
    });

    it("draws its source where the line shows its marks", () => {
      const view = markView("x ![](src) y", { room, mode: "show" });
      expect(view.contentDOM.textContent).toBe("x ![](src) y");
    });

    it("steps the caret over it in one press", () => {
      const view = markView("x ![](src) y", { room });
      setCaret(view, 0);
      expect(walk(view, "ArrowRight", 5)).toEqual([1, 2, 10, 11, 12]);
      expect(walk(view, "ArrowLeft", 5)).toEqual([11, 10, 2, 1, 0]);
    });

    it("is removed whole by one Backspace, and back with one undo", () => {
      const view = markView("x ![](src) y", { room });
      setCaret(view, 10);
      keys(view, "Backspace");
      expect(bytes(view.state)).toBe("x  y");
      expect(caret(view)).toBe(2);
      undoOnce(view);
      expect(bytes(view.state)).toBe("x ![](src) y");
    });

    it("is removed whole by one Delete", () => {
      const view = markView("x ![](src) y", { room });
      setCaret(view, 2);
      keys(view, "Delete");
      expect(bytes(view.state)).toBe("x  y");
    });

    it("puts the caret after it when the last alt letter goes", () => {
      const view = markView("x ![a](src) y", { room });
      setCaret(view, 11);
      keys(view, "Backspace");
      expect(bytes(view.state)).toBe("x ![](src) y");
      expect(caret(view)).toBe(10);
      keys(view, "Backspace");
      expect(bytes(view.state)).toBe("x  y");
    });

    it("takes a linked image with no alt text whole, its link too", () => {
      const view = markView("a [![](s)](h) b", { room });
      expect(view.contentDOM.textContent).toBe("a  b");
      setCaret(view, 13);
      keys(view, "Backspace");
      expect(bytes(view.state)).toBe("a  b");
      const forward = markView("a [![](s)](h) b", { room });
      setCaret(forward, 2);
      keys(forward, "Delete");
      expect(bytes(forward.state)).toBe("a  b");
    });

    it("walks a whole linked image from its first letter to nothing", () => {
      const view = markView(LINKED, { room });
      setCaret(view, 21);
      expect(presses(view, "Backspace", 5)).toEqual([
        "a [![al](src)](href) b",
        "a [![a](src)](href) b",
        "a [![](src)](href) b",
        "a  b",
        "a b",
      ]);
    });
  },
);
