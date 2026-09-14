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

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import Panzoom from "@panzoom/panzoom";
import { useRef, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import DiagramOverlay from "./DiagramOverlay";

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

/**
 * The overlay as it is really used: a button opens it, the button is what it
 * hands the keyboard back to, and closing unmounts it. Focus restoration is
 * only observable this way round - the overlay lets go of the keyboard as it
 * goes away, not while it is still on screen.
 */
function Harness({ svg }: { svg: string }) {
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
          svg={svg}
          returnFocusTo={opener}
          onClose={() => {
            setOpen(false);
          }}
        />
      )}
    </>
  );
}

async function open(svg: string = DIAGRAM) {
  render(<Harness svg={svg} />);
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
});
