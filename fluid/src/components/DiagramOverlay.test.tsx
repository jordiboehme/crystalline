/**
 * The full-window view of one diagram, and the contracts a layer over the
 * whole page owes the person who opened it.
 *
 * Two of them are the reason this is a test file rather than a screenshot. The
 * first is the way out: Escape closes, and the keyboard lands back on the
 * button that opened the overlay rather than at the top of the document, which
 * is the difference between a detour and a dead end. The second is that the
 * pan-and-zoom library is loaded on first open and never before, so this file
 * mocks it and pins that it is handed the element the SVG actually sits in.
 *
 * jsdom has no layout, so nothing here asserts where anything ended up on
 * screen: fit-and-centre is checked in a real browser. What is checked is that
 * every control is wired to the instance method it names.
 */

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import Panzoom from "@panzoom/panzoom";
import { useRef, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import DiagramOverlay from "./DiagramOverlay";
import type { OverlayContent } from "./DiagramOverlay";

const instance = {
  zoomIn: vi.fn(),
  zoomOut: vi.fn(),
  zoom: vi.fn(),
  pan: vi.fn(),
  reset: vi.fn(),
  zoomWithWheel: vi.fn(),
  destroy: vi.fn(),
};

vi.mock("@panzoom/panzoom", () => ({
  default: vi.fn(() => instance),
}));

const panzoom = vi.mocked(Panzoom);

const DIAGRAM =
  '<svg viewBox="0 0 600 400" width="100%" style="max-width: 600px;"><g id="drawing"/></svg>';

/** The fence the fixture was drawn from, which is what a download hands back. */
const SOURCE = "graph TD; A-->B;";

/**
 * A diagram with a link in it. Mermaid writes one wherever a node carries a
 * `click` directive or a `href`, so the hosted markup can bring its own focus
 * stops into a layer that promises to be modal.
 */
const LINKED =
  '<svg viewBox="0 0 600 400"><g id="drawing"><a href="https://example.invalid"><text>Go</text></a></g></svg>';

/** The options panzoom was constructed with, for the hooks handed to it. */
function panzoomOptions(): { handleStartEvent?: (event: Event) => void } {
  return panzoom.mock.calls[0]?.[1] ?? {};
}

/** A press, delivered the way panzoom delivers one: through its own start hook. */
function press(x: number, y: number) {
  const hook = panzoomOptions().handleStartEvent;
  expect(hook).toBeTypeOf("function");
  hook?.(new MouseEvent("pointerdown", { clientX: x, clientY: y }));
}

/**
 * The overlay as it is really used: a button opens it, the button is what it
 * hands the keyboard back to, and closing unmounts it. Focus restoration is
 * only observable this way round - the overlay lets go of the keyboard as it
 * goes away, not while it is still on screen.
 */
function Harness({ content }: { content: OverlayContent }) {
  const opener = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        type="button"
        ref={opener}
        onClick={() => {
          setOpen(true);
        }}
      >
        Open in full window
      </button>
      {open && (
        <DiagramOverlay
          content={content}
          returnFocusTo={opener}
          onClose={() => {
            setOpen(false);
          }}
        />
      )}
    </>
  );
}

/**
 * The stage has no size of its own in jsdom, and a fit computed from nothing
 * is skipped rather than guessed at, so a test about fitting has to hand the
 * layer the room it would have in a browser.
 */
function sizeStage(width: number, height: number) {
  Object.defineProperty(
    screen.getByTestId("diagram-overlay-stage"),
    "getBoundingClientRect",
    {
      value: () => ({ width, height }) as DOMRect,
      configurable: true,
    },
  );
}

async function open(
  content: OverlayContent = { kind: "diagram", svg: DIAGRAM, source: SOURCE },
) {
  render(<Harness content={content} />);
  await userEvent.click(
    screen.getByRole("button", { name: "Open in full window" }),
  );
  await screen.findByRole("dialog");
  // The library arrives through a dynamic import, so the instance exists a
  // microtask after the dialog does.
  await waitFor(() => {
    expect(panzoom).toHaveBeenCalled();
  });
}

beforeEach(() => {
  panzoom.mockClear();
  for (const method of Object.values(instance)) {
    method.mockClear();
  }
});

