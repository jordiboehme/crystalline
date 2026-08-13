/**
 * The engram page: the screen this whole app exists to draw.
 *
 * What is pinned here is what the screen is allowed to claim. A wikilink is a
 * link only where the server resolved it and the graph says where it landed;
 * one the index looked for and did not find is marked and left unlinked; the
 * details panel shows the fields the engram carries and invents nothing for
 * the ones it does not, which for the temporal fields is the difference between
 * "valid forever" and a date nobody wrote. Backlinks are counted by relation
 * across the whole index rather than drawn from the capped neighborhood, they
 * cost no request at all when the detail payload already counted none, and the
 * empty case says so plainly instead of pretending the panel is still loading.
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

/**
 * An engram as one is written: the frontmatter, the title as an opening
 * heading, prose, and the two structured bullet kinds. The heading and the
 * bullets are here because what this screen must not do is draw any of them
 * twice.
 */
const BODY = [
  "---",
  "title: Alpha",
  "---",
  "",
  "# Alpha",
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
        line: 9,
        category: "decision",
        content: "we chose X",
        tags: ["tag"],
        context: "context",
      },
    ],
    relations: [
      {
        line: 10,
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

/**
 * What points at Alpha, as the inbound endpoint answers it: Beta's prose
 * wikilink, which is the same reference the neighborhood above carries as an
 * inbound `links_to` edge. The summary and the page come out of one function
 * because the endpoint answers both, differing only in whether a relation was
 * named.
 */
function inboundResponse(path: string) {
  const rel = new URLSearchParams(path.split("?")[1] ?? "").get("rel");
  return {
    total: 1,
    page: 1,
    limit: rel === null ? 1 : 20,
    count: rel === null ? 0 : 1,
    types: [{ rel: "links_to", count: 1 }],
    hits:
      rel === null
        ? []
        : [
            {
              domain: "eng",
              permalink: "notes/beta",
              title: "Beta",
              path: "notes/beta.md",
              rel: "links_to",
              status: "stable",
            },
          ],
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
      // What points here, which the backlinks panel reads on its own: the
      // summary on first paint, one relation's page when a chip is opened.
      "/domains/eng/inbound/alpha": (path: string) => inboundResponse(path),
      ...routes,
    }),
  );
}

