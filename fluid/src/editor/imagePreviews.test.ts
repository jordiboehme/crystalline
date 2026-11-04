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
import { afterEach, describe, expect, it, vi } from "vitest";

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

const opened: EditorView[] = [];

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

function press(el: Element, init: MouseEventInit = {}): MouseEvent {
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
