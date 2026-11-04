/**
 * One layer per picture span: where the picture layer draws a reference, it
 * owns the `Image` span, and the hidden-mark layer leaves it alone (P2, P5).
 * Where it does not (Raw, no picture layer, a table cell, the edited line in
 * the show-marks mode), the hidden-mark layer keeps its own rules.
 *
 * Every view is built through the surface with a finished parse (`markView`).
 */

import { Compartment, EditorSelection, StateEffect } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";

import { copyHtmlOf } from "../copyHtml";
import type { ElementEdit } from "../elementEdit";
import { imagePreviews } from "../imagePreviews";
import { toggleFormat } from "../marks/formatting";
import { atomsOnLine, marksOnLine } from "../marks/hiddenMarks";
import { mainSelectionParked } from "../parkedSelection";
import type { MarkViewOptions } from "../../test/markKit";
import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
  setRaw,
  setShowMarks,
} from "../../test/markKit";

afterEach(cleanupMarkViews);

const DOC = "a ![x](assets/a.png) b\n\nend";

function drawn(text: string, options: MarkViewOptions = {}): EditorView {
  return markView(text, { images: imagePreviews("eng"), ...options });
}

/** The pictures' images (CodeMirror's own widget buffers are images too). */
function images(view: EditorView): HTMLImageElement[] {
  return [
    ...view.contentDOM.querySelectorAll<HTMLImageElement>(
      "img:not(.cm-widgetBuffer)",
    ),
  ];
}

function press(el: Element): MouseEvent {
  const event = new MouseEvent("mousedown", {
    bubbles: true,
    cancelable: true,
    button: 0,
  });
  el.dispatchEvent(event);
  return event;
}

describe("the picture layer and the hidden-mark layer", () => {
  it("draws one image and no source in the hidden mode", () => {
    const view = drawn(DOC);
    expect(images(view)).toHaveLength(1);
    expect(view.contentDOM.textContent).not.toContain("![");
    expect(view.contentDOM.textContent).not.toContain("](");
    expect(view.contentDOM.querySelector(".cm-picture-block")).toBeNull();
  });

  it("draws one image and no source in the show mode, caret elsewhere", () => {
    const view = drawn(DOC, { mode: "show" });
    setCaret(view, DOC.length);
    expect(images(view)).toHaveLength(1);
    expect(view.contentDOM.textContent).not.toContain("![");
  });

  it("shows the source on the edited line, with the picture below it", () => {
    const view = drawn(DOC, { mode: "show" });
    setCaret(view, 0);
    expect(view.contentDOM.textContent).toContain("![x](assets/a.png)");
    expect(images(view)).toHaveLength(1);
    const block = view.contentDOM.querySelector(".cm-picture-block");
    expect(block?.querySelector("img")).toBe(images(view)[0]);
    setCaret(view, DOC.length);
    expect(view.contentDOM.querySelector(".cm-picture-block")).toBeNull();
    expect(images(view)).toHaveLength(1);
  });

  it("keeps the picture in place when a press selects it in the show mode", () => {
    const view = drawn(DOC, { mode: "show" });
    setCaret(view, DOC.length);
    const picture = view.contentDOM.querySelector(".cm-picture");
    if (!picture) throw new Error("no picture");
    press(picture);
    expect(view.state.selection.main.from).toBe(2);
    expect(view.state.selection.main.to).toBe(20);
    expect(view.contentDOM.querySelector(".cm-picture")).toBe(picture);
    expect(picture.classList.contains("cm-picture-selected")).toBe(true);
    expect(view.contentDOM.textContent).not.toContain("![");
  });

  it("keeps the other marks of the line hidden while one picture is selected", () => {
    const view = drawn("**a** ![x](assets/a.png)\n\nend", { mode: "show" });
    setCaret(view, 29);
    view.dispatch({ selection: EditorSelection.range(6, 24) });
    expect(view.contentDOM.textContent).not.toContain("**");
  });

  it("draws an empty alt as a picture, not as Part 2's box", () => {
    const view = drawn("x ![](assets/a.png) y");
    expect(images(view)).toHaveLength(1);
    expect(view.contentDOM.querySelector(".cm-image-atom")).toBeNull();
  });

  it("gives the span back to the hidden-mark layer in Raw and out of it", () => {
    const view = drawn("x ![](assets/a.png) y");
    setRaw(view, true);
    expect(view.contentDOM.querySelector(".cm-picture")).toBeNull();
    expect(view.contentDOM.textContent).toBe("x ![](assets/a.png) y");
    setRaw(view, false);
    expect(images(view)).toHaveLength(1);
  });

  it("keeps Part 2's atom where no picture layer is installed", () => {
    const view = markView("x ![](assets/a.png) y");
    expect(view.contentDOM.querySelector(".cm-image-atom")).not.toBeNull();
    expect(view.contentDOM.querySelector(".cm-picture")).toBeNull();
  });

  it("draws a linked picture in place, its link marks hidden", () => {
    const view = drawn("a [![Shot](assets/a.png)](https://x) b");
    expect(images(view)).toHaveLength(1);
    expect(view.contentDOM.textContent).toBe("a  b");
    expect(view.contentDOM.querySelector(".cm-image-atom")).toBeNull();
  });

  it("leaves a press on a linked picture to the link", () => {
    const heard: (ElementEdit | null)[] = [];
    const view = drawn("a [![Shot](assets/a.png)](https://x) b", {
      onElementEdit: (edit) => {
        heard.push(edit);
      },
    });
    setCaret(view, 0);
    const picture = view.contentDOM.querySelector(".cm-picture");
    if (!picture) throw new Error("no picture");
    press(picture);
    expect(heard.at(-1)).toMatchObject({ kind: "link", at: 2 });
    expect(view.state.selection.main.empty).toBe(true);
  });
});

