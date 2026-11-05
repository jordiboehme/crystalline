/**
 * The picture in place of the reference that names it.
 *
 * An author who pastes a screenshot should see the screenshot, not a path:
 * the reference is drawn as the picture itself, from the same files route the
 * reading page reads it from, with its placement. The source comes back in
 * Raw, and on the line the person edits when the setting shows marks there.
 *
 * What is asserted here is what the widget asks for, what it does when the
 * bytes do not arrive, and that drawing it never writes: a picture is a view
 * over the bytes.
 */

import type { Extension } from "@codemirror/state";
import { Compartment, EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { beforeAll, afterEach, describe, expect, it, vi } from "vitest";

import type { AssetResolver } from "../api/files";
import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
} from "../test/markKit";
import { parsedState } from "../test/parse";
import { attachmentUploads } from "./attachments";
import { assetPath } from "./imageFormat";
import { imagePreviews } from "./imagePreviews";
import { baseExtensions, lineSeparatorFor } from "./setup";
import { prefetchPictureTools } from "./pictures/loader";

const opened: EditorView[] = [];

beforeAll(async () => {
  await prefetchPictureTools();
});

afterEach(() => {
  for (const view of opened) view.destroy();
  opened.length = 0;
  cleanupMarkViews();
});

/** A buffer with the picture layer and no mark layer, caret at 0. */
function editor(
  doc: string,
  extra: Extension[] = [],
  images: Extension = imagePreviews("eng"),
): EditorView {
  const view = new EditorView({
    state: parsedState(
      EditorState.create({
        doc,
        selection: EditorSelection.cursor(0),
        extensions: [
          ...lineSeparatorFor(doc),
          ...baseExtensions(false),
          images,
          ...extra,
        ],
      }),
    ),
    parent: document.body,
  });
  opened.push(view);
  return view;
}

/** Every image the buffer drew, in document order. */
function previews(view: EditorView): HTMLImageElement[] {
  return [
    ...view.dom.querySelectorAll<HTMLImageElement>(".cm-image-preview img"),
  ];
}

function pictures(view: EditorView): HTMLElement[] {
  return [...view.contentDOM.querySelectorAll<HTMLElement>(".cm-picture")];
}

function nth(view: EditorView, index = 0): HTMLElement {
  const el = pictures(view)[index];
  if (!el) throw new Error(`no picture ${String(index)}`);
  return el;
}

/** A press and its release on `el`, as a click is; answers the press. */
function press(el: Element, init: MouseEventInit = {}): MouseEvent {
  const event = down(el, init);
  el.dispatchEvent(
    new MouseEvent("mouseup", { bubbles: true, button: 0, ...init }),
  );
  return event;
}

/** A press on `el` alone, not released yet. */
function down(el: Element, init: MouseEventInit = {}): MouseEvent {
  const event = new MouseEvent("mousedown", {
    bubbles: true,
    cancelable: true,
    button: 0,
    ...init,
  });
  el.dispatchEvent(event);
  return event;
}

/** A resolver that refuses anything under `private/`, with its own reason. */
const GUARDED: AssetResolver = {
  resolve: assetPath,
  url: (path) => `/files/${path}`,
  refused: (target) =>
    target.startsWith("private/") ? "This file is not shared here." : null,
};

