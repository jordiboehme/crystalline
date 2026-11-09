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

  it("draws a light copy for the printer on a dark screen, when idle", async () => {
    localStorage.setItem("fluid-theme", "dark");
    renderDiagram.mockImplementation((id: string) =>
      Promise.resolve({
        svg: id.endsWith("-paper")
          ? "<svg data-paper></svg>"
          : "<svg data-diagram></svg>",
        diagramType: "flowchart-v2",
      }),
    );
    const { container } = draw("graph TD; A-->B;");
    await waitFor(() => {
      expect(
        container.querySelector("[data-print] svg[data-paper]"),
      ).not.toBeNull();
    });
    const paper = renderDiagram.mock.calls.find(([id]) =>
      id.endsWith("-paper"),
    );
    // Light by its own directive, whatever the library was initialised with.
    expect(paper?.[1]).toContain('"darkMode":false');
    expect(paper?.[1]).toContain('"primaryColor":"#ece8f9"');
    // Hidden on screen; the screen's dark drawing stays off paper instead.
    expect(container.querySelector("[data-print]")).toHaveAttribute("hidden");
    expect(
      container.querySelector("svg[data-diagram]")?.closest(".print\\:hidden"),
    ).not.toBeNull();
  });

  it("tells the print sheet each drawing's own size, the paper copy's too", async () => {
    // Paper prints a diagram at its viewBox width, less to fit the column or
    // a page, never more (`index.css`); the wrapper carries the numbers.
    localStorage.setItem("fluid-theme", "dark");
    renderDiagram.mockImplementation((id: string) =>
      Promise.resolve({
        svg: id.endsWith("-paper")
          ? '<svg data-paper viewBox="0 0 230 410"><g/></svg>'
          : '<svg data-diagram viewBox="0 0 228 400"><g/></svg>',
        diagramType: "er",
      }),
    );
    const { container } = draw("erDiagram\n A ||--o{ B : has");
    await waitFor(() => {
      expect(container.querySelector("svg[data-paper]")).not.toBeNull();
    });
    const screenBox = container.querySelector("svg[data-diagram]")
      ?.parentElement as HTMLElement;
    expect(screenBox).toHaveAttribute("data-drawing");
    expect(screenBox.style.getPropertyValue("--diagram-width")).toBe("228");
    expect(screenBox.style.getPropertyValue("--diagram-height")).toBe("400");
    const paperBox = container.querySelector("svg[data-paper]")
      ?.parentElement as HTMLElement;
    expect(paperBox).toHaveAttribute("data-drawing");
    expect(paperBox.style.getPropertyValue("--diagram-width")).toBe("230");
    expect(paperBox.style.getPropertyValue("--diagram-height")).toBe("410");

    await userEvent.click(
      screen.getByRole("button", { name: "Show at full width" }),
    );
    const wideBox = container.querySelector("svg[data-diagram]")
      ?.parentElement as HTMLElement;
    expect(wideBox.className).toContain("overflow-x-auto");
    expect(wideBox).toHaveAttribute("data-drawing");
    expect(wideBox.style.getPropertyValue("--diagram-width")).toBe("228");
  });

  it("draws no printer copy on a light screen", async () => {
    const { container } = draw("graph TD; A-->B;");
    await waitFor(() => {
      expect(container.querySelector("svg[data-diagram]")).not.toBeNull();
    });
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(renderDiagram.mock.calls.some(([id]) => id.endsWith("-paper"))).toBe(
      false,
    );
    expect(container.querySelector("[data-print]")).toBeNull();
  });

  /**
   * The paper copies of a whole page share one queue. Safari and Playwright's
   * WebKit have no idle callback, and there all copies once started in one
   * burst 200 ms after the drawings, so the page painted nothing and a click
   * landed a second late. Each paper render here is held until the test lets
   * it go, so what is in flight at any moment can be counted.
   */
  describe("the paper copies of a dark page", () => {
    /** Each held paper render's way out, by the first line of its source. */
    const held = new Map<string, (fail: boolean) => void>();

    function papers(): string[] {
      return renderDiagram.mock.calls
        .filter(([id]) => id.endsWith("-paper"))
        .map(([, text]) => text.split("\n")[0] ?? "");
    }

    function release(source: string, fail = false) {
      const go = held.get(source);
      if (go === undefined) {
        throw new Error(`no paper render held for ${source}`);
      }
      held.delete(source);
      go(fail);
    }

    /** A macrotask of its own, after any 0 ms timer queued before it. */
    async function aTask(ms = 0) {
      await act(() => new Promise((resolve) => setTimeout(resolve, ms)));
    }

    /**
     * Release one render and count the paper renders once its microtasks have
     * run, before any timer can: the count is taken inside `act`, whose own
     * flush afterwards may run a task.
     */
    async function releaseAndCount(source: string): Promise<number> {
      let count = -1;
      await act(async () => {
        release(source);
        for (let i = 0; i < 20; i += 1) {
          await Promise.resolve();
        }
        count = papers().length;
      });
      return count;
    }

    beforeEach(() => {
      localStorage.setItem("fluid-theme", "dark");
      held.clear();
      renderDiagram.mockImplementation((id: string, text: string) => {
        if (!id.endsWith("-paper")) {
          return Promise.resolve({
            svg: "<svg data-diagram></svg>",
            diagramType: "flowchart-v2",
          });
        }
        return new Promise((resolve, reject) => {
          held.set(text.split("\n")[0] ?? "", (fail) => {
            if (fail) {
              reject(new Error("paper failed"));
            } else {
              resolve({
                svg: "<svg data-paper></svg>",
                diagramType: "flowchart-v2",
              });
            }
          });
        });
      });
    });

    afterEach(async () => {
      // The queue is shared by the whole module: a render left held would
      // keep it busy for the next test.
      for (const source of [...held.keys()]) {
        release(source);
      }
      await aTask(250);
    });

    function Page({ show }: { show: string[] }) {
      return (
        <ThemeProvider>
          {show.map((source) => (
            <MermaidDiagram key={source} source={source} />
          ))}
        </ThemeProvider>
      );
    }

    const AB = "graph TD; A-->B;";
    const CD = "graph TD; C-->D;";
    const EF = "graph TD; E-->F;";

    it("draws them one at a time, a task apart, where there is no idle callback", async () => {
      // jsdom has none, as Safari and Playwright's WebKit: pinned, not assumed.
      expect(typeof window.requestIdleCallback).toBe("undefined");
      render(<Page show={[AB, CD, EF]} />);
      await waitFor(() => {
        expect(papers()).toHaveLength(1);
      });
      // Long past the 200 ms the old burst waited for: still the one.
      await aTask(400);
      expect(papers()).toEqual([AB]);

      // Settled, but the next render waits for a task of its own.
      expect(await releaseAndCount(AB)).toBe(1);
      await aTask(5);
      expect(papers()).toEqual([AB, CD]);

      release(CD);
      await aTask(5);
      expect(papers()).toEqual([AB, CD, EF]);
      release(EF);
      await aTask(5);
      expect(
        document.querySelectorAll("[data-print] svg[data-paper]"),
      ).toHaveLength(3);
    });

    it("goes on past a paper render that fails", async () => {
      render(<Page show={[AB, CD]} />);
      await waitFor(() => {
        expect(papers()).toHaveLength(1);
      });
      release(AB, true);
      await aTask(5);
      expect(papers()).toEqual([AB, CD]);
    });

    it("never draws the copy of a diagram that left while it waited", async () => {
      const view = render(<Page show={[AB, CD, EF]} />);
      await waitFor(() => {
        expect(papers()).toHaveLength(1);
      });
      view.rerender(<Page show={[AB, EF]} />);
      release(AB);
      await aTask(5);
      expect(papers()).toEqual([AB, EF]);
      release(EF);
      await aTask(250);
      expect(papers()).toEqual([AB, EF]);
    });

    it("leaves nothing behind when the page goes mid-queue", async () => {
      const view = render(<Page show={[AB, CD, EF]} />);
      await waitFor(() => {
        expect(papers()).toHaveLength(1);
      });
      view.unmount();
      // The render that was running settles after its diagram is gone.
      release(AB);
      await aTask(250);
      expect(papers()).toEqual([AB]);
      expect(document.querySelector("[data-print]")).toBeNull();
      // And the queue is free: the next page's copy is drawn.
      const GH = "graph TD; G-->H;";
      render(<Page show={[GH]} />);
      await waitFor(() => {
        expect(papers()).toEqual([AB, GH]);
      });
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
