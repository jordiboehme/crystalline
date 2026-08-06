/**
 * The full-screen graph, whose whole state is its URL.
 *
 * The anchor and the depth live in the search params and nowhere else, so a
 * picture is a link somebody can send and the back button walks the hops rather
 * than the clicks. What is pinned here is that: the URL is what gets asked for,
 * a depth change is written back and refires, an address arriving without an
 * anchor is met with an instruction rather than an error, and a node leads to
 * its engram.
 *
 * The renderer is stubbed, as it is in the component's own test: jsdom has no
 * canvas, and what this screen owns is the wiring around one.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import type { GraphElement, GraphNodeData } from "../graphElements";
import { isEdgeElement } from "../graphElements";
import GraphView from "./GraphView";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

vi.mock("../components/GraphCanvas", () => ({
  default: ({
    elements,
    onSelect,
  }: {
    elements: GraphElement[];
    onSelect: (domain: string, permalink: string) => void;
  }) => (
    <div data-testid="canvas">
      {elements.map((element) =>
        isEdgeElement(element) ? null : (
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

function graphResponse() {
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
        status: "stable",
        type: "engram",
      },
    ],
    edges: [{ from: 1, to: 2, rel_type: "links_to" }],
    truncated: false,
  };
}

/** The URL, which is this screen's only state. */
function Probe() {
  const location = useLocation();
  return (
    <span data-testid="url">{`${location.pathname}${location.search}`}</span>
  );
}

function url(): string {
  return screen.getByTestId("url").textContent ?? "";
}

/** Every path asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => call[0]);
}

function mount(entry: string) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[entry]}>
        <Routes>
          <Route path="/graph" element={<GraphView />} />
        </Routes>
        <Probe />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const ALPHA = "/graph?anchor=crystalline%3A%2F%2Feng%2Falpha";

beforeEach(() => {
  apiMock.mockReset();
  apiMock.mockImplementation(() => Promise.resolve(graphResponse()));
});

describe("the graph screen", () => {
  it("draws the neighborhood the URL's anchor names", async () => {
    mount(ALPHA);

    await screen.findByTestId("canvas");
    expect(requested()[0]).toContain("anchor=crystalline%3A%2F%2Feng%2Falpha");
    // One hop unless the URL says otherwise: a second hop is a deliberate act.
    expect(requested()[0]).toContain("depth=1");
  });

  it("opens at the depth a shared link carries", async () => {
    mount(`${ALPHA}&depth=2`);

    await screen.findByTestId("canvas");
    expect(requested()[0]).toContain("depth=2");
    expect(screen.getByLabelText("Depth")).toHaveValue("2");
  });

  it("writes a new depth into the URL and draws it again", async () => {
    mount(ALPHA);
    await screen.findByTestId("canvas");

    await userEvent.selectOptions(screen.getByLabelText("Depth"), "2");

    await waitFor(() => {
      expect(requested().some((path) => path.includes("depth=2"))).toBe(true);
    });
    // In the URL, so the wider picture is a link like any other.
    expect(url()).toContain("depth=2");
  });

  it("keeps a multi-segment permalink whole inside the anchor", async () => {
    // A permalink is a path of its own, so the address splits at the first
    // slash after the domain and every slash after it stays in the permalink.
    mount("/graph?anchor=crystalline%3A%2F%2Feng%2Fnotes%2Fdeep%2Fgamma");

    await screen.findByTestId("canvas");
    expect(requested()[0]).toContain(
      "anchor=crystalline%3A%2F%2Feng%2Fnotes%2Fdeep%2Fgamma",
    );
  });

  it("asks for an anchor rather than failing without one", async () => {
    mount("/graph");

    expect(await screen.findByText(/no engram chosen yet/i)).toBeVisible();
    // An instruction, not an error, and nothing was asked of the server.
    expect(screen.queryByRole("alert")).toBeNull();
    expect(requested()).toEqual([]);
  });

  it("says what an anchor looks like when the one in the URL is not one", async () => {
    mount("/graph?anchor=alpha");

    expect(await screen.findByText(/crystalline:\/\//)).toBeVisible();
    expect(requested()).toEqual([]);
  });

  it("follows a node to its engram", async () => {
    mount(ALPHA);

    await userEvent.click(await screen.findByText("drawn:Beta"));

    await waitFor(() => {
      expect(url()).toBe("/d/eng/e/notes/beta");
    });
  });
});
