/**
 * Resizing a picture by dragging a corner handle: pointer events dispatched
 * by hand, the image's box and the column's width stubbed. Every document
 * is written out by hand, and a full parse is read where the picture is
 * judged (solo, in a room, hidden and show mode).
 */

import { EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import {
  bytes,
  cleanupMarkViews,
  markPeers,
  markView,
  remoteInsert,
  setRaw,
  undoOnce,
  undoSteps,
} from "../../test/markKit";
import { imagePreviews } from "../imagePreviews";
import { prefetchPictureTools } from "./loader";
import { pictureAt } from "./refs";

beforeAll(async () => {
  await prefetchPictureTools();
});

afterEach(cleanupMarkViews);

type Mode = "hidden" | "show";

const COLUMN = 400;

function build(doc: string, mode: Mode, room: boolean): EditorView {
  const images = imagePreviews("eng");
  if (!room) return markView(doc, { mode, images });
  return markPeers(doc, { mode, images }).local;
}

function selectPicture(view: EditorView, ref: string): void {
  const from = view.state.doc.toString().indexOf(ref);
  view.dispatch({ selection: { anchor: from, head: from + ref.length } });
}

function box(view: EditorView): HTMLElement {
  const found = view.contentDOM.querySelector<HTMLElement>(".cm-picture");
  if (!found) throw new Error("no drawn picture");
  return found;
}

function handle(view: EditorView, corner: string): HTMLElement {
  const found = box(view).querySelector<HTMLElement>(
    `.cm-picture-handle[data-corner=${corner}]`,
  );
  if (!found) throw new Error("no handle");
  return found;
}

/** A stubbed layout: the column is COLUMN wide, the image `width` wide. */
function layout(view: EditorView, width: number): void {
  Object.defineProperty(view.contentDOM, "clientWidth", {
    configurable: true,
    value: COLUMN,
  });
  const image = box(view).querySelector("img");
  if (!image) throw new Error("no image");
  image.getBoundingClientRect = () =>
    ({
      width,
      height: 50,
      left: 0,
      top: 0,
      right: width,
      bottom: 50,
    }) as DOMRect;
}

function pointer(
  target: EventTarget,
  type: string,
  x: number,
  extra: Record<string, unknown> = {},
): Event {
  const event = new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    clientX: x,
    button: 0,
  });
  Object.defineProperty(event, "pointerId", { value: 1 });
  for (const [key, value] of Object.entries(extra)) {
    Object.defineProperty(event, key, { value });
  }
  target.dispatchEvent(event);
  return event;
}

/** Press on `corner` at x=100, move to each of `xs`, release at the last. */
function drag(
  view: EditorView,
  corner: string,
  xs: number[],
  release = true,
): Event {
  const down = pointer(handle(view, corner), "pointerdown", 100);
  for (const x of xs) pointer(document, "pointermove", x);
  if (release) pointer(document, "pointerup", xs[xs.length - 1] ?? 100);
  return down;
}

const P = "![x](assets/p.png#right,w=50%)";
const PAGE = `Intro ${P} words.\n\nEnd.\n`;
const at = (rest: string): string => PAGE.replace(P, rest);