describe("the editor's attachment previews", () => {
  it("draws the image a reference names, from the files route", () => {
    const view = editor("Before\n\n![Shot](assets/2026/08/shot.png)\n");
    const drawn = previews(view);
    expect(drawn).toHaveLength(1);
    expect(drawn[0]?.getAttribute("src")).toBe(
      "/api/v1/domains/eng/files/assets/2026/08/shot.png",
    );
    // The source is no longer on screen, so the picture says it itself.
    expect(drawn[0]?.getAttribute("alt")).toBe("Shot");
    // The source is untouched: a picture is a view over the bytes.
    expect(view.state.doc.toString()).toContain(
      "![Shot](assets/2026/08/shot.png)",
    );
  });

  it("honors the fragment, and never sends it", () => {
    const view = editor("![Shot](assets/a.png#right,w=50%)\n");
    const drawn = previews(view)[0];
    expect(drawn?.getAttribute("src")).toBe(
      "/api/v1/domains/eng/files/assets/a.png",
    );
    expect(drawn?.style.float).toBe("right");
    expect(drawn?.style.width).toBe("50%");
  });

  it("previews a reference written with a leading ./", () => {
    const view = editor("![Shot](./assets/a.png)\n");
    expect(previews(view)[0]?.getAttribute("src")).toBe(
      "/api/v1/domains/eng/files/assets/a.png",
    );
  });

  it("previews every attachment image on a line", () => {
    const view = editor("![a](assets/a.png) ![b](assets/b.png)\n");
    expect(previews(view)).toHaveLength(2);
  });

  it("draws nothing for a document link or a code fence", () => {
    const view = editor(
      [
        "[deck](assets/deck.pdf)",
        "",
        "```",
        "![a](assets/a.png)",
        "```",
        "",
      ].join("\n"),
    );
    expect(pictures(view)).toHaveLength(0);
  });

  it("says an image is unavailable rather than leaving a torn box", () => {
    const view = editor("![Shot](assets/a.png)\n");
    const drawn = previews(view)[0];
    expect(drawn).toBeDefined();
    drawn?.dispatchEvent(new Event("error"));
    const box = view.dom.querySelector(".cm-image-preview");
    expect(box?.querySelector("img")).toBeNull();
    expect(box?.textContent).toBe("ShotThis image is not available.");
    expect(box?.querySelector(".cm-image-error")?.textContent).toBe(
      "This image is not available.",
    );
  });
});

/**
 * A left or right picture's line holds its float (P44): the text continues
 * below the picture in the editor, so nothing reflows when that line leaves
 * CodeMirror's viewport. The reading page keeps wrapping (`imageStyle` is
 * untouched); only the editor's line is contained.
 */
/**
 * The handles are placed from a measure, never from inside CodeMirror's
 * update (M4), and the observer that keeps them on the image's corners is
 * let go with the image (M5).
 */
describe("the handles' layout", () => {
  const SIZES: [string, number][] = [
    ["offsetWidth", 120],
    ["offsetHeight", 60],
    ["offsetLeft", 10],
    ["offsetTop", 4],
  ];

  /** Layout reads, each with the view's update state when it was made. */
  function watchLayout(view: () => EditorView | null): number[] {
    const states: number[] = [];
    for (const [name, value] of SIZES) {
      vi.spyOn(
        HTMLElement.prototype,
        name as "offsetWidth",
        "get",
      ).mockImplementation(() => {
        const now = view();
        states.push(
          now === null
            ? -1
            : (now as unknown as { updateState: number }).updateState,
        );
        return value;
      });
    }
    return states;
  }

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("reads the image's box in a measure, after the update", async () => {
    let current: EditorView | null = null;
    const states = watchLayout(() => current);
    const view = editor("a ![x](assets/a.png) b");
    current = view;
    view.dispatch({ selection: EditorSelection.range(2, 20) });
    // Nothing read while the selection was drawn.
    expect(states.filter((state) => state === 2)).toEqual([]);
    await new Promise((resolve) => requestAnimationFrame(resolve));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(states.filter((state) => state === 2)).toEqual([]);
    const se = nth(view).querySelector<HTMLElement>(
      ".cm-picture-handle[data-corner=se]",
    );
    expect(se?.style.left).toBe("130px");
    expect(se?.style.top).toBe("64px");
  });

  it("lets the image's observer go when the picture is deselected or gone", () => {
    // The images' observers only: CodeMirror keeps one of its own.
    const observed: Element[] = [];
    const watching = new WeakMap<ResizeObserver, Element>();
    let disconnected = 0;
    vi.spyOn(ResizeObserver.prototype, "observe").mockImplementation(function (
      this: ResizeObserver,
      target: Element,
    ) {
      if (target instanceof HTMLImageElement) {
        observed.push(target);
        watching.set(this, target);
      }
    });
    vi.spyOn(ResizeObserver.prototype, "disconnect").mockImplementation(
      function (this: ResizeObserver) {
        if (watching.has(this)) disconnected += 1;
      },
    );
    const view = editor("a ![x](assets/a.png) b\n");
    view.dispatch({ selection: EditorSelection.range(2, 20) });
    expect(observed).toHaveLength(1);
    view.dispatch({ selection: EditorSelection.cursor(0) });
    expect(disconnected).toBe(1);
    view.dispatch({ selection: EditorSelection.range(2, 20) });
    expect(observed).toHaveLength(2);
    view.dispatch({ changes: { from: 2, to: 20 } });
    expect(disconnected).toBe(2);
  });
});

