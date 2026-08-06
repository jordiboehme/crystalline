/**
 * The engram page: the screen this whole app exists to draw.
 *
 * What is pinned here is what the screen is allowed to claim. A wikilink is a
 * link only where the server resolved it and the graph says where it landed;
 * one the index looked for and did not find is marked and left unlinked; the
 * frontmatter panel shows the fields the engram carries and invents nothing for
 * the ones it does not, which for the temporal fields is the difference between
 * "valid forever" and a date nobody wrote. Backlinks come from the graph rather
 * than from the detail payload's capped sample, and the empty case says so
 * plainly instead of pretending the panel is still loading.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

const BODY = [
  "---",
  "title: Alpha",
  "---",
  "",
  "Body prose linking [[Beta]] and [[Nowhere]] inline.",
  "",
  "- [decision] we chose X #tag (context)",
  "- superseded_by [[Beta]]",
  "",
].join("\n");

/**
 * The detail payload, in the engine's own shape: the frontmatter is the parsed
 * struct, so `type` is `engram_type` there and `salience` sits under `extra`.
 */
function detailResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    permalink: "alpha",
    title: "Alpha",
    type: "decision",
    status: "superseded",
    path: "alpha.md",
    url: "crystalline://eng/alpha",
    content: BODY,
    checksum: "3f8a1c05e2",
    frontmatter: {
      engram_type: "decision",
      title: "Alpha",
      permalink: "alpha",
      status: "superseded",
      tags: ["one", "two"],
      extra: { salience: 7 },
      valid_from: "2026-01-02",
      valid_to: "2026-06-30",
      stale_after: "2026-01-01",
      verified: [{ by: "human:jordi", at: "2026-02-01T10:00:00+01:00" }],
      last_verified: null,
      review_after: null,
      recorded_at: null,
    },
    observations: [
      {
        line: 7,
        category: "decision",
        content: "we chose X",
        tags: ["tag"],
        context: "context",
      },
    ],
    relations: [
      {
        line: 8,
        rel_type: "superseded_by",
        resolved: true,
        target: { domain: null, target: "Beta" },
      },
    ],
    links: [
      { line: 5, resolved: true, target: { domain: null, target: "Beta" } },
      { line: 5, resolved: false, target: { domain: null, target: "Nowhere" } },
    ],
    inbound: {
      count: 1,
      refs: [{ domain: "eng", path: "beta.md", kind: "link" }],
    },
    ...overrides,
  };
}

