/**
 * The diagram's two contracts with mermaid.
 *
 * The first is that a failed render leaves nothing behind. Mermaid's default
 * on a parse failure is to append its own error graphic to `document.body` -
 * outside React's tree, where nothing this component does can unmount it, so
 * the bombs pile up at the bottom of every page until a reload. Half-typed
 * diagrams fail constantly (the editor's live preview renders on every
 * keystroke), so `suppressErrorRendering` is not an edge case, it is the
 * normal path. The second is the fallback that was always here: a diagram
 * that will not parse shows the source the author wrote.
 */

import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import mermaid from "mermaid";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ThemeProvider } from "../theme/ThemeProvider";
import MermaidDiagram from "./MermaidDiagram";

// The overlay loads the pan-and-zoom library on first open, and jsdom has no
// layout for it to work with. What this file is about is what the diagram does
// when the two buttons are pressed, so the library is a stub here and its own
// contract is pinned in `DiagramOverlay.test.tsx`.
vi.mock("@panzoom/panzoom", () => ({
  default: vi.fn(() => ({
    zoomIn: vi.fn(),
    zoomOut: vi.fn(),
    zoom: vi.fn(),
    pan: vi.fn(),
    reset: vi.fn(),
    zoomWithWheel: vi.fn(),
    destroy: vi.fn(),
  })),
}));

vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn(() => Promise.resolve({ svg: "<svg data-diagram></svg>" })),
  },
}));

/**
 * Whether the diagram's container overflows is a layout question, and jsdom
 * answers every layout question with zero. These two make the answer the
 * test's: the measurements the component reads, and a resize observer whose
 * callbacks the test can fire, so both states are pinned rather than one.
 */
const resizes: (() => void)[] = [];

class TestResizeObserver {
  constructor(callback: () => void) {
    resizes.push(callback);
  }
  observe() {}
  unobserve() {}
  disconnect() {}
}

function measuresAt(scrollWidth: number, clientWidth: number) {
  for (const [name, value] of [
    ["scrollWidth", scrollWidth],
    ["clientWidth", clientWidth],
  ] as const) {
    Object.defineProperty(HTMLElement.prototype, name, {
      configurable: true,
      value,
    });
  }
}

function resized() {
  act(() => {
    for (const fire of resizes) {
      fire();
    }
  });
}

const initialize = vi.mocked(mermaid.initialize);
const renderDiagram = vi.mocked(mermaid.render);

function draw(source: string) {
  return render(
    <ThemeProvider>
      <MermaidDiagram source={source} />
    </ThemeProvider>,
  );
}

beforeEach(() => {
  // The theme provider reads its preference out of storage on mount, so each
  // test starts from "system", which resolves to light here.
  localStorage.clear();
  resizes.length = 0;
  // Assigned rather than stubbed: the shared setup defines a do-nothing
  // observer as a writable but non-configurable global, which `stubGlobal`
  // cannot redefine.
  globalThis.ResizeObserver =
    TestResizeObserver as unknown as typeof ResizeObserver;
  // The default for every test: a container with nothing to scroll to, which
  // is what jsdom would have said on its own.
  measuresAt(0, 0);
  initialize.mockClear();
  renderDiagram.mockClear();
  renderDiagram.mockResolvedValue({
    svg: "<svg data-diagram></svg>",
    diagramType: "flowchart-v2",
  });
});

const sharedResizeObserver = globalThis.ResizeObserver;

afterEach(() => {
  globalThis.ResizeObserver = sharedResizeObserver;
  Reflect.deleteProperty(HTMLElement.prototype, "scrollWidth");
  Reflect.deleteProperty(HTMLElement.prototype, "clientWidth");
});

