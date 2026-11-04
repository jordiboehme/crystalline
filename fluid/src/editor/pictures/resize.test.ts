/**
 * Resizing a picture by dragging a corner handle: pointer events dispatched
 * by hand, the image's box and the column's width stubbed. Every document
 * is written out by hand, and a full parse is read where the picture is
 * judged (solo, in a room, hidden and show mode).
 */

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
    const down = drag(view, "se", [150, 200]);
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
      "se",
      Array.from({ length: 10 }, (_, i) => 110 + i * 10),
      false,
    );
    expect(bytes(view.state)).toBe(before);
    expect(image?.style.width).toBe("300px");
    pointer(document, "pointerup", 200);
  });

  it("the west handles grow to the left", () => {
    const view = ready();
    drag(view, "nw", [50]);
    expect(bytes(view.state)).toBe(at("![x](assets/p.png#right,w=63%)"));
  });

  it("clamps to 10 and 100 percent", () => {
    const small = ready();
    drag(small, "se", [-300]);
    expect(bytes(small.state)).toBe(at("![x](assets/p.png#right,w=10%)"));
    cleanupMarkViews();
    const big = ready();
    drag(big, "se", [900]);
    expect(bytes(big.state)).toBe(at("![x](assets/p.png#right,w=100%)"));
  });

  it("writes nothing when the width does not change", () => {
    const view = ready();
    const steps = undoSteps(view);
    drag(view, "se", [100]);
    expect(bytes(view.state)).toBe(PAGE);
    expect(undoSteps(view)).toBe(steps);
  });

  it("Escape ends the drag, resets the width and writes nothing", () => {
    const view = ready();
    const image = box(view).querySelector("img");
    const was = image?.style.width;
    drag(view, "se", [150, 220], false);
    expect(image?.style.width).toBe("320px");
    document.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    expect(image?.style.width).toBe(was);
    pointer(document, "pointerup", 220);
    expect(bytes(view.state)).toBe(PAGE);
  });

  it("losing the pointer capture without a release writes nothing", () => {
    const view = ready();
    const image = box(view).querySelector("img");
    const was = image?.style.width;
    drag(view, "se", [150], false);
    handle(view, "se").dispatchEvent(new Event("lostpointercapture"));
    expect(image?.style.width).toBe(was);
    pointer(document, "pointerup", 150);
    expect(bytes(view.state)).toBe(PAGE);
  });

  it("Raw toggled mid-drag ends it with nothing written", () => {
    const view = ready();
    const steps = undoSteps(view);
    drag(view, "se", [150, 200], false);
    setRaw(view, true);
    pointer(document, "pointerup", 200);
    expect(bytes(view.state)).toBe(PAGE);
    expect(undoSteps(view)).toBe(steps);
  });

  it("a remote insert before the picture still writes the width on it", () => {
    if (!room) return;
    const view = build(PAGE, mode, true);
    selectPicture(view, P);
    layout(view, 200);
    drag(view, "se", [150], false);
    remoteInsert(view, 0, "Peer. ");
    pointer(document, "pointerup", 150);
    expect(bytes(view.state)).toBe(
      `Peer. ${at("![x](assets/p.png#right,w=63%)")}`,
    );
  });

  it("a press on a handle does not change the selection or start a move", () => {
    const view = ready();
    const selection = view.state.selection.main;
    const down = pointer(handle(view, "se"), "pointerdown", 100);
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