beforeEach(() => {
  apiMock.mockReset();
  // The folded sections remember whether they were left open, so each test
  // starts from a browser that has never opened one.
  localStorage.clear();
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

    expect(await screen.findByText("Details")).toBeVisible();
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

  it("says where the engram lives in a trail above its title", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    const trail = await screen.findByRole("navigation", { name: "Breadcrumb" });
    // The domain is the one crumb that leads somewhere: there is no route for
    // a folder to point at, and the leaf is the page the reader is on.
    expect(within(trail).getByRole("link", { name: "eng" })).toHaveAttribute(
      "href",
      "/d/eng",
    );
    expect(within(trail).getByText("Alpha")).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  it("counts what points here by relation, and opens onto them on request", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    const panel = await screen.findByRole("region", { name: "Backlinks" });
    // The counts are of the whole index rather than of the capped
    // neighborhood, so the panel is a chip per relation and no rows at all
    // until one is opened.
    const chip = await within(panel).findByRole("button", {
      name: /links_to/,
    });
    expect(chip).toHaveTextContent("1");
    expect(within(panel).queryByRole("link")).toBeNull();

    await userEvent.click(chip);

    expect(
      await screen.findByRole("link", { name: "Beta, eng / notes/beta.md" }),
    ).toHaveAttribute("href", "/d/eng/e/notes/beta");
  });

  it("says what the server said when what points here could not be read", async () => {
    serve({
      // A refusal rather than a server error, so the query layer answers it
      // once instead of retrying: what is pinned here is the message, not the
      // retry policy.
      "/domains/eng/inbound/alpha": () => {
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

  it("says plainly when nothing points here yet, without asking", async () => {
    serve({
      // The detail payload omits the inbound block entirely when nothing
      // points here, which is exactly the case this panel must answer from
      // what the page has already read.
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ inbound: undefined }),
    });

    renderApp("/d/eng/e/alpha");

    expect(await screen.findByText(/nothing links here yet/i)).toBeVisible();
    expect(apiMock.mock.calls.map((call) => String(call[0]))).not.toContain(
      "/domains/eng/inbound/alpha?page=1&limit=1",
    );
  });

  it("draws an observation once, in the body, as what it is", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    // Once: the written line and its indexed reading are the same line, so a
    // second list of them would be the same page saying one thing twice.
    const hits = await screen.findAllByText(/we chose X/);
    expect(hits).toHaveLength(1);
    // And nothing is lost by that: in brackets, the way it was written, which
    // is what tells the category apart from the engram's type, with the tag
    // and the context still on the line.
    const [line] = hits;
    expect(screen.getByText("[decision]")).toBeVisible();
    expect(line).toHaveTextContent("#tag");
    expect(line).toHaveTextContent("(context)");
  });

  it("draws a relation once, in the body, as what it is", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    const body = await screen.findByRole("article");
    const relType = await within(body).findByText("superseded_by");
    expect(relType).toBeVisible();
    // The type is on the bullet the target is on, and the target is the link
    // the graph placed rather than a second copy of the same fact.
    expect(within(body).getAllByText("superseded_by")).toHaveLength(1);
  });

  it("draws the title once", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    // The header's h1 is the page's rendering of the title; the body opens
    // with the same line and it folds away rather than repeating it.
    const headings = await screen.findAllByRole("heading", { name: "Alpha" });
    expect(headings).toHaveLength(1);
    expect(headings[0]).toHaveAttribute("id", "engram-title");
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

  it("puts the utilities on the row as icons and the writes behind one menu", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    // A quiet strip of icons, then the one labelled thing somebody came to
    // this header for. Each icon carries EXACTLY the name its menu row carried,
    // because the name is the whole contract a keyboard or a screen reader
    // holds over a control with no text in it.
    for (const name of ["Share link", "Download as Markdown", "Print view"]) {
      expect(await screen.findByRole("button", { name })).toBeVisible();
    }
    expect(screen.getByRole("link", { name: "Edit" })).toHaveAttribute(
      "href",
      "/d/eng/edit/alpha",
    );
    expect(screen.queryByRole("button", { name: "Retire" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Move" })).toBeNull();

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "More actions" }));

    // What is left behind the fold is what belongs there: the move, and the
    // destructive one alone under a rule.
    for (const name of ["Move", "Retire"]) {
      expect(await screen.findByRole("menuitem", { name })).toBeVisible();
    }
    // And nothing is offered twice.
    for (const name of ["Share link", "Download as Markdown", "Print view"]) {
      expect(screen.queryByRole("menuitem", { name })).toBeNull();
    }
  });

  it("carries the actions in the trail's own row, leaving the title alone", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    const trail = await screen.findByRole("navigation", { name: "Breadcrumb" });
    const row = trail.parentElement;
    expect(row).not.toBeNull();
    // Where this engram is and what can be done with it are one line: the
    // address reads from the left, the controls sit at its right end.
    expect(
      within(row as HTMLElement).getByRole("link", { name: "Edit" }),
    ).toBeInTheDocument();
    expect(
      within(row as HTMLElement).getByRole("button", { name: "More actions" }),
    ).toBeInTheDocument();
    expect(row).toHaveClass("justify-between");

    // And the title stands by itself, with nothing in its block to compete
    // with it - which is the whole point of the row above.
    const title = screen.getByRole("heading", { name: "Alpha", level: 1 });
    expect(row).not.toContainElement(title);
    // The trail's row, then the title, and that is the whole header: no
    // control row underneath the name any more.
    expect(title.previousElementSibling).toBe(row);
    expect(title.parentElement?.lastElementChild).toBe(title);
  });

  it("runs a utility from its icon rather than from a second copy of it", async () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    serve();

    renderApp("/d/eng/e/alpha");

    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Print view" }));

    expect(print).toHaveBeenCalled();
    print.mockRestore();
  });

  it("opens the guided retirement from the menu", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "More actions" }),
    );
    await user.click(await screen.findByRole("menuitem", { name: "Retire" }));

    expect(
      await screen.findByRole("button", { name: "Retire engram" }),
    ).toBeVisible();
  });

  it("offers a reader who may not write the utilities and nothing else", async () => {
    serve({ "/auth/me": () => meResponse({ anonymous: true }) });

    renderApp("/d/eng/e/alpha");

    await screen.findByRole("heading", { name: "Alpha" });
    expect(screen.queryByRole("link", { name: "Edit" })).toBeNull();

    // The three utilities are everybody's, and they are the whole row now.
    for (const name of ["Share link", "Download as Markdown", "Print view"]) {
      expect(screen.getByRole("button", { name })).toBeVisible();
    }
    // The writes are absent, which leaves the menu with nothing to hold: an
    // ellipsis that opens onto an empty panel is worse than no ellipsis.
    expect(screen.queryByRole("button", { name: "More actions" })).toBeNull();
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
    // The body's wikilinks are resolved from the same neighborhood, so once
    // they are links the graph section has one too.
    const body = await screen.findByRole("article");
    await within(body).findAllByRole("link", { name: "Beta" });

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

  it("opens the sections a reader left open last time", async () => {
    // The choice to read this way outlives the visit it was made on, under a
    // key of each section's own.
    localStorage.setItem("fluid.section.graph", "open");
    localStorage.setItem("fluid.section.agents-eye", "open");
    serve();

    renderApp("/d/eng/e/alpha");

    expect(
      await screen.findByRole("button", { name: "Hide the neighborhood" }),
    ).toHaveAttribute("aria-expanded", "true");
    expect(
      screen.getByRole("button", { name: /what an agent is taught/i }),
    ).toHaveAttribute("aria-expanded", "true");
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