/** The neighborhood: Beta on both ends of the arrow, one hop out. */
function graphResponse(overrides: Record<string, unknown> = {}) {
  return {
    nodes: [
      {
        id: 1,
        domain: "eng",
        permalink: "alpha",
        title: "Alpha",
        status: "superseded",
        type: "decision",
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
    edges: [
      { from: 1, to: 2, rel_type: "superseded_by" },
      { from: 1, to: 2, rel_type: "links_to" },
      { from: 2, to: 1, rel_type: "links_to" },
    ],
    truncated: false,
    ...overrides,
  };
}

function serve(routes: Record<string, (path: string) => unknown> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      "/domains/eng/engrams/alpha": () => detailResponse(),
      "/graph": () => graphResponse(),
      ...routes,
    }),
  );
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the engram page", () => {
  it("draws the body and links the wikilinks the server resolved", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    // Scoped to the body, because the same engram is named by the lifecycle
    // banner, the relations list and the backlinks panel as well. Both
    // occurrences in the body are linkified: the one in the prose and the one
    // in the relation bullet, which is part of the markdown as written.
    const body = await screen.findByRole("article");
    await waitFor(() => {
      const links = within(body).getAllByRole("link", { name: "Beta" });
      expect(links).toHaveLength(2);
      for (const link of links) {
        // The permalink comes from the graph, not from the bracket text: the
        // detail payload only ever says "Beta".
        expect(link).toHaveAttribute("href", "/d/eng/e/notes/beta");
      }
    });
  });

  it("marks a wikilink the index could not resolve rather than linking it", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    const marked = await screen.findByTitle("not resolved");
    expect(marked).toHaveTextContent("[[Nowhere]]");
    expect(screen.queryByRole("link", { name: /Nowhere/ })).toBeNull();
  });

  it("shows the frontmatter the engram carries", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    expect(await screen.findByText("decision")).toBeVisible();
    expect(screen.getByRole("link", { name: "#one" })).toHaveAttribute(
      "href",
      "/search?tags=one",
    );
    expect(screen.getByText("7")).toBeVisible();
    expect(screen.getByText(/2026-01-02/)).toBeVisible();
    expect(screen.getByText(/human:jordi/)).toBeVisible();
  });

  it("shows nothing at all for the temporal fields an engram leaves out", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({
          status: "stable",
          frontmatter: {
            engram_type: "engram",
            title: "Alpha",
            permalink: "alpha",
            status: "stable",
            tags: [],
            extra: {},
            valid_from: null,
            valid_to: null,
            stale_after: null,
            verified: [],
            last_verified: null,
            review_after: null,
            recorded_at: null,
          },
        }),
    });

    renderApp("/d/eng/e/alpha");

    expect(await screen.findByText("Frontmatter")).toBeVisible();
    // Absent means always valid and valid forever, so the row is absent too.
    // A placeholder here would be a date nobody wrote.
    expect(screen.queryByText("Valid")).toBeNull();
    expect(screen.queryByText("Salience")).toBeNull();
    expect(screen.queryByText("Verified")).toBeNull();
    expect(screen.queryByText(/forever/i)).toBeNull();
    expect(screen.queryByText(/9999/)).toBeNull();
  });

  it("banners a retired engram and points at what replaced it", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    expect(await screen.findByText(/kept for the record/)).toBeVisible();
    const successors = await screen.findAllByRole("link", { name: "Beta" });
    expect(
      successors.some(
        (link) => link.getAttribute("href") === "/d/eng/e/notes/beta",
      ),
    ).toBe(true);
  });

  it("lists what points here, from the graph rather than the capped sample", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    const panel = await screen.findByRole("region", { name: "Backlinks" });
    await waitFor(() => {
      expect(
        within(panel).getByRole("link", { name: "Beta, notes/beta" }),
      ).toHaveAttribute("href", "/d/eng/e/notes/beta");
    });
    // How it points, which is what the edge carries beyond the fact of it.
    expect(panel).toHaveTextContent("links_to");
  });

  it("says plainly when nothing points here yet", async () => {
    serve({
      "/graph": () =>
        graphResponse({
          nodes: [
            {
              id: 1,
              domain: "eng",
              permalink: "alpha",
              title: "Alpha",
              status: "stable",
              type: "decision",
            },
          ],
          edges: [],
        }),
    });

    renderApp("/d/eng/e/alpha");

    expect(await screen.findByText(/nothing links here yet/i)).toBeVisible();
  });

  it("lists the observations and the relations the body declares", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    expect(await screen.findByText("we chose X")).toBeVisible();
    // In brackets, the way it was written, which is what tells the category
    // apart from the same word used as the engram's type.
    expect(screen.getByText("[decision]")).toBeVisible();
    expect(screen.getByText("#tag")).toBeVisible();
    expect(screen.getByText("superseded_by")).toBeVisible();
  });

  it("copies the engram's crystalline address", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>(() =>
      Promise.resolve(),
    );
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    serve();

    renderApp("/d/eng/e/alpha");

    const button = await screen.findByRole("button", { name: /copy address/i });
    await userEvent.click(button);

    expect(writeText).toHaveBeenCalledWith("crystalline://eng/alpha");
    await waitFor(() => {
      expect(screen.getByText(/copied/i)).toBeVisible();
    });
  });

  it("says an engram nobody wrote is a wrong address", async () => {
    serve({
      "/domains/eng/engrams/alpha": () => {
        throw new ApiProblem(404, "not found", "no engram 'alpha' in 'eng'");
      },
    });

    renderApp("/d/eng/e/alpha");

    expect(
      await screen.findByRole("heading", { name: "Engram not found" }),
    ).toBeVisible();
  });
});
