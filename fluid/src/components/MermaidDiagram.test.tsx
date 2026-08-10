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

  it("shows the source when the diagram will not parse", async () => {
    renderDiagram.mockRejectedValue(new Error("no idea what that is"));
    draw("graph TD; A--");
    expect(await screen.findByText(/graph TD; A--/)).toBeInTheDocument();
    // Nothing of mermaid's landed outside the component's own tree.
    expect(document.body.querySelector("svg")).toBeNull();
  });
});
