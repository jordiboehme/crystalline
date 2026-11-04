/**
 * The drag-move when the picture tools load after the view was built, as
 * they do in the app: the chunk is asked for when the view's tree first
 * holds an image. Its listeners then register after CodeMirror's own, so
 * only the capture phase keeps CodeMirror's text drop from putting a copy of
 * the reference beside the move (N1). No tools are loaded before the view
 * here, unlike the other drag tests.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

import { FakeTransfer, fireDrag } from "../../test/dragKit";
import { bytes, cleanupMarkViews, markView } from "../../test/markKit";
import { imagePreviews } from "../imagePreviews";
import { prefetchPictureTools } from "./loader";

afterEach(cleanupMarkViews);

const P = "![x](assets/p.png)";
const PAGE = `Intro ${P} words.\n\nThird.\n\nEnd.\n`;

describe("a picture dragged in a view built before the tools loaded", () => {
  it("moves once, with no copy left by the editor's own drop", async () => {
    const view = markView(PAGE, { images: imagePreviews("eng") });
    // The view asked for the chunk itself; wait until it is in and the
    // view took it.
    await prefetchPictureTools();
    await vi.waitFor(() => {
      const box = view.contentDOM.querySelector(".cm-picture");
      const transfer = new FakeTransfer();
      if (box) fireDrag(box, "dragstart", transfer);
      expect(transfer.getData("text/plain")).toBe(P);
      if (box) fireDrag(box, "dragend", transfer);
    });
    const box = view.contentDOM.querySelector(".cm-picture");
    if (!box) throw new Error("no drawn picture");
    const transfer = new FakeTransfer();
    fireDrag(box, "dragstart", transfer);
    vi.spyOn(view, "posAtCoords").mockReturnValue(PAGE.indexOf("Third"));
    const at = { x: 40, y: 80 };
    fireDrag(view.contentDOM, "dragover", transfer, at);
    fireDrag(view.contentDOM, "drop", transfer, at);
    fireDrag(box, "dragend", transfer);
    expect(bytes(view.state)).toBe(
      `Intro  words.\n\nThird.\n\n${P}\n\n\nEnd.\n`,
    );
    expect(bytes(view.state).split(P)).toHaveLength(2);
  });
});
