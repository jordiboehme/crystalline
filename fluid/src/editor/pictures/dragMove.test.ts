/**
 * Moving a picture by dragging it: a dragstart on the drawn picture, a
 * dragover and a drop somewhere in the same view, as a browser fires them.
 *
 * Every document is written out by hand, so a move that also leaves a copy
 * behind (CodeMirror's own text drop of the `text/plain` reference the drag
 * carries, which the fake transfer hands out as a browser does) or lands
 * one line off fails. The pointer is a stubbed `posAtCoords`. Views are
 * built through the surface with the picture layer and a finished parse
 * (`markView`), solo and as a room peer, in the hidden-marks and the
 * show-marks mode; the tools chunk is loaded first.
 */

import type { TransactionSpec } from "@codemirror/state";
import { EditorState, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import type { MarkViewOptions } from "../../test/markKit";
import {
  bytes,
  cleanupMarkViews,
  markPeers,
  markView,
  undoOnce,
  undoSteps,
} from "../../test/markKit";
import { FakeTransfer, fireDrag } from "../../test/dragKit";
import { parsedState } from "../../test/parse";
import { imagePreviews } from "../imagePreviews";
import { CANNOT_JOIN } from "../marks/deleteCheck";
import { PICTURE_TYPE } from "./dragMove";
import { prefetchPictureTools } from "./loader";
import { selectedPicture } from "./refs";

beforeAll(async () => {
  await prefetchPictureTools();
});

afterEach(cleanupMarkViews);

const AT = { x: 40, y: 80 };

type Mode = "hidden" | "show";

function build(
  doc: string,
  mode: Mode,
  room: boolean,
  options: MarkViewOptions = {},
): EditorView {
  const images = imagePreviews("eng");
  if (!room) return markView(doc, { mode, images, ...options });
  return markPeers(doc, { mode, images, ...options }).local;
}

/** The drawn element of the `index`th picture of `view`. */
function pictureBox(view: EditorView, index = 0): HTMLElement {
  const box =
    view.contentDOM.querySelectorAll<HTMLElement>(".cm-picture")[index];
  if (!box) throw new Error("no drawn picture");
  return box;
}

/** The announcements `view` makes from now on. */
function heard(view: EditorView): string[] {
  const said: string[] = [];
  const own = view.dispatch.bind(view);
  view.dispatch = ((...specs: TransactionSpec[]) => {
    const tr = view.state.update(...specs);
    for (const effect of tr.effects) {
      if (effect.is(EditorView.announce)) said.push(effect.value);
    }
    own(tr);
  }) as EditorView["dispatch"];
  return said;
}

interface Dragged {
  transfer: FakeTransfer;
  start: MouseEvent;
  over: MouseEvent;
  drop: MouseEvent;
}

/**
 * Drags the `index`th picture of `from` and drops it in `to` with the
 * pointer at `pos`, then ends the drag on the picture's element, which a
 * finished move may have taken out of the page.
 */
function dragPicture(
  from: EditorView,
  pos: number,
  options: { to?: EditorView; index?: number; between?: () => void } = {},
): Dragged {
  const to = options.to ?? from;
  const box = pictureBox(from, options.index);
  const transfer = new FakeTransfer();
  const start = fireDrag(box, "dragstart", transfer);
  options.between?.();
  vi.spyOn(to, "posAtCoords").mockReturnValue(pos);
  const over = fireDrag(to.contentDOM, "dragover", transfer, AT);
  const drop = fireDrag(to.contentDOM, "drop", transfer, AT);
  fireDrag(box, "dragend", transfer);
  return { transfer, start, over, drop };
}

/** The main selection as `[from, to]`. */
function selection(view: EditorView): [number, number] {
  const { main } = view.state.selection;
  return [main.from, main.to];
}

const P = "![x](assets/p.png#right)";
const PAGE = `Intro ${P} words.\n\nSecond.\n\nThird.\n\nEnd.\n`;
const MOVED = `Intro  words.\n\nSecond.\n\nThird.\n\n${P}\n\n\nEnd.\n`;

describe.each([
  ["hidden", false],
  ["hidden", true],
  ["show", false],
  ["show", true],
] as const)("dragging a picture (%s mode, room %s)", (mode, room) => {
  it("moves it below the block it is dropped on, as one undo step", () => {
    const view = build(PAGE, mode, room);
    const steps = undoSteps(view);
    const { transfer, start, over, drop } = dragPicture(
      view,
      PAGE.indexOf("Third"),
    );
    expect(start.defaultPrevented).toBe(false);
    expect(transfer.getData("text/plain")).toBe(P);
    expect(transfer.getData("text/uri-list")).toMatch(/^https?:\/\/.*p\.png/);
    expect(transfer.getData(PICTURE_TYPE)).not.toBe("");
    expect(transfer.effectAllowed).toBe("move");
    expect(transfer.dragImage?.tagName).toBe("IMG");
    expect(over.defaultPrevented).toBe(true);
    expect(transfer.dropEffect).toBe("move");
    expect(drop.defaultPrevented).toBe(true);
    // The removal and the insert and nothing else: no copy beside it.
    expect(bytes(view.state)).toBe(MOVED);
    const at = MOVED.indexOf(P);
    expect(selection(view)).toEqual([at, at + P.length]);
    expect(selectedPicture(parsedState(view.state))?.from).toBe(at);
    expect(undoSteps(view)).toBe(steps + 1);
    undoOnce(view);
    expect(bytes(view.state)).toBe(PAGE);
  });

  it("selects the picture when the drag starts, so an unselected one moves", () => {
    const view = build(PAGE, mode, room);
    expect(selection(view)).toEqual([0, 0]);
    const box = pictureBox(view);
    fireDrag(box, "dragstart", new FakeTransfer());
    const from = PAGE.indexOf(P);
    expect(selection(view)).toEqual([from, from + P.length]);
    fireDrag(box, "dragend", null);
  });

  it("moves a selected picture whose drag the browser began as a text drag", () => {
    // Chrome drags the selection, not the element, from a press inside the
    // selection: the target is a text node and the transfer holds the
    // drawn words as HTML and text.
    const view = build(PAGE, mode, room);
    const from = PAGE.indexOf(P);
    view.dispatch({ selection: { anchor: from, head: from + P.length } });
    pictureBox(view).querySelector("img")?.dispatchEvent(new Event("error"));
    const caption = pictureBox(view).querySelector(".cm-image-error");
    const words = caption?.firstChild;
    if (!words) throw new Error("no caption text");
    const transfer = new FakeTransfer();
    transfer.setData(
      "text/html",
      "<span>x This image is not available.</span>",
    );
    transfer.setData("text/plain", "x This image is not available.");
    fireDrag(words, "dragstart", transfer);
    expect(transfer.types).not.toContain("text/html");
    expect(transfer.getData("text/plain")).toBe(P);
    expect(transfer.getData(PICTURE_TYPE)).not.toBe("");
    vi.spyOn(view, "posAtCoords").mockReturnValue(PAGE.indexOf("Third"));
    fireDrag(view.contentDOM, "dragover", transfer, AT);
    fireDrag(view.contentDOM, "drop", transfer, AT);
    expect(bytes(view.state)).toBe(MOVED);
  });

  it("writes nothing and adds no undo step on its own landing", () => {
    const doc = `intro\n\n${P}\n\nlater\n`;
    const view = build(doc, mode, room);
    const steps = undoSteps(view);
    const { drop } = dragPicture(view, 1);
    expect(drop.defaultPrevented).toBe(true);
    expect(bytes(view.state)).toBe(doc);
    expect(undoSteps(view)).toBe(steps);
  });

  it("moves an external picture the same way", () => {
    const web = "![x](https://example.com/p.png)";
    const doc = `Intro ${web} words.\n\nThird.\n`;
    const view = build(doc, mode, room);
    const { transfer } = dragPicture(view, doc.indexOf("Third"));
    expect(transfer.getData("text/uri-list")).toBe("https://example.com/p.png");
    expect(bytes(view.state)).toBe(`Intro  words.\n\nThird.\n\n${web}\n\n`);
  });

  it("puts a picture dropped on a Reflection heading below its id line", () => {
    const doc = `Intro.\n\n${P}\n\n## H\n<!-- r:7f3a9c2e -->\n\nBody.\n`;
    const view = build(doc, mode, room, { adapter: "reflection" });
    dragPicture(view, doc.indexOf("H\n"));
    expect(bytes(view.state)).toBe(
      `Intro.\n\n## H\n<!-- r:7f3a9c2e -->\n\n${P}\n\n\nBody.\n`,
    );
  });

  it("takes a pair whose only content the picture is along (P29)", () => {
    const pic = "![x](assets/p.png)";
    const doc = `a **${pic}** b\n\nlater\n`;
    const view = build(doc, mode, room);
    dragPicture(view, doc.indexOf("later"));
    expect(bytes(view.state)).toBe(`a  b\n\nlater\n\n${pic}\n\n`);
    undoOnce(view);
    expect(bytes(view.state)).toBe(doc);
  });

  it("refuses a move whose removal would break a pair, and says why", () => {
    const doc = "**a.**![x](assets/p.png)b\n\nlater\n";
    const view = build(doc, mode, room);
    const said = heard(view);
    const steps = undoSteps(view);
    const { drop } = dragPicture(view, doc.indexOf("later"));
    expect(drop.defaultPrevented).toBe(true);
    expect(bytes(view.state)).toBe(doc);
    expect(said).toEqual([CANNOT_JOIN]);
    expect(undoSteps(view)).toBe(steps);
  });
});

describe("a picture drop the move does not take", () => {
  it("writes nothing in another Fluid view", () => {
    const one = markView(PAGE, { images: imagePreviews("eng") });
    const two = markView(PAGE, { images: imagePreviews("eng") });
    const { transfer, over, drop } = dragPicture(one, PAGE.indexOf("Third"), {
      to: two,
    });
    expect(over.defaultPrevented).toBe(true);
    expect(transfer.dropEffect).toBe("none");
    expect(drop.defaultPrevented).toBe(true);
    expect(bytes(one.state)).toBe(PAGE);
    expect(bytes(two.state)).toBe(PAGE);
  });

  it("writes nothing when no drag started here", () => {
    const view = markView(PAGE, { images: imagePreviews("eng") });
    const transfer = new FakeTransfer();
    transfer.setData(PICTURE_TYPE, "stale");
    transfer.setData("text/plain", P);
    vi.spyOn(view, "posAtCoords").mockReturnValue(PAGE.indexOf("Third"));
    const drop = fireDrag(view.contentDOM, "drop", transfer, AT);
    expect(drop.defaultPrevented).toBe(true);
    expect(bytes(view.state)).toBe(PAGE);
  });

  it("writes nothing after the drag ended", () => {
    const view = markView(PAGE, { images: imagePreviews("eng") });
    const box = pictureBox(view);
    const transfer = new FakeTransfer();
    fireDrag(box, "dragstart", transfer);
    fireDrag(box, "dragend", transfer);
    vi.spyOn(view, "posAtCoords").mockReturnValue(PAGE.indexOf("Third"));
    const drop = fireDrag(view.contentDOM, "drop", transfer, AT);
    expect(drop.defaultPrevented).toBe(true);
    expect(bytes(view.state)).toBe(PAGE);
  });

  it("starts no move from a picture inside a link", () => {
    const doc = `[${P}](https://example.com) words.\n\nThird.\n`;
    const view = markView(doc, { images: imagePreviews("eng") });
    const transfer = new FakeTransfer();
    fireDrag(pictureBox(view), "dragstart", transfer);
    expect(transfer.getData(PICTURE_TYPE)).toBe("");
  });

  it("starts no move from a picture's handle", () => {
    const view = markView(PAGE, { images: imagePreviews("eng") });
    const from = PAGE.indexOf(P);
    view.dispatch({ selection: { anchor: from, head: from + P.length } });
    const handle = pictureBox(view).querySelector(".cm-picture-handle");
    if (!handle) throw new Error("no handle");
    handle.dispatchEvent(
      new MouseEvent("mousedown", { bubbles: true, button: 0 }),
    );
    const transfer = new FakeTransfer();
    const start = fireDrag(pictureBox(view), "dragstart", transfer);
    expect(start.defaultPrevented).toBe(true);
    expect(transfer.getData(PICTURE_TYPE)).toBe("");
  });

  it("leaves a text drag to the editor", () => {
    const view = markView(PAGE, { images: imagePreviews("eng") });
    const transfer = new FakeTransfer();
    transfer.setData("text/plain", "Second.");
    vi.spyOn(view, "posAtCoords").mockReturnValue(PAGE.indexOf("End"));
    fireDrag(view.contentDOM, "drop", transfer, AT);
    // CodeMirror's own drop puts a copy of the text at the point.
    expect(bytes(view.state)).toBe(
      `Intro ${P} words.\n\nSecond.\n\nThird.\n\nSecond.End.\n`,
    );
  });

  it("writes nothing on a page that takes no writes", () => {
    const view = markView(PAGE, { images: imagePreviews("eng") });
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    const { drop } = dragPicture(view, PAGE.indexOf("Third"));
    expect(drop.defaultPrevented).toBe(true);
    expect(bytes(view.state)).toBe(PAGE);
  });
});