describe("the delete check around a drawn picture", () => {
  const BOLD = "**a**![x](assets/p.png)**b**\n\nend";

  /** The bytes after Backspace over `[from, to)`, with or without pictures. */
  function backspace(
    mode: "hidden" | "show",
    pictures: boolean,
    from: number,
    to: number,
  ): string {
    const view = markView(BOLD, {
      mode,
      images: pictures ? imagePreviews("eng") : [],
    });
    setCaret(view, BOLD.length);
    view.dispatch({ selection: EditorSelection.range(from, to) });
    keys(view, "Backspace");
    return bytes(view.state);
  }

  it.each(["hidden", "show"] as const)(
    "joins the two bolds when exactly the picture goes (%s mode)",
    (mode) => {
      // Never `**a****b**`: the bold marks left touching are joined.
      expect(backspace(mode, true, 5, 23)).toBe("**ab**\n\nend");
      expect(backspace(mode, false, 5, 23)).toBe("**ab**\n\nend");
    },
  );

  it("takes the picture whole with one Backspace after it or one Delete before it", () => {
    for (const [at, key] of [
      [23, "Backspace"],
      [5, "Delete"],
    ] as const) {
      const view = drawn(BOLD);
      setCaret(view, at);
      keys(view, key);
      expect(bytes(view.state)).toBe("**ab**\n\nend");
    }
  });

  it.each([
    ["hidden", 3, 12],
    ["show", 3, 12],
    ["show", 12, 25],
  ] as const)(
    "judges a range into the picture as without it (%s mode, %i to %i)",
    (mode, from, to) => {
      expect(backspace(mode, true, from, to)).toBe(
        backspace(mode, false, from, to),
      );
    },
  );

  it("takes the whole picture with a range that starts inside it, and no pair breaks", () => {
    // Hidden mode: the picture is one atom, so the range covers all of it,
    // and the bold marks left with nothing between them are joined.
    expect(backspace("hidden", true, 12, 25)).toBe("**ab**\n\nend");
  });
});