describe("a floated picture's line", () => {
  const DOC = [
    "A ![L](assets/l.png#left) here.",
    "",
    "![C](assets/c.png)",
    "",
    "B ![R](assets/r.png#right,w=25%) b",
    "",
    "![Out](https://example.com/o.png#right)",
    "",
  ].join("\n");

  const contained = (view: EditorView) =>
    [...view.contentDOM.querySelectorAll<HTMLElement>(".cm-line")]
      .filter((line) => line.classList.contains("cm-picture-float-line"))
      .map((line) => line.textContent);

  it("is contained for an attachment set left or right, and only then", () => {
    const view = editor(DOC);
    expect(contained(view)).toEqual(["A  here.", "B  b"]);
    const line = view.contentDOM.querySelector<HTMLElement>(
      ".cm-picture-float-line",
    );
    expect(line && getComputedStyle(line).display).toBe("flow-root");
    // The image keeps the float the reading page gives it.
    expect(
      line?.querySelector<HTMLImageElement>(".cm-picture img")?.style.float,
    ).toBe("left");
  });

  it("follows the placement as it is set and taken away", () => {
    const view = editor("A ![L](assets/l.png) here.\n");
    expect(contained(view)).toEqual([]);
    view.dispatch({ changes: { from: 19, insert: "#right" } });
    expect(contained(view)).toEqual(["A  here."]);
    view.dispatch({ changes: { from: 19, to: 25 } });
    expect(contained(view)).toEqual([]);
  });

  it("holds a float drawn under the edited line too", () => {
    const view = markView("A ![L](assets/l.png#left) here.\n\nend", {
      mode: "show",
      images: imagePreviews("eng"),
    });
    const block =
      view.contentDOM.querySelector<HTMLElement>(".cm-picture-block");
    expect(block).not.toBeNull();
    expect(block && getComputedStyle(block).display).toBe("flow-root");
  });
});

