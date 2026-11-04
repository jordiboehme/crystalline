/**
 * The editor draws a fence the way the reading page does: a fence the page
 * draws as a diagram is a mermaid preview in the editor, a formula is a
 * display formula, and any other closed fence is a code box. The info word
 * is read the same way on both sides (after the spaces that follow the
 * marks, a tilde fence too, with more info after the word).
 */

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Markdown } from "../components/Markdown";
import { cleanupMarkViews, markView, setCaret } from "../test/markKit";
import { ThemeProvider } from "../theme/ThemeProvider";

// Mermaid draws nothing under jsdom: a stub drawing stands in for it.
vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn(() =>
      Promise.resolve({ svg: '<svg viewBox="0 0 600 400"><g/></svg>' }),
    ),
  },
}));

afterEach(() => {
  cleanup();
  cleanupMarkViews();
});

type Face = "diagram" | "formula" | "code";

async function onThePage(source: string): Promise<Face> {
  const { container, unmount } = render(
    <MemoryRouter>
      <ThemeProvider>
        <Markdown source={source} />
      </ThemeProvider>
    </MemoryRouter>,
  );
  await waitFor(() => {
    expect(
      screen.queryByRole("status", { name: "Loading the text" }),
    ).toBeNull();
  });
  let face: Face = "code";
  if (screen.queryByLabelText("Diagram") !== null) {
    face = "diagram";
  } else {
    await waitFor(() => {
      expect(
        container.querySelector(".katex") !== null ||
          container.querySelector("pre") !== null,
      ).toBe(true);
    });
    if (container.querySelector(".katex") !== null) face = "formula";
  }
  unmount();
  return face;
}

function inTheEditor(source: string): Face {
  const view = markView(`a\n\n${source}`);
  setCaret(view, 0);
  const dom = view.contentDOM;
  if (dom.querySelector(".cm-mermaid-preview") !== null) return "diagram";
  if (dom.querySelector(".cm-math-block") !== null) return "formula";
  expect(dom.querySelector(".cm-code-header")).not.toBeNull();
  return "code";
}

describe("a fence is drawn the same in the editor and on the page", () => {
  it.each([
    ["```mermaid\ngraph TD; A-->B\n```", "diagram"],
    ["``` mermaid\ngraph TD; A-->B\n```", "diagram"],
    ["~~~mermaid\ngraph TD; A-->B\n~~~", "diagram"],
    ["```mermaid title\ngraph TD; A-->B\n```", "diagram"],
    ["```mermaid\n\n```", "code"],
    ["```math\nx^2\n```", "formula"],
    ["``` math\nx^2\n```", "formula"],
    ["~~~math\nx^2\n~~~", "formula"],
    ["```math title\nx^2\n```", "formula"],
    ["```ts\nconst a = 1;\n```", "code"],
    ["``` ts\nconst a = 1;\n```", "code"],
  ] as const)("%s", async (source, face) => {
    expect(await onThePage(source)).toBe(face);
    expect(inTheEditor(source)).toBe(face);
  });
});