describe("the caret around a drawn picture", () => {
  /** Where ArrowRight stops from 0, then ArrowLeft back, until each end. */
  function walk(view: EditorView): { right: number[]; left: number[] } {
    const right: number[] = [];
    const left: number[] = [];
    setCaret(view, 0);
    for (let i = 0; i < 40 && caret(view) < view.state.doc.length; i += 1) {
      keys(view, "ArrowRight");
      right.push(caret(view));
    }
    for (let i = 0; i < 40 && caret(view) > 0; i += 1) {
      keys(view, "ArrowLeft");
      left.push(caret(view));
    }
    return { right, left };
  }

  it.each([
    ["a [![Shot](assets/a.png)](https://x) b", 3, 24],
    ["x **![x](assets/a.png)** y", 4, 22],
  ])("walks %s with no stop inside the picture", (doc, from, to) => {
    const view = drawn(doc);
    const { right, left } = walk(view);
    expect(right.at(-1)).toBe(doc.length);
    expect(left.at(-1)).toBe(0);
    for (const stops of [right, left]) {
      // Every press moves the caret.
      expect(new Set(stops).size).toBe(stops.length);
      expect(stops.some((pos) => pos > from && pos < to)).toBe(false);
    }
  });
});

describe("a copy around a drawn picture", () => {
  it.each([
    "a [![Shot](assets/a.png)](https://x) b",
    "x **![x](assets/a.png)** y",
  ])("writes the same HTML as without the picture layer for %s", (doc) => {
    const [withPictures, without] = [drawn(doc), markView(doc)].map((view) =>
      copyHtmlOf(view.state, [EditorSelection.range(0, doc.length)]),
    );
    expect(withPictures).toBe(without);
  });
});

describe("a press on the picture under an edited line (P25)", () => {
  it("selects it and keeps it the same block, its line still edited", () => {
    const view = drawn(DOC, { mode: "show" });
    setCaret(view, 0);
    const block = view.contentDOM.querySelector(".cm-picture-block");
    if (!block) throw new Error("no block picture");
    press(block);
    expect(view.state.selection.main.from).toBe(2);
    expect(view.state.selection.main.to).toBe(20);
    expect(view.contentDOM.querySelector(".cm-picture")).toBe(block);
    expect(block.classList.contains("cm-picture-block")).toBe(true);
    expect(block.classList.contains("cm-picture-selected")).toBe(true);
    expect(view.contentDOM.textContent).toContain("![x](assets/a.png)");
  });

  it("keeps an inline picture inline when it is pressed", () => {
    const view = drawn(DOC, { mode: "show" });
    setCaret(view, DOC.length);
    const picture = view.contentDOM.querySelector(".cm-picture");
    if (!picture) throw new Error("no picture");
    press(picture);
    expect(view.contentDOM.querySelector(".cm-picture")).toBe(picture);
    expect(picture.classList.contains("cm-picture-block")).toBe(false);
  });

  it("follows the picture through an edit elsewhere, and lets go on the next selection", () => {
    const view = drawn(DOC, { mode: "show" });
    setCaret(view, 0);
    press(view.contentDOM.querySelector(".cm-picture-block") as Element);
    view.dispatch({ changes: { from: DOC.length, insert: "!" } });
    expect(view.contentDOM.querySelector(".cm-picture-block")).not.toBeNull();
    setCaret(view, DOC.length);
    expect(view.contentDOM.querySelector(".cm-picture-block")).toBeNull();
  });
});

describe("a linked picture with no alt text", () => {
  const LINKED_EMPTY = "a [![](assets/a.png)](https://x) b";

  it("is the picture, not one atom over the whole link", () => {
    const view = drawn(LINKED_EMPTY);
    expect(images(view)).toHaveLength(1);
    expect(view.contentDOM.querySelector(".cm-image-atom")).toBeNull();
    expect(atomsOnLine(view.state, 1)).toEqual([]);
    expect(view.contentDOM.textContent).toBe("a  b");
  });

  it("stays Part 2's atom without the picture layer", () => {
    const view = markView(LINKED_EMPTY);
    expect(atomsOnLine(view.state, 1)).toEqual([{ from: 2, to: 32 }]);
  });
});