afterEach(() => {
  Reflect.deleteProperty(document, "fullscreenEnabled");
});

describe("DiagramOverlay", () => {
  it("is a modal dialog with a name, and it holds the diagram", async () => {
    await open();
    const dialog = screen.getByRole("dialog");
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(dialog.getAttribute("aria-label")).toBeTruthy();
    expect(dialog.querySelector("#drawing")).not.toBeNull();
  });

  it("holds an image, fits it once it has loaded and names it by its alt", async () => {
    await open({
      kind: "image",
      src: "/api/v1/files/eng/assets/map.png",
      alt: "The map",
      filename: "map.png",
    });
    const dialog = screen.getByRole("dialog");
    const image = dialog.querySelector("img");
    expect(image).not.toBeNull();
    expect(image?.getAttribute("src")).toBe("/api/v1/files/eng/assets/map.png");
    expect(image?.getAttribute("alt")).toBe("The map");
    expect(image?.getAttribute("draggable")).toBe("false");
    // jsdom loads nothing, so the natural size is 0 by 0 and the fit is
    // skipped rather than computed from nothing: the instance is created and
    // never asked to zoom.
    expect(panzoom).toHaveBeenCalledTimes(1);
    expect(instance.zoom).not.toHaveBeenCalled();
    // Once the browser reports a size, the fit runs.
    sizeStage(1000, 800);
    Object.defineProperty(image, "naturalWidth", {
      value: 800,
      configurable: true,
    });
    Object.defineProperty(image, "naturalHeight", {
      value: 600,
      configurable: true,
    });
    fireEvent.load(image as HTMLImageElement);
    expect(instance.zoom).toHaveBeenCalled();
  });

  it("puts the keyboard on the way out", async () => {
    await open();
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Close" }),
    );
  });

  it("hands the library the element the diagram sits in, once", async () => {
    await open();
    expect(panzoom).toHaveBeenCalledTimes(1);
    const hosted = panzoom.mock.calls[0]?.[0];
    expect(hosted).toBeInstanceOf(HTMLElement);
    expect((hosted as HTMLElement).querySelector("#drawing")).not.toBeNull();
  });

  it("closes on Escape and gives the keyboard back to the button that opened it", async () => {
    await open();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Open in full window" }),
    );
    expect(instance.destroy).toHaveBeenCalled();
  });

  it("closes when the empty space around the diagram is clicked", async () => {
    await open();
    // The stage is the backdrop: a click that lands on it rather than on the
    // drawing inside it is the lightbox gesture for "I am done looking".
    await userEvent.click(screen.getByTestId("diagram-overlay-stage"));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
  });

  it("stays open when the drawing itself is clicked", async () => {
    await open();
    const drawing = screen.getByRole("dialog").querySelector("#drawing");
    expect(drawing).not.toBeNull();
    await userEvent.click(drawing as Element);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  /**
   * The press has to be recorded through panzoom's own start hook, not through
   * a React `onPointerDown`. With `canvas: true` panzoom listens on the stage
   * and its default start handler calls `stopPropagation`, so the event never
   * reaches the listener React keeps on the portal's container: a React
   * handler would never run and the guard below would quietly never apply.
   */
  it("records the press through the hook it hands panzoom", async () => {
    await open();
    expect(panzoomOptions().handleStartEvent).toBeTypeOf("function");
    const event = new MouseEvent("pointerdown", { clientX: 10, clientY: 10 });
    const prevented = vi.spyOn(event, "preventDefault");
    const stopped = vi.spyOn(event, "stopPropagation");
    panzoomOptions().handleStartEvent?.(event);
    // The default behaviour panzoom would have done itself, kept.
    expect(prevented).toHaveBeenCalled();
    expect(stopped).toHaveBeenCalled();
  });

  it("stays open on the click that ends a drag", async () => {
    await open();
    press(120, 90);
    fireEvent.click(screen.getByTestId("diagram-overlay-stage"), {
      clientX: 260,
      clientY: 150,
    });
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("closes on a click that stayed where it was pressed", async () => {
    await open();
    press(120, 90);
    fireEvent.click(screen.getByTestId("diagram-overlay-stage"), {
      clientX: 122,
      clientY: 91,
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
  });

  it("keeps the keys working when focus is on a link inside the diagram", async () => {
    await open({ kind: "diagram", svg: LINKED, source: SOURCE });
    const link = screen.getByRole("link");
    link.focus();
    expect(document.activeElement).toBe(link);
    await userEvent.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
  });

  it("wraps Tab around the layer, the diagram's own links included", async () => {
    await open({ kind: "diagram", svg: LINKED, source: SOURCE });
    const link = screen.getByRole("link");
    // The link is the last stop in document order, after the bar's buttons.
    link.focus();
    await userEvent.tab();
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Copy source" }),
    );
    await userEvent.tab({ shift: true });
    expect(document.activeElement).toBe(link);
  });

  it("pulls focus back in when it is somewhere that is not a stop", async () => {
    await open();
    // A click on the bar's empty half lands on the dialog itself, which is
    // focusable for exactly this reason.
    const dialog = screen.getByRole("dialog");
    dialog.focus();
    expect(document.activeElement).toBe(dialog);
    await userEvent.tab();
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Copy source" }),
    );
    // And the keys still reach the handler from there.
    dialog.focus();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
  });

  it("wires each control to the instance method it names", async () => {
    await open();
    await userEvent.click(screen.getByRole("button", { name: "Zoom in" }));
    expect(instance.zoomIn).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Zoom out" }));
    expect(instance.zoomOut).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Reset" }));
    expect(instance.reset).toHaveBeenCalled();
  });

  it("answers the keys a zoomed picture is driven with", async () => {
    await open();
    await userEvent.keyboard("+");
    expect(instance.zoomIn).toHaveBeenCalledTimes(1);
    await userEvent.keyboard("-");
    expect(instance.zoomOut).toHaveBeenCalledTimes(1);
    await userEvent.keyboard("0");
    expect(instance.reset).toHaveBeenCalled();
    await userEvent.keyboard("{ArrowRight}");
    expect(instance.pan).toHaveBeenCalledWith(
      expect.any(Number),
      expect.any(Number),
      { relative: true },
    );
  });

  it("lets the wheel zoom in here, where no page is scrolling behind it", async () => {
    await open();
    const stage = screen.getByTestId("diagram-overlay-stage");
    stage.dispatchEvent(
      new WheelEvent("wheel", { deltaY: -120, cancelable: true }),
    );
    expect(instance.zoomWithWheel).toHaveBeenCalled();
  });

  it("leaves the page where it was while it is open", async () => {
    await open();
    expect(document.body.style.overflow).toBe("hidden");
    await userEvent.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(document.body.style.overflow).not.toBe("hidden");
  });

  it("offers true fullscreen only where the browser has it", async () => {
    Object.defineProperty(document, "fullscreenEnabled", {
      value: false,
      configurable: true,
    });
    await open();
    expect(
      screen.queryByRole("button", { name: "Enter fullscreen" }),
    ).toBeNull();
  });

  it("offers true fullscreen where the browser has it", async () => {
    Object.defineProperty(document, "fullscreenEnabled", {
      value: true,
      configurable: true,
    });
    await open();
    expect(
      screen.getByRole("button", { name: "Enter fullscreen" }),
    ).toBeInTheDocument();
  });
  /**
   * What the full window is for once the reader has seen the picture: keeping
   * it. A diagram leaves as the fence that made it and as a drawing anything
   * opens; an image leaves as the file it already is.
   */
  describe("downloads", () => {
    let saved: { name: string; blob: Blob | null; href: string }[];
    // Held here rather than read back off `URL`: a method taken off an object
    // is a method without its own `this`, and the lint that says so is right
    // often enough to be worth one variable.
    let madeUrl: ReturnType<typeof vi.fn<(blob: Blob) => string>>;
    beforeEach(() => {
      saved = [];
      // jsdom implements neither half of the object-URL pair, so both are
      // defined here rather than spied on.
      madeUrl = vi.fn<(blob: Blob) => string>(() => "blob:fake");
      Object.defineProperty(URL, "createObjectURL", {
        configurable: true,
        value: madeUrl,
      });
      Object.defineProperty(URL, "revokeObjectURL", {
        configurable: true,
        value: vi.fn(),
      });
      vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(
        function (this: HTMLAnchorElement) {
          saved.push({
            name: this.download,
            blob: madeUrl.mock.calls.at(-1)?.[0] ?? null,
            href: this.href,
          });
        },
      );
    });
    afterEach(() => {
      vi.restoreAllMocks();
    });

    it("hands over a diagram's source as .mmd, named by document and hash", async () => {
      await open({
        kind: "diagram",
        svg: DIAGRAM,
        source: "graph TD; A-->B;",
        name: "alpha",
      });
      await userEvent.click(
        screen.getByRole("button", { name: "Download source (.mmd)" }),
      );
      expect(saved).toHaveLength(1);
      expect(saved[0]?.name).toMatch(/^alpha-diagram-[0-9a-f]{4}\.mmd$/);
      expect(saved[0]?.blob?.type).toBe("text/plain");
      expect(await saved[0]?.blob?.text()).toBe("graph TD; A-->B;");
    });

    it("hands over the drawing as a standalone SVG", async () => {
      await open({
        kind: "diagram",
        svg: DIAGRAM,
        source: "graph TD; A-->B;",
        name: "alpha",
      });
      await userEvent.click(
        screen.getByRole("button", { name: "Download SVG" }),
      );
      const text = await saved[0]?.blob?.text();
      expect(saved[0]?.name).toMatch(/^alpha-diagram-[0-9a-f]{4}\.svg$/);
      expect(saved[0]?.blob?.type).toBe("image/svg+xml");
      expect(
        text?.startsWith('<?xml version="1.0" encoding="UTF-8"?>\n<svg'),
      ).toBe(true);
      expect(text).toContain('xmlns="http://www.w3.org/2000/svg"');
      // The sized drawing, not the markup that arrived: the fixture's viewBox
      // is 600 by 400 and the host's pass writes that on in pixels, so a file
      // that still said `width="100%"` would be the wrong one.
      expect(text).toContain('width="600px"');
    });

    it("puts a diagram's source on the clipboard and says so", async () => {
      const writeText = vi.fn(() => Promise.resolve());
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: { writeText },
      });
      await open({
        kind: "diagram",
        svg: DIAGRAM,
        source: "graph TD; A-->B;",
        name: "alpha",
      });
      await userEvent.click(
        screen.getByRole("button", { name: "Copy source" }),
      );
      expect(writeText).toHaveBeenCalledWith("graph TD; A-->B;");
      expect(await screen.findByRole("status")).toHaveTextContent("Copied");
    });

    it("says when the clipboard refused", async () => {
      // Absent rather than rejecting: on an insecure context there is no
      // `clipboard` to read `writeText` off, and the read itself throws.
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: undefined,
      });
      await open({ kind: "diagram", svg: DIAGRAM, source: "graph TD; A-->B;" });
      await userEvent.click(
        screen.getByRole("button", { name: "Copy source" }),
      );
      expect(await screen.findByRole("status")).toHaveTextContent(
        "Copy refused",
      );
    });

    it("names a nameless diagram plainly", async () => {
      await open({ kind: "diagram", svg: DIAGRAM, source: "graph TD; A-->B;" });
      await userEvent.click(
        screen.getByRole("button", { name: "Download source (.mmd)" }),
      );
      expect(saved[0]?.name).toMatch(/^diagram-[0-9a-f]{4}\.mmd$/);
    });

    /**
     * The image download is the browser's own, so what is asserted is the
     * anchor rather than a click on it: `userEvent.click` dispatches the
     * pointer and mouse events rather than calling the element's `click`, so
     * the spy above never sees one - and jsdom answers the default action of
     * a real click with "not implemented: navigation" rather than a download.
     * The attributes ARE the contract here: the name the file lands under and
     * the address it comes from, with nothing of the picture in this page's
     * memory.
     */
    it("hands over an image by its own name from its own address", async () => {
      await open({
        kind: "image",
        src: "/api/v1/files/eng/assets/map.png",
        alt: "The map",
        filename: "map.png",
      });
      expect(
        screen.queryByRole("button", { name: /Download source|Download SVG/ }),
      ).toBeNull();
      const link = screen.getByRole("link", { name: "Download image" });
      expect(link.getAttribute("download")).toBe("map.png");
      expect(link.getAttribute("href")).toBe(
        "/api/v1/files/eng/assets/map.png",
      );
      expect(madeUrl).not.toHaveBeenCalled();
    });
  });
});
