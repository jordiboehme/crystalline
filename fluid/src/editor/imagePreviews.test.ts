/**
 * The picture under the line that references it.
 *
 * An author who pastes a screenshot should see the screenshot, not a path, and
 * the widget draws AFTER the source it previews - the reference stays on screen
 * and editable, the same trade-off the mermaid and table previews make.
 *
 * What is asserted here is what the widget asks for and what it does when the
 * bytes do not arrive: a broken image is a quiet caption, never a torn layout.
 */

import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it } from "vitest";

import { parsedState } from "../test/parse";
import { imagePreviews } from "./imagePreviews";
import { baseExtensions } from "./setup";

function editor(doc: string): EditorView {
  return new EditorView({
    state: parsedState(
      EditorState.create({
        doc,
        selection: EditorSelection.cursor(0),
        extensions: [...baseExtensions(false), imagePreviews("eng")],
      }),
    ),
    parent: document.body,
  });
}

/** Every preview the buffer drew, in document order. */
function previews(view: EditorView): HTMLImageElement[] {
  return [
    ...view.dom.querySelectorAll<HTMLImageElement>(".cm-image-preview img"),
  ];
}

describe("the editor's attachment previews", () => {
  it("draws the image a line references, from the files route", () => {
    const view = editor("Before\n\n![Shot](assets/2026/08/shot.png)\n");
    const drawn = previews(view);
    expect(drawn).toHaveLength(1);
    expect(drawn[0]?.getAttribute("src")).toBe(
      "/api/v1/domains/eng/files/assets/2026/08/shot.png",
    );
    expect(drawn[0]?.getAttribute("alt")).toBe("");
    // The source is untouched: a preview is a view over the bytes.
    expect(view.state.doc.toString()).toContain(
      "![Shot](assets/2026/08/shot.png)",
    );
    view.destroy();
  });

  it("honors the fragment, and never sends it", () => {
    const view = editor("![Shot](assets/a.png#right,w=50%)\n");
    const drawn = previews(view)[0];
    expect(drawn?.getAttribute("src")).toBe(
      "/api/v1/domains/eng/files/assets/a.png",
    );
    expect(drawn?.style.float).toBe("right");
    expect(drawn?.style.width).toBe("50%");
    view.destroy();
  });

  it("previews every attachment image on a line", () => {
    const view = editor("![a](assets/a.png) ![b](assets/b.png)\n");
    expect(previews(view)).toHaveLength(2);
    view.destroy();
  });

  it("previews nothing for an external image, a document or a code fence", () => {
    const view = editor(
      [
        "![out](https://example.com/a.png)",
        "",
        "[deck](assets/deck.pdf)",
        "",
        "```",
        "![a](assets/a.png)",
        "```",
        "",
      ].join("\n"),
    );
    expect(previews(view)).toHaveLength(0);
    view.destroy();
  });

  it("says an image is unavailable rather than leaving a torn box", () => {
    const view = editor("![Shot](assets/a.png)\n");
    const drawn = previews(view)[0];
    expect(drawn).toBeDefined();
    drawn?.dispatchEvent(new Event("error"));
    const box = view.dom.querySelector(".cm-image-preview");
    expect(box?.querySelector("img")).toBeNull();
    expect(box?.textContent).toBe("This image is not available.");
    view.destroy();
  });
});