describe("what the delete check and the format commands read at a drawn picture", () => {
  const BOLD = "**a**![x](assets/p.png)**b**\n\nend";

  it("reads the picture's pair whether the marks show or not, and not for the drawing", () => {
    const view = drawn(BOLD);
    const spans = (always: boolean) =>
      marksOnLine(view.state, 1, { always }).map((m) => [m.from, m.to]);
    expect(spans(true)).toEqual([
      [0, 2],
      [3, 5],
      [5, 7],
      [8, 23],
      [23, 25],
      [26, 28],
    ]);
    expect(spans(false)).toEqual([
      [0, 2],
      [3, 5],
      [23, 25],
      [26, 28],
    ]);
  });

  it.each([
    [7, 12],
    [6, 8],
    [3, 12],
  ])(
    "keeps the picture's pair when a delete cuts into it (%i to %i)",
    (from, to) => {
      const results = [drawn(BOLD), markView(BOLD)].map((view) => {
        view.dispatch({ changes: { from, to }, userEvent: "delete.selection" });
        return bytes(view.state);
      });
      expect(results[0]).toBe("**a**![](assets/p.png)**b**\n\nend");
      expect(results[1]).toBe(results[0]);
    },
  );

  it("formats the alt words of a range that cuts into the picture", () => {
    const view = drawn(BOLD);
    view.dispatch({ selection: EditorSelection.range(3, 12) });
    toggleFormat(view, "em");
    expect(bytes(view.state)).toBe("**a**![*x*](assets/p.png)**b**\n\nend");
  });
});

describe("a picture swaps between in place and under its line", () => {
  it("when the setting flips with the caret on its line", () => {
    const view = drawn(DOC, { mode: "show" });
    setCaret(view, 0);
    expect(view.contentDOM.querySelector(".cm-picture-block")).not.toBeNull();
    setShowMarks(view, false);
    expect(view.contentDOM.querySelector(".cm-picture-block")).toBeNull();
    expect(images(view)).toHaveLength(1);
    expect(view.contentDOM.textContent).not.toContain("![");
    setShowMarks(view, true);
    expect(view.contentDOM.querySelector(".cm-picture-block")).not.toBeNull();
  });

  it("when the selection is parked behind an open cell", () => {
    const view = drawn(DOC, { mode: "show" });
    setCaret(view, 0);
    view.dispatch({
      effects: StateEffect.appendConfig.of(mainSelectionParked.of(true)),
    });
    expect(view.contentDOM.querySelector(".cm-picture-block")).toBeNull();
    expect(view.contentDOM.textContent).not.toContain("![");
  });

  it("when the picture layer comes in, the hidden-mark layer gives the span up", () => {
    const images = new Compartment();
    const view = markView("x ![](assets/a.png) y", { images: images.of([]) });
    expect(view.contentDOM.querySelector(".cm-image-atom")).not.toBeNull();
    view.dispatch({ effects: images.reconfigure(imagePreviews("eng")) });
    expect(view.contentDOM.querySelector(".cm-image-atom")).toBeNull();
    expect(view.contentDOM.querySelectorAll(".cm-picture")).toHaveLength(1);
  });
});

describe("a picture in a table", () => {
  it("keeps Part 2's marks in the main view's scan", () => {
    const TABLE = "| a |\n| - |\n| ![x](assets/a.png) |\n";
    const view = drawn(TABLE);
    const spans = (state: typeof view.state) =>
      marksOnLine(state, 3).map((m) => [m.from, m.to]);
    expect(spans(view.state)).toEqual(spans(markView(TABLE).state));
    expect(spans(view.state)).toContainEqual([14, 16]);
  });
});
