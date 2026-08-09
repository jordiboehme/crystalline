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

/** The graph renderer paints to a canvas, which jsdom has none of. */
vi.mock("../components/GraphCanvas", () => ({
  default: () => <div data-testid="canvas" />,
}));

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
      // The frame around this screen browses the domain it is in, so the tree
      // is stubbed here the way the domain listing is: an unstubbed one would
      // put a failed sidebar beside every assertion below.
      "/domains/eng/tree": () => ({
        domain: "eng",
        path: "/",
        folders: [],
        engrams: [
          {
            permalink: "alpha",
            title: "Alpha",
            type: "decision",
            path: "alpha.md",
          },
        ],
      }),
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

  it("never calls the successor unresolved while the graph is still coming", async () => {
    // The ordinary load path: the graph is asked for only once the detail has
    // landed, so every retired engram spends at least one round trip here.
    serve({ "/graph": () => new Promise(() => undefined) });

    renderApp("/d/eng/e/alpha");

    const banner = await screen.findByRole("note");
    // The index resolved it, so any claim that nothing does would be false,
    // and there is nothing to hover over saying one.
    const successor = within(banner).getByText("Beta");
    expect(successor).not.toHaveAttribute("title");
    expect(within(banner).queryByRole("link")).toBeNull();
  });

  it("shows a successor that declared the relation from its own side", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        // Alpha declares nothing: the successor is the one that wrote
        // `- supersedes [[Alpha]]`, so the fact lives on an inbound edge.
        detailResponse({ relations: [], links: [] }),
      "/graph": () =>
        graphResponse({
          edges: [{ from: 2, to: 1, rel_type: "supersedes" }],
        }),
    });

    renderApp("/d/eng/e/alpha");

    const banner = await screen.findByRole("note");
    await waitFor(() => {
      expect(
        within(banner).getByRole("link", { name: "Beta" }),
      ).toHaveAttribute("href", "/d/eng/e/notes/beta");
    });
    expect(banner).toHaveTextContent("Superseded by");
  });

  it("lists a successor named on both sides only once", async () => {
    serve({
      "/graph": () =>
        graphResponse({
          edges: [
            { from: 1, to: 2, rel_type: "superseded_by" },
            { from: 2, to: 1, rel_type: "supersedes" },
          ],
        }),
    });

    renderApp("/d/eng/e/alpha");

    const banner = await screen.findByRole("note");
    await waitFor(() => {
      expect(
        within(banner).getAllByRole("link", { name: "Beta" }),
      ).toHaveLength(1);
    });
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

  it("says what the server said when the neighborhood could not be read", async () => {
    serve({
      // A refusal rather than a server error, so the query layer answers it
      // once instead of retrying: what is pinned here is the message, not the
      // retry policy.
      "/graph": () => {
        throw new ApiProblem(403, "forbidden", "this account may not read eng");
      },
    });

    renderApp("/d/eng/e/alpha");

    const panel = await screen.findByRole("region", { name: "Backlinks" });
    const alert = await within(panel).findByRole("alert");
    // The server's own words, the way every other error surface in this app
    // shows them: a house sentence would hide the one thing worth reading.
    expect(alert).toHaveTextContent("this account may not read eng");
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
    // Scoped to the Relations panel: the same relation type also opens the
    // markdown bullet it was declared in, further up the page.
    const relations = screen.getByRole("region", { name: "Relations" });
    expect(within(relations).getByText("superseded_by")).toBeVisible();
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

    const button = await screen.findByRole("button", { name: "Copy address" });
    await userEvent.click(button);

    expect(writeText).toHaveBeenCalledWith("crystalline://eng/alpha");
    // Announced rather than merely drawn: the outcome lands in a live region,
    // so somebody who cannot see the label change is told it worked.
    const outcome = screen.getByRole("status", { name: "Copy address result" });
    await waitFor(() => {
      expect(outcome).toHaveTextContent("Copied");
    });
    // And the control keeps its name, so it is not silently renamed under a
    // reader navigating by control.
    expect(button).toHaveAccessibleName("Copy address");
  });

  it("says so when the browser refuses the clipboard", async () => {
    Object.defineProperty(navigator, "clipboard", {
      value: {
        writeText: () => Promise.reject(new Error("denied")),
      },
      configurable: true,
    });
    serve();

    renderApp("/d/eng/e/alpha");

    await userEvent.click(
      await screen.findByRole("button", { name: "Copy address" }),
    );

    const outcome = screen.getByRole("status", { name: "Copy address result" });
    await waitFor(() => {
      expect(outcome).toHaveTextContent("Copy refused");
    });
  });

  it("keeps the graph folded away until somebody asks for it", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    // Folded, and unbuilt: the drawing is the heaviest thing this page can
    // load, and a reader who never opens it never pays for it.
    const toggle = await screen.findByRole("button", { name: /neighborhood/i });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByTestId("canvas")).toBeNull();
  });

  it("draws the neighborhood the page already read, on request", async () => {
    serve();

    renderApp("/d/eng/e/alpha");
    // The backlinks panel is drawn from the same neighborhood, so once it has
    // one the graph section has one too.
    const panel = await screen.findByRole("region", { name: "Backlinks" });
    await within(panel).findByRole("link", { name: /Beta/ });

    await userEvent.click(
      screen.getByRole("button", { name: /neighborhood/i }),
    );

    // Under the same cache key, so the section opens onto the neighborhood
    // rather than onto a wait for one.
    expect(screen.queryByText(/reading the neighborhood/i)).toBeNull();
    expect(await screen.findByTestId("canvas")).toBeVisible();
    // And the full view is a link away, pointed at this engram already.
    expect(screen.getByRole("link", { name: /full view/i })).toHaveAttribute(
      "href",
      "/graph?anchor=crystalline%3A%2F%2Feng%2Falpha",
    );
  });

  it("keeps the agent's-eye view folded away until somebody asks for it", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    const toggle = await screen.findByRole("button", {
      name: /what an agent is taught/i,
    });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText(/Route here for eng questions/)).toBeNull();
  });

  it("shows what an agent is taught about this engram", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    await userEvent.click(
      await screen.findByRole("button", { name: /what an agent is taught/i }),
    );

    const panel = await screen.findByRole("region", { name: /agent/i });
    // The domain's routing line, which is what sends an agent here at all.
    expect(panel).toHaveTextContent("Route here for eng questions.");
    // The salience the engram carries, which is what ranks it once it is here.
    expect(within(panel).getByText("7")).toBeVisible();
    // And what reading it costs, named as the estimate it is rather than as a
    // count nobody measured.
    const tokens = Math.ceil(BODY.length / 4);
    expect(panel).toHaveTextContent(new RegExp(`${String(tokens)} tokens`));
    expect(panel).toHaveTextContent(/approximate/i);
  });

  it("says nothing about a salience the engram does not carry", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({
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

    await userEvent.click(
      await screen.findByRole("button", { name: /what an agent is taught/i }),
    );

    const panel = await screen.findByRole("region", { name: /agent/i });
    expect(panel).toHaveTextContent("Route here for eng questions.");
    // No row, no placeholder, no zero: an engram with no salience has none.
    expect(within(panel).queryByText(/salience/i)).toBeNull();
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