describe("a picture in place of its reference", () => {
  const FOUR = [
    "A ![Shot](assets/a.png#left) here.",
    "",
    "![Out](https://example.com/o.png)",
    "",
    "![Hidden](private/x.png)",
    "",
    "![Deck](assets/deck.pdf)",
    "",
  ].join("\n");

  it("draws an attachment, an external, a refused and an unavailable one, and no source", () => {
    const view = editor(FOUR, [], imagePreviews(GUARDED));
    const drawn = pictures(view);
    expect(drawn.map((el) => el.dataset["kind"])).toEqual([
      "attachment",
      "external",
      "refused",
      "unavailable",
    ]);
    expect(view.contentDOM.textContent).not.toContain("![");
    const [attachment, external, refused, unavailable] = drawn;
    const shot = attachment?.querySelector("img");
    expect(shot?.getAttribute("alt")).toBe("Shot");
    expect(shot?.getAttribute("src")).toBe("/files/assets/a.png");
    expect(shot?.style.float).toBe("left");
    const out = external?.querySelector("img");
    expect(out?.getAttribute("src")).toBe("https://example.com/o.png");
    expect(out?.getAttribute("alt")).toBe("Out");
    expect(out?.style.display).toBe("block");
    expect(out?.style.marginLeft).toBe("auto");
    expect(refused?.querySelector("img")).toBeNull();
    expect(refused?.textContent).toContain("Hidden");
    expect(refused?.querySelector(".cm-image-error")?.textContent).toBe(
      "This file is not shared here.",
    );
    expect(unavailable?.querySelector("img")).toBeNull();
    expect(unavailable?.textContent).toContain("Deck");
    expect(unavailable?.querySelector(".cm-image-error")?.textContent).toBe(
      "This image is not available.",
    );
  });

  it("starts a drag on the wrapper, never on the image", () => {
    const view = editor("![Shot](assets/a.png)\n");
    const wrapper = nth(view);
    expect(wrapper.getAttribute("draggable")).toBe("true");
    expect(wrapper.querySelector("img")?.getAttribute("draggable")).toBe(
      "false",
    );
  });

  it('draws an empty alt as alt=""', () => {
    const view = editor("x ![](assets/a.png) y\n");
    expect(previews(view)[0]?.getAttribute("alt")).toBe("");
    expect(view.contentDOM.textContent).toBe("x  y");
  });

  it("shows the source in Raw, and no picture", () => {
    const preview = new Compartment();
    const view = editor(
      "![Shot](assets/a.png)\n",
      [],
      preview.of(imagePreviews("eng")),
    );
    expect(pictures(view)).toHaveLength(1);
    view.dispatch({ effects: preview.reconfigure([]) });
    expect(pictures(view)).toHaveLength(0);
    expect(view.contentDOM.textContent).toBe("![Shot](assets/a.png)");
  });

  const FIXTURES = [
    "![a](./assets/a.png)",
    '![a](assets/a.png "Q3")',
    "![a](assets/a(1).png)",
    "![a](assets/a%20b.png)",
    "![a](assets/a.png#right,foo)",
    "![a [b] c](assets/a.png)",
  ];

  describe.each(["\n", "\r\n"])(
    "opening a hand-written reference (%j)",
    (eol) => {
      it.each(FIXTURES)("writes nothing for %s", (reference) => {
        const doc = `Intro${eol}${eol}${reference}${eol}${eol}End${eol}`;
        let changes = 0;
        const view = editor(doc, [
          EditorView.updateListener.of((update) => {
            if (update.docChanged) changes += 1;
          }),
        ]);
        expect(pictures(view)).toHaveLength(1);
        expect(changes).toBe(0);
        expect(bytes(view.state)).toBe(doc);
        const full = markView(doc, {
          images: imagePreviews("eng"),
          lineSeparator: eol === "\r\n" ? "\r\n" : "\n",
        });
        expect(full.contentDOM.querySelectorAll(".cm-picture")).toHaveLength(1);
        expect(bytes(full.state)).toBe(doc);
      });
    },
  );

  it("is one step for the caret", () => {
    const view = markView("a ![x](assets/a.png) b", {
      images: imagePreviews("eng"),
    });
    setCaret(view, 2);
    keys(view, "ArrowRight");
    expect(caret(view)).toBe(20);
    keys(view, "ArrowLeft");
    expect(caret(view)).toBe(2);
  });

  it("is one step for the caret with no mark layer too", () => {
    const view = editor("a ![x](assets/a.png) b");
    view.dispatch({ selection: EditorSelection.cursor(2) });
    keys(view, "ArrowRight");
    expect(caret(view)).toBe(20);
  });

  it("is selected whole by a press", () => {
    const view = editor("a ![x](assets/a.png) b");
    const event = press(nth(view));
    expect(view.state.selection.main.anchor).toBe(2);
    expect(view.state.selection.main.head).toBe(20);
    // The press keeps its default, so a drag can still start from it.
    expect(event.defaultPrevented).toBe(false);
  });

  it("is selected when the press is released, never while it is held", () => {
    // Chrome starts no drag from a press whose selection changes while it
    // is held (the smoke's experiments), so the press selects on release.
    const view = editor("a ![x](assets/a.png) b");
    const event = down(nth(view));
    expect(event.defaultPrevented).toBe(false);
    expect(view.state.selection.main.empty).toBe(true);
    document.body.dispatchEvent(
      new MouseEvent("mouseup", { bubbles: true, button: 0 }),
    );
    expect(view.state.selection.main.from).toBe(2);
    expect(view.state.selection.main.to).toBe(20);
  });

  it("selects the picture where an edit above moved it while the press was held (P39)", () => {
    const view = editor("a ![x](assets/a.png) b");
    down(nth(view));
    view.dispatch({ changes: { from: 0, insert: "zz" } });
    document.body.dispatchEvent(
      new MouseEvent("mouseup", { bubbles: true, button: 0 }),
    );
    expect(view.state.selection.main.from).toBe(4);
    expect(view.state.selection.main.to).toBe(22);
  });

  it("selects nothing on release when the picture itself changed meanwhile", () => {
    const view = editor("a ![x](assets/a.png) b");
    down(nth(view));
    view.dispatch({ changes: { from: 5, insert: "y" } });
    document.body.dispatchEvent(
      new MouseEvent("mouseup", { bubbles: true, button: 0 }),
    );
    expect(view.state.selection.main.empty).toBe(true);
  });

  it("selects nothing on release when the press became a drag", () => {
    const view = editor("a ![x](assets/a.png) b");
    const picture = nth(view);
    down(picture);
    picture.dispatchEvent(
      new Event("dragstart", { bubbles: true, cancelable: true }),
    );
    picture.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    expect(view.state.selection.main.empty).toBe(true);
  });

  it("is selected whole by a press on its image", () => {
    const view = editor("a ![x](assets/a.png) b");
    press(nth(view).querySelector("img") as HTMLElement);
    expect(view.state.selection.main.from).toBe(2);
    expect(view.state.selection.main.to).toBe(20);
  });

  it("picks the right one of two pictures that touch", () => {
    const view = editor("![a](assets/a.png)![b](assets/b.png)");
    press(nth(view, 1));
    expect(view.state.selection.main.from).toBe(18);
    expect(view.state.selection.main.to).toBe(36);
  });

  it("leaves a press with a modifier or another button alone", () => {
    const view = editor("a ![x](assets/a.png) b");
    press(nth(view), { shiftKey: true });
    press(nth(view), { button: 2 });
    expect(view.state.selection.main.empty).toBe(true);
  });

  it("draws its frame when selected, and keeps its image", () => {
    const view = editor("a ![x](assets/a.png) b");
    const image = nth(view).querySelector("img");
    expect(image).toBeTruthy();
    view.dispatch({ selection: EditorSelection.range(2, 20) });
    const selected = nth(view);
    expect(selected.classList.contains("cm-picture-selected")).toBe(true);
    expect(selected.querySelector("img")).toBe(image);
    const handles = [
      ...selected.querySelectorAll<HTMLElement>(".cm-picture-handle"),
    ];
    expect(handles.map((h) => h.dataset["corner"])).toEqual([
      "nw",
      "ne",
      "sw",
      "se",
    ]);
    for (const handle of handles) {
      expect(handle.getAttribute("aria-hidden")).toBe("true");
      expect(handle.getAttribute("draggable")).toBe("false");
    }
    view.dispatch({ selection: EditorSelection.cursor(0) });
    const plain = nth(view);
    expect(plain.classList.contains("cm-picture-selected")).toBe(false);
    expect(plain.querySelector(".cm-picture-handle")).toBeNull();
    expect(plain.querySelector("img")).toBe(image);
  });

  it("draws a float's handles on its free edge only, where they follow the pointer", () => {
    const corners = (doc: string, from: number, to: number) => {
      const view = editor(doc);
      view.dispatch({ selection: EditorSelection.range(from, to) });
      return [
        ...nth(view).querySelectorAll<HTMLElement>(".cm-picture-handle"),
      ].map((h) => h.dataset["corner"]);
    };
    // A right float's east edge is pinned to the column: its handles are west.
    expect(corners("a ![x](assets/a.png#right) b", 2, 26)).toEqual([
      "nw",
      "sw",
    ]);
    expect(corners("a ![x](assets/a.png#left,w=40%) b", 2, 31)).toEqual([
      "ne",
      "se",
    ]);
    expect(corners("a ![x](assets/a.png#w=40%) b", 2, 26)).toEqual([
      "nw",
      "ne",
      "sw",
      "se",
    ]);
  });

  it("frames an external picture without handles", () => {
    const view = editor("a ![x](https://e.org/x.png) b");
    view.dispatch({ selection: EditorSelection.range(2, 27) });
    const selected = nth(view);
    expect(selected.classList.contains("cm-picture-selected")).toBe(true);
    expect(selected.querySelector(".cm-picture-handle")).toBeNull();
  });

  it("cancels a drag that starts on a handle, and only that one", () => {
    const view = editor("a ![x](assets/a.png) b");
    view.dispatch({ selection: EditorSelection.range(2, 20) });
    const picture = nth(view);
    const handle = picture.querySelector(".cm-picture-handle") as HTMLElement;
    press(handle);
    // A browser names the draggable wrapper as the drag's target.
    const fromHandle = new Event("dragstart", {
      bubbles: true,
      cancelable: true,
    });
    picture.dispatchEvent(fromHandle);
    expect(fromHandle.defaultPrevented).toBe(true);
    press(picture.querySelector("img") as HTMLElement);
    const fromPicture = new Event("dragstart", {
      bubbles: true,
      cancelable: true,
    });
    picture.dispatchEvent(fromPicture);
    expect(fromPicture.defaultPrevented).toBe(false);
  });

  it("hands a file dropped onto it to the upload", () => {
    const onFiles = vi.fn();
    const view = editor("a ![x](assets/a.png) b", [attachmentUploads(onFiles)]);
    const file = new File(["x"], "shot.png", { type: "image/png" });
    const drop = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(drop, "dataTransfer", {
      value: { files: [file], types: ["Files"], getData: () => "" },
    });
    nth(view).dispatchEvent(drop);
    expect(onFiles).toHaveBeenCalledWith([file]);
    expect(drop.defaultPrevented).toBe(true);
  });

  it("leaves a picture inside a table to the grid", () => {
    const view = editor("| a |\n| - |\n| ![x](assets/a.png) |\n");
    expect(pictures(view)).toHaveLength(0);
  });
});
