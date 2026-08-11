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

import { render, screen, waitFor } from "@testing-library/react";
import mermaid from "mermaid";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ThemeProvider } from "../theme/ThemeProvider";
import MermaidDiagram from "./MermaidDiagram";

vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn(() => Promise.resolve({ svg: "<svg data-diagram></svg>" })),
  },
}));

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
  initialize.mockClear();
  renderDiagram.mockClear();
  renderDiagram.mockResolvedValue({
    svg: "<svg data-diagram></svg>",
    diagramType: "flowchart-v2",
  });
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
    // purple, beside an app that is teal everywhere else.
    draw("graph TD; A-->B;");
    await waitFor(() => {
      expect(initialize).toHaveBeenCalled();
    });
    const config = initialize.mock.calls.at(-1)?.[0];
    expect(config).toMatchObject({ theme: "base" });
    expect(config?.themeVariables).toMatchObject({
      primaryColor: "#ccfbf1",
      primaryTextColor: "#0f172a",
      primaryBorderColor: "#0f766e",
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
      primaryColor: "#134e4a",
      primaryTextColor: "#e2e8f0",
      primaryBorderColor: "#2dd4bf",
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
  });

  it("shows the source when the diagram will not parse", async () => {
    renderDiagram.mockRejectedValue(new Error("no idea what that is"));
    draw("graph TD; A--");
    expect(await screen.findByText(/graph TD; A--/)).toBeInTheDocument();
    // Nothing of mermaid's landed outside the component's own tree.
    expect(document.body.querySelector("svg")).toBeNull();
  });
});
