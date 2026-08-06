/**
 * The neighborhood graph, minus the drawing.
 *
 * The renderer paints to a canvas, which jsdom has none of, so what is pinned
 * here is everything around it: that the anchor and the depth it was given are
 * what gets asked for, that the elements handed to the renderer are the ones
 * the payload describes, that a click on a node leads to that engram, and that
 * a bounded answer says it is bounded rather than passing a capped picture off
 * as the whole neighborhood. The renderer itself is stubbed, so a click is
 * fired through the same callback the real one calls.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { GraphElement, GraphNodeData } from "../graphElements";
import { isEdgeElement } from "../graphElements";
import { NeighborhoodGraph } from "./NeighborhoodGraph";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

/**
 * The renderer, stubbed: every element it was handed, named, and a way to fire
 * the same selection callback a tap on a node fires.
 */
vi.mock("./GraphCanvas", () => ({
  default: ({
    elements,
    onSelect,
  }: {
    elements: GraphElement[];
    onSelect: (domain: string, permalink: string) => void;
  }) => (
    <div data-testid="canvas">
      {elements.map((element) =>
        isEdgeElement(element) ? (
          <span key={element.data.id}>{`arrow:${element.data.label}`}</span>
        ) : (
          <button
            key={element.data.id}
            type="button"
            onClick={() => {
              const data: GraphNodeData = element.data;
              onSelect(data.domain, data.permalink);
            }}
          >
            {`drawn:${element.data.label}`}
          </button>
        ),
      )}
    </div>
  ),
}));

const apiMock = vi.mocked(api);

const ANCHOR = { domain: "eng", permalink: "alpha" };

/** Alpha is the anchor, Beta is retired and points at it. */
function graphResponse(overrides: Record<string, unknown> = {}) {
  return {
    nodes: [
      {
        id: 1,
        domain: "eng",
        permalink: "alpha",
        title: "Alpha",
        status: "stable",
        type: "engram",
      },
      {
        id: 2,
        domain: "eng",
        permalink: "notes/beta",
        title: "Beta",
        status: "deprecated",
        type: "decision",
      },
    ],
    edges: [{ from: 2, to: 1, rel_type: "links_to" }],
    truncated: false,
    ...overrides,
  };
}

/** The URL, which is where a click on a node has to land. */
function Probe() {
  const location = useLocation();
  return <span data-testid="url">{location.pathname}</span>;
}

function mount(depth = 1) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/graph"]}>
        <NeighborhoodGraph anchor={ANCHOR} depth={depth} />
        <Probe />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** Every path asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => call[0]);
}

beforeEach(() => {
  apiMock.mockReset();
  apiMock.mockImplementation(() => Promise.resolve(graphResponse()));
});

describe("the neighborhood graph", () => {
  it("asks for the anchor's neighborhood at the depth it was given", async () => {
    mount(2);

    await screen.findByTestId("canvas");
    const path = requested()[0];
    expect(path).toContain("anchor=crystalline%3A%2F%2Feng%2Falpha");
    expect(path).toContain("depth=2");
  });

  it("hands the renderer the engrams and the arrows the payload describes", async () => {
    mount();

    expect(await screen.findByText("drawn:Alpha")).toBeVisible();
    expect(screen.getByText("drawn:Beta")).toBeVisible();
    expect(screen.getByText("arrow:links_to")).toBeVisible();
  });

  it("follows a node to the engram it stands for", async () => {
    mount();

    await userEvent.click(await screen.findByText("drawn:Beta"));

    await waitFor(() => {
      expect(screen.getByTestId("url")).toHaveTextContent(
        "/d/eng/e/notes/beta",
      );
    });
  });

  it("lists the engrams as links, so the picture is not the only way in", async () => {
    mount();

    const list = await screen.findByRole("list", {
      name: /engrams in this neighborhood/i,
    });
    expect(within(list).getByRole("link", { name: /Beta/ })).toHaveAttribute(
      "href",
      "/d/eng/e/notes/beta",
    );
  });

  it("says a capped answer is capped, with what it is showing", async () => {
    apiMock.mockImplementation(() =>
      Promise.resolve(graphResponse({ truncated: true })),
    );

    mount();

    const notice = await screen.findByText(/showing the first 2 engrams/i);
    expect(notice).toBeVisible();
  });

  it("claims nothing about a cap when the answer is whole", async () => {
    mount();

    await screen.findByTestId("canvas");
    expect(screen.queryByText(/showing the first/i)).toBeNull();
  });

  it("says an engram nothing connects to has nothing to draw", async () => {
    apiMock.mockImplementation(() =>
      Promise.resolve(
        graphResponse({
          nodes: [
            {
              id: 1,
              domain: "eng",
              permalink: "alpha",
              title: "Alpha",
              status: "stable",
              type: "engram",
            },
          ],
          edges: [],
        }),
      ),
    );

    mount();

    expect(await screen.findByText(/nothing is connected/i)).toBeVisible();
    expect(screen.queryByTestId("canvas")).toBeNull();
  });

  it("says so when the neighborhood cannot be read", async () => {
    apiMock.mockImplementation(() => {
      throw new ApiProblem(500, "server error", "the index is rebuilding");
    });

    mount();

    expect(await screen.findByRole("alert")).toHaveTextContent(
      /neighborhood could not be read/i,
    );
  });

  it("says an anchor nobody wrote is a wrong address rather than a failure", async () => {
    apiMock.mockImplementation(() => {
      throw new ApiProblem(404, "not found", "no engram 'alpha' in 'eng'");
    });

    mount();

    expect(
      await screen.findByText(/no engram at crystalline:\/\/eng\/alpha/i),
    ).toBeVisible();
  });
});