describe.each([
  ["hidden", false],
  ["hidden", true],
  ["show", false],
  ["show", true],
] as const)("resizing a picture (%s mode, room %s)", (mode, room) => {
  function ready(): EditorView {
    const view = build(PAGE, mode, room);
    selectPicture(view, P);
    layout(view, 200);
    return view;
  }

  it("writes the new width on release, nothing else, as one undo step", () => {
    const view = ready();
    const steps = undoSteps(view);
    const selection = view.state.selection.main;
    const down = drag(view, "sw", [50, 0]);
    expect(down.defaultPrevented).toBe(true);
    expect(bytes(view.state)).toBe(at("![x](assets/p.png#right,w=75%)"));
    expect(undoSteps(view)).toBe(steps + 1);
    const ref = pictureAt(view.state, view.state.selection.main.from + 1);
    expect(ref?.format?.width).toBe("75%");
    expect(selection.empty).toBe(false);
    undoOnce(view);
    expect(bytes(view.state)).toBe(PAGE);
  });

  it("writes nothing while the pointer moves, only the image's inline width", () => {
    const view = ready();
    const before = bytes(view.state);
    const image = box(view).querySelector("img");
    drag(
      view,
      "sw",
      Array.from({ length: 10 }, (_, i) => 90 - i * 10),
      false,
    );
    expect(bytes(view.state)).toBe(before);
    expect(image?.style.width).toBe("300px");
    pointer(document, "pointerup", 0);
  });

  it("the west handles grow to the left", () => {
    const view = ready();
    drag(view, "nw", [50]);
    expect(bytes(view.state)).toBe(at("![x](assets/p.png#right,w=63%)"));
  });

  it("clamps to 10 and 100 percent", () => {
    const small = ready();
    drag(small, "sw", [500]);
    expect(bytes(small.state)).toBe(at("![x](assets/p.png#right,w=10%)"));
    cleanupMarkViews();
    const big = ready();
    drag(big, "sw", [-700]);
    expect(bytes(big.state)).toBe(at("![x](assets/p.png#right,w=100%)"));
  });

  it("writes nothing when the width does not change", () => {
    const view = ready();
    const steps = undoSteps(view);
    drag(view, "sw", [100]);
    expect(bytes(view.state)).toBe(PAGE);
    expect(undoSteps(view)).toBe(steps);
  });

  it("Escape ends the drag, resets the width and writes nothing", () => {
    const view = ready();
    const image = box(view).querySelector("img");
    const was = image?.style.width;
    drag(view, "sw", [50, -20], false);
    expect(image?.style.width).toBe("320px");
    document.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    expect(image?.style.width).toBe(was);
    expect(box(view).dataset["resizing"]).toBeUndefined();
    pointer(document, "pointerup", -20);
    expect(bytes(view.state)).toBe(PAGE);
  });

  it("losing the pointer capture without a release writes nothing", () => {
    const view = ready();
    const image = box(view).querySelector("img");
    const was = image?.style.width;
    drag(view, "sw", [50], false);
    handle(view, "sw").dispatchEvent(new Event("lostpointercapture"));
    expect(image?.style.width).toBe(was);
    pointer(document, "pointerup", 50);
    expect(bytes(view.state)).toBe(PAGE);
  });

  it("Raw toggled mid-drag ends it with nothing written", () => {
    const view = ready();
    const steps = undoSteps(view);
    drag(view, "sw", [50, 0], false);
    setRaw(view, true);
    pointer(document, "pointerup", 0);
    expect(bytes(view.state)).toBe(PAGE);
    expect(undoSteps(view)).toBe(steps);
  });

  it("a remote insert before the picture still writes the width on it", () => {
    if (!room) return;
    const view = build(PAGE, mode, true);
    selectPicture(view, P);
    layout(view, 200);
    drag(view, "sw", [50], false);
    remoteInsert(view, 0, "Peer. ");
    pointer(document, "pointerup", 50);
    expect(bytes(view.state)).toBe(
      `Peer. ${at("![x](assets/p.png#right,w=63%)")}`,
    );
  });

  it("a press on a handle does not change the selection or start a move", () => {
    const view = ready();
    const selection = view.state.selection.main;
    const down = pointer(handle(view, "sw"), "pointerdown", 100);
    expect(down.defaultPrevented).toBe(true);
    expect(view.state.selection.main.eq(selection)).toBe(true);
    pointer(document, "pointerup", 100);
    expect(view.state.selection.main.eq(selection)).toBe(true);
  });

  it("an external picture has no handles", () => {
    const web = "![x](https://example.com/p.png)";
    const view = build(`Intro ${web} words.\n`, mode, room);
    selectPicture(view, web);
    expect(box(view).querySelector(".cm-picture-handle")).toBeNull();
  });
});

/** The element the image's percent width is a share of, as the code picks it. */
function container(view: EditorView): HTMLElement {
  const picture = box(view);
  const found = picture.classList.contains("cm-picture-block")
    ? picture
    : picture.closest<HTMLElement>(".cm-line");
  if (!found) throw new Error("no containing block");
  return found;
}

/** Gives the containing block a width and a horizontal padding of `pad` each side. */
function pad(view: EditorView, width: number, each: number): void {
  const element = container(view);
  Object.defineProperty(element, "clientWidth", {
    configurable: true,
    value: width,
  });
  element.style.paddingLeft = `${String(each)}px`;
  element.style.paddingRight = `${String(each)}px`;
}

describe("the percent base is the image's containing block (P41)", () => {
  const cases: [string, string, Mode][] = [
    ["a list item", "- item ![x](assets/p.png#right,w=50%) end\n", "hidden"],
    ["a quote", "> quoted ![x](assets/p.png#right,w=50%) end\n", "hidden"],
    ["a plain line", "Intro ![x](assets/p.png#right,w=50%) end\n", "hidden"],
  ];
  it.each(cases)(
    "%s: the written share is of its padded width",
    (_n, doc, mode) => {
      const view = build(doc, mode, false);
      selectPicture(view, "![x](assets/p.png#right,w=50%)");
      layout(view, 200);
      pad(view, 400, 15);
      // The base is 370: 200 + 100 pixels is 81 percent, not the 75 of 400.
      drag(view, "sw", [0]);
      expect(bytes(view.state)).toBe(doc.replace("w=50%", "w=81%"));
    },
  );

  it("a block picture is a share of its own box less its padding", () => {
    const doc = "Intro.\n\n![x](assets/p.png#right,w=50%)\n\nEnd.\n";
    const view = build(doc, "show", false);
    const ref = "![x](assets/p.png#right,w=50%)";
    view.dispatch({ selection: { anchor: doc.indexOf(ref) + ref.length } });
    expect(box(view).classList.contains("cm-picture-block")).toBe(true);
    selectPicture(view, ref);
    layout(view, 200);
    pad(view, 400, 12);
    drag(view, "sw", [0]);
    expect(bytes(view.state)).toBe(doc.replace("w=50%", "w=80%"));
  });
});