describe("MermaidDiagram", () => {
  it("suppresses mermaid's own error rendering", async () => {
    draw("graph TD; A-->B;");
    await waitFor(() => {
      expect(initialize).toHaveBeenCalled();
    });
    expect(initialize.mock.calls.at(-1)?.[0]).toMatchObject({
      suppressErrorRendering: true,
    });
  });

  it("draws in the app's own palette rather than mermaid's", async () => {
    // `base` is the theme that takes variables; the built-in `default` and
    // `dark` themes ignore them and a diagram would arrive in mermaid's own
    // purple, a different shade than the app's own accent.
    draw("graph TD; A-->B;");
    await waitFor(() => {
      expect(initialize).toHaveBeenCalled();
    });
    const config = initialize.mock.calls.at(-1)?.[0];
    expect(config).toMatchObject({ theme: "base" });
    expect(config?.themeVariables).toMatchObject({
      primaryColor: "#ece8f9",
      primaryTextColor: "#0f172a",
      primaryBorderColor: "#45388c",
      // Named rather than left to `base`, which would otherwise derive a
      // highlighter-yellow note and an inverted title color.
      noteBkgColor: "#f1f5f9",
      noteTextColor: "#0f172a",
      titleColor: "#0f172a",
    });
  });

  it("takes the dark palette when the app is dark", async () => {
    localStorage.setItem("fluid-theme", "dark");
    draw("graph TD; A-->B;");
    await waitFor(() => {
      expect(initialize).toHaveBeenCalled();
    });
    expect(initialize.mock.calls.at(-1)?.[0]?.themeVariables).toMatchObject({
      darkMode: true,
      primaryColor: "#2a1f5f",
      primaryTextColor: "#e2e8f0",
      primaryBorderColor: "#978bd3",
      noteBkgColor: "#1e293b",
      noteTextColor: "#e2e8f0",
      titleColor: "#e2e8f0",
    });
  });

  it("centers the diagram it rendered", async () => {
    const { container } = draw("graph TD; A-->B;");
    await waitFor(() => {
      expect(container.querySelector("svg")).not.toBeNull();
    });
    // A diagram is narrower than the column more often than not, and one
    // pinned to the left edge of a wide figure reads as a mistake.
    const wrapper = container.querySelector("svg")?.parentElement;
    expect(wrapper?.className).toContain("justify-center");
    // The ordinary diagram is exactly what it was: clamped to the column, not
    // a scroller, and not a tab stop.
    expect(wrapper?.className).toContain("[&_svg]:max-w-full");
    expect(wrapper?.className).not.toContain("overflow-x-auto");
    expect(wrapper?.getAttribute("tabindex")).toBeNull();
    expect(wrapper?.getAttribute("role")).toBeNull();
  });

  it("fits a wide diagram to the column until the reader widens it", async () => {
    // A drawing far past what any column holds, exactly the case the old
    // threshold widened on its own. The reader widens it now, or not.
    measuresAt(1600, 654);
    renderDiagram.mockResolvedValue({
      svg: '<svg viewBox="0 0 1600 400" width="100%" style="max-width: 1600px;"><g/></svg>',
      diagramType: "flowchart-v2",
    });
    const { container } = draw("graph LR; A-->B;");
    await waitFor(() => {
      expect(container.querySelector("svg")).not.toBeNull();
    });
    expect(container.querySelector("svg")?.getAttribute("width")).toBe("100%");
    expect(container.querySelector("svg")?.getAttribute("style")).toContain(
      "max-width: 1600px",
    );
    const fitted = container.querySelector("svg")?.parentElement;
    expect(fitted?.className).toContain("justify-center");
    expect(fitted?.className).toContain("[&_svg]:max-w-full");
    expect(fitted?.className).not.toContain("overflow-x-auto");
    expect(fitted?.getAttribute("tabindex")).toBeNull();

    await userEvent.click(
      screen.getByRole("button", { name: "Show at full width" }),
    );

    const svg = container.querySelector("svg");
    expect(svg?.getAttribute("width")).toBe("100%");
    expect(svg?.getAttribute("style") ?? "").toContain("min-width: 1600px");
    expect(svg?.getAttribute("style") ?? "").not.toContain("max-width");
    const wrapper = svg?.parentElement;
    expect(wrapper?.className).toContain("overflow-x-auto");
    expect(wrapper?.className).not.toContain("max-w-full");
    expect(wrapper?.className).toContain("[&_svg]:shrink-0");
    expect(wrapper?.className).toContain("overscroll-x-contain");
    await waitFor(() => {
      expect(wrapper?.getAttribute("tabindex")).toBe("0");
    });
    expect(screen.getByRole("region")).toBe(wrapper);
    expect(wrapper?.className).toContain("mask-image");

    await userEvent.click(
      screen.getByRole("button", { name: "Show at reading width" }),
    );
    expect(container.querySelector("svg")?.getAttribute("width")).toBe("100%");
    expect(container.querySelector("svg")?.parentElement?.className).toContain(
      "justify-center",
    );
  });

  it("shows the source when the diagram will not parse", async () => {
    renderDiagram.mockRejectedValue(new Error("no idea what that is"));
    draw("graph TD; A--");
    expect(await screen.findByText(/graph TD; A--/)).toBeInTheDocument();
    // Nothing of mermaid's landed outside the component's own tree.
    expect(document.body.querySelector("svg")).toBeNull();
  });

  /**
   * The two ways out of the reading column: wider, and out of the page
   * altogether. Both are the reader's own decision - nothing here happens on
   * its own - so both are buttons with names, and the names say what pressing
   * them will do rather than what state the diagram is in.
   */
  describe("the diagram's own actions", () => {
    const CLAMPED =
      '<svg viewBox="0 0 600 400" width="100%" style="max-width: 600px;"><g/></svg>';

    async function drawn(svg: string = CLAMPED) {
      renderDiagram.mockResolvedValue({ svg, diagramType: "flowchart-v2" });
      const view = draw("graph TD; A-->B;");
      await waitFor(() => {
        expect(view.container.querySelector("svg")).not.toBeNull();
      });
      return view;
    }

    it("offers both actions on a diagram it drew", async () => {
      await drawn();
      expect(
        screen.getByRole("button", { name: "Show at full width" }),
      ).toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: "Open in full window" }),
      ).toBeInTheDocument();
    });

    it("offers neither on a diagram that would not parse", async () => {
      renderDiagram.mockRejectedValue(new Error("no idea what that is"));
      draw("graph TD; A--");
      expect(await screen.findByText(/graph TD; A--/)).toBeInTheDocument();
      expect(screen.queryByRole("button")).toBeNull();
    });

    it("says nothing about scrolling when there is nothing to scroll to", async () => {
      // The ruling routes every full-width diagram through the scroll
      // container, including one narrower than the column. A container that
      // cannot scroll is a plain box: no region role, no tab stop, no name
      // promising sideways scrolling, and no mask fading an edge that is
      // simply where the drawing ends.
      const { container } = await drawn();
      await userEvent.click(
        screen.getByRole("button", { name: "Show at full width" }),
      );
      const wrapper = container.querySelector("svg")?.parentElement;
      expect(wrapper?.getAttribute("role")).toBeNull();
      expect(wrapper?.getAttribute("tabindex")).toBeNull();
      expect(wrapper?.getAttribute("aria-label")).toBeNull();
      expect(wrapper?.className).not.toContain("mask-image");
      // Still the scroll container, so the moment it does overflow it can.
      expect(wrapper?.className).toContain("overflow-x-auto");
    });

    it("becomes a named scroll region as soon as it does overflow", async () => {
      const { container } = await drawn();
      await userEvent.click(
        screen.getByRole("button", { name: "Show at full width" }),
      );
      const wrapper = container.querySelector("svg")?.parentElement;
      expect(wrapper?.getAttribute("role")).toBeNull();
      // The column narrowed under it, which is the case only a resize can
      // report: the markup did not change and neither did the measurement.
      measuresAt(936, 654);
      resized();
      expect(wrapper?.getAttribute("role")).toBe("region");
      expect(wrapper?.getAttribute("tabindex")).toBe("0");
      expect(wrapper?.getAttribute("aria-label")).toBeTruthy();
      expect(wrapper?.className).toContain("mask-image");
    });

    it("hands the diagram its own width when full width is asked for", async () => {
      // Measured as overflowing, because that is what this case is: a diagram
      // the column cannot hold, in the scroll container that carries it.
      measuresAt(936, 654);
      const { container } = await drawn();
      // Before: mermaid's scale-to-fit, clamped to the column.
      expect(container.querySelector("svg")?.getAttribute("width")).toBe(
        "100%",
      );
      await userEvent.click(
        screen.getByRole("button", { name: "Show at full width" }),
      );
      const svg = container.querySelector("svg");
      // As wide as the column allows: the viewBox fills whatever room there is,
      // and the floor keeps a column narrower than the drawing from squeezing
      // it back down.
      expect(svg?.getAttribute("width")).toBe("100%");
      expect(svg?.getAttribute("style") ?? "").toContain("min-width: 600px");
      expect(svg?.getAttribute("style") ?? "").not.toContain("max-width");
      // The same scroll region a widened diagram always uses: a diagram wider
      // than the column has to be reachable, not merely unclamped.
      const wrapper = svg?.parentElement;
      expect(wrapper?.className).toContain("overflow-x-auto");
      expect(wrapper?.className).not.toContain("max-w-full");
      expect(wrapper?.getAttribute("tabindex")).toBe("0");
    });

    it("says how to get back to the reading width once it is wide", async () => {
      await drawn();
      await userEvent.click(
        screen.getByRole("button", { name: "Show at full width" }),
      );
      const back = screen.getByRole("button", {
        name: "Show at reading width",
      });
      expect(back).toBeInTheDocument();
      await userEvent.click(back);
      expect(
        screen.getByRole("button", { name: "Show at full width" }),
      ).toBeInTheDocument();
    });

    it("opens the full window in a modal dialog and closes it again", async () => {
      await drawn();
      expect(screen.queryByRole("dialog")).toBeNull();
      await userEvent.click(
        screen.getByRole("button", { name: "Open in full window" }),
      );
      const dialog = await screen.findByRole("dialog");
      expect(dialog.getAttribute("aria-modal")).toBe("true");
      await userEvent.click(screen.getByRole("button", { name: "Close" }));
      await waitFor(() => {
        expect(screen.queryByRole("dialog")).toBeNull();
      });
      // The button that opened it is where the keyboard lands again.
      expect(document.activeElement).toBe(
        screen.getByRole("button", { name: "Open in full window" }),
      );
    });
  });
});