describe("a drag follows the picture and its peers", () => {
  it("a centered picture grows on both sides, so the handle follows the pointer", () => {
    const doc = "Intro ![x](assets/p.png#w=50%) words.\n";
    const view = build(doc, "hidden", false);
    selectPicture(view, "![x](assets/p.png#w=50%)");
    layout(view, 200);
    const image = box(view).querySelector("img");
    drag(view, "se", [150], false);
    // 50 pixels of pointer is 50 pixels at each edge.
    expect(image?.style.width).toBe("300px");
    pointer(document, "pointerup", 150);
    expect(bytes(view.state)).toBe(doc.replace("w=50%", "w=75%"));
  });

  it("an unchanged share writes nothing even when the pixels moved", () => {
    const view = build(PAGE, "hidden", false);
    selectPicture(view, P);
    layout(view, 200);
    const steps = undoSteps(view);
    // 201 of 400 pixels still rounds to 50.
    drag(view, "sw", [99]);
    expect(bytes(view.state)).toBe(PAGE);
    expect(undoSteps(view)).toBe(steps);
  });

  it("setState ends the drag and puts the inline width back", () => {
    const view = build(PAGE, "hidden", false);
    selectPicture(view, P);
    layout(view, 200);
    const image = box(view).querySelector("img");
    const was = image?.style.width;
    drag(view, "sw", [50, -20], false);
    expect(image?.style.width).toBe("320px");
    view.setState(EditorState.create({ doc: PAGE, extensions: [] }));
    expect(image?.style.width).toBe(was);
  });

  it("a deleted picture ends the drag with nothing written", () => {
    const view = build(PAGE, "hidden", false);
    selectPicture(view, P);
    layout(view, 200);
    drag(view, "sw", [50], false);
    const from = PAGE.indexOf(P);
    view.dispatch({ changes: { from, to: from + P.length } });
    const after = bytes(view.state);
    pointer(document, "pointerup", 50);
    expect(bytes(view.state)).toBe(after);
  });

  it("a picture a peer turned into an external one is not resized", () => {
    const view = build(PAGE, "hidden", false);
    selectPicture(view, P);
    layout(view, 200);
    drag(view, "sw", [50], false);
    const from = PAGE.indexOf("assets/p.png#right,w=50%");
    view.dispatch({
      changes: {
        from,
        to: from + "assets/p.png#right,w=50%".length,
        insert: "https://example.com/p.png",
      },
    });
    const after = bytes(view.state);
    pointer(document, "pointerup", 50);
    expect(bytes(view.state)).toBe(after);
  });

  it("a drawn picture taken out of the page by an update ends the drag with nothing written", () => {
    const view = build(PAGE, "hidden", false);
    selectPicture(view, P);
    layout(view, 200);
    drag(view, "sw", [50], false);
    // What a redraw into a new element does to the one the drag holds.
    box(view).remove();
    view.dispatch({ changes: { from: 0, insert: "Peer. " } });
    const after = bytes(view.state);
    pointer(document, "pointerup", 50);
    expect(bytes(view.state)).toBe(after);
  });

  it("a peer's alt edit keeps the drag going", () => {
    const view = build(PAGE, "hidden", false);
    selectPicture(view, P);
    layout(view, 200);
    drag(view, "sw", [50], false);
    const at = PAGE.indexOf("![x]") + 2;
    view.dispatch({ changes: { from: at, insert: "long" } });
    pointer(document, "pointerup", 50);
    expect(bytes(view.state)).toBe(
      at_(PAGE, "![longx](assets/p.png#right,w=63%)"),
    );
  });

  it("a peer's change of alignment mid-drag keeps the drag and the write follows it", () => {
    const view = build(PAGE, "hidden", true);
    selectPicture(view, P);
    layout(view, 200);
    const image = box(view).querySelector("img");
    drag(view, "sw", [50], false);
    const at = PAGE.indexOf("#right") + 1;
    view.dispatch({ changes: { from: at, to: at + 5, insert: "left" } });
    expect(box(view).querySelector("img")).toBe(image);
    expect(image?.style.width).toBe("250px");
    pointer(document, "pointerup", 50);
    expect(bytes(view.state)).toBe(at_(PAGE, "![x](assets/p.png#left,w=63%)"));
  });
});

function at_(page: string, replacement: string): string {
  return page.replace(P, replacement);
}
