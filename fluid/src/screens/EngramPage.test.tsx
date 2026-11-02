/**
 * The engram page: the screen this whole app exists to draw.
 *
 * What is pinned here is what the screen is allowed to claim. A wikilink is a
 * link only where the server resolved it and the graph says where it landed;
 * one the index looked for and did not find is marked and left unlinked; the
 * details panel shows the fields the engram carries and invents nothing for
 * the ones it does not, which for the temporal fields is the difference between
 * "valid forever" and a date nobody wrote. Inbound links are read from the inbound
 * endpoint rather than drawn from the capped neighborhood, they
 * cost no request at all when the detail payload already counted none, and the
 * empty case says so plainly instead of pretending the panel is still loading.
 */

import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { NavigateFunction } from "react-router";
import {
  MemoryRouter,
  useLocation,
  useNavigate,
  useNavigationType,
} from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import App from "../App";
import { ApiProblem, api } from "../api/client";
import { FakeEventSource } from "../events/testSupport";
import { LAYOUT_WIDTH_KEY } from "../layoutWidth";
import { engramRoute } from "../paths";
import type { Answer } from "../test/harness";
import {
  answersFor,
  defaultPolicyRows,
  domainsResponse,
  manifestSectionsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";

// A tick whose box is no longer where the page drew it: `refuse` makes the
// next `tickAt` answer null, the way an engram that moved underneath does.
const tickControl = vi.hoisted(() => ({ refuse: false }));
vi.mock("../taskTick", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../taskTick")>();
  return {
    ...actual,
    tickAt: (...args: Parameters<typeof actual.tickAt>) =>
      tickControl.refuse ? null : actual.tickAt(...args),
  };
});
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
function inboundResponse(_path: string) {
  return {
    total: 1,
    page: 1,
    limit: 20,
    count: 1,
    types: [{ rel: "links_to", count: 1 }],
    hits: [
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

function serve(routes: Record<string, Answer> = {}) {
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
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: "",
        sections: manifestSectionsResponse(),
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

// Above roughly load average 80 this file's slower tests come close to the
// 5000 ms default (measured 2026-10-28); the raise keeps a loaded machine from
// reading as a failure. Never raise the global default to hide this.
vi.setConfig({ testTimeout: 15000 });

describe("the engram page", () => {
  it("a locked domain's engram page shows the lock and its text and offers no Edit", async () => {
    serve({
      "/domains": () => ({
        ...domainsResponse(),
        domains: [{ ...domainsResponse().domains[0], read_only: true }],
      }),
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: "",
        sections: manifestSectionsResponse({
          policies: defaultPolicyRows().map((row) =>
            row.key === "read_only"
              ? { ...row, declared: "on", effective: "on" }
              : row,
          ),
        }),
      }),
    });
    renderApp("/d/eng/e/alpha");
    expect(await screen.findByText("read-only")).toBeInTheDocument();
    expect(
      screen.getByText(
        "its owner switches read_only off in the domain policies",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("link", { name: "Edit" }),
    ).not.toBeInTheDocument();
    // The menu stays for the utilities; the write rows are not in it.
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "More actions" }));
    expect(
      await screen.findByRole("menuitem", { name: "Share link" }),
    ).toBeVisible();
    expect(screen.queryByRole("menuitem", { name: "Move" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: "Retire" })).toBeNull();
  });

  it("a domain off this caller's write list offers no Edit and wears no lock", async () => {
    serve({
      "/domains": () => ({
        ...domainsResponse(),
        domains: [{ ...domainsResponse().domains[0], read_only: true }],
      }),
    });
    renderApp("/d/eng/e/alpha");
    expect(
      await screen.findByRole("heading", { level: 1 }),
    ).toBeInTheDocument();
    await waitFor(() => {
      expect(
        screen.queryByRole("link", { name: "Edit" }),
      ).not.toBeInTheDocument();
    });
    expect(screen.queryByText("read-only")).not.toBeInTheDocument();
    expect(
      screen.queryByText(
        "its owner switches read_only off in the domain policies",
      ),
    ).not.toBeInTheDocument();
  });

  describe("the outline of a long page", () => {
    const LONG = `${BODY}\n## Setup\n\ntext\n\n### Install\n\ntext\n\n## Usage\n\ntext\n\n## Limits\n\ntext\n`;
    const HREFS = ["#setup", "#install", "#usage", "#limits"];

    /** A window of this many rem: answers min-width queries by it. */
    function stubWidth(rem: number) {
      Object.defineProperty(window, "matchMedia", {
        configurable: true,
        value: (query: string) => {
          const min = /min-width:\s*([\d.]+)rem/.exec(query);
          return {
            matches: min ? rem >= Number(min[1]) : false,
            media: query,
            addEventListener: () => undefined,
            removeEventListener: () => undefined,
          };
        },
      });
    }
    afterEach(() => {
      Reflect.deleteProperty(window, "matchMedia");
    });

    function hrefs(nav: HTMLElement) {
      return Array.from(nav.querySelectorAll("a")).map((a) =>
        a.getAttribute("href"),
      );
    }

    it("leads the rail from lg up", async () => {
      stubWidth(80);
      serve({
        "/domains/eng/engrams/alpha": () => detailResponse({ content: LONG }),
      });
      renderApp("/d/eng/e/alpha");
      const nav = await screen.findByRole("navigation", {
        name: "On this page",
      });
      expect(hrefs(nav)).toEqual(HREFS);
      expect(nav.closest("aside")).not.toBeNull();
    });

    it("is a collapsed disclosure over the body between md and lg", async () => {
      stubWidth(52);
      serve({
        "/domains/eng/engrams/alpha": () => detailResponse({ content: LONG }),
      });
      renderApp("/d/eng/e/alpha");
      const summary = await screen.findByText("On this page", {
        selector: "summary",
      });
      const details = summary.closest("details");
      expect(details).not.toBeNull();
      expect(details?.open).toBe(false);
      expect(details?.nextElementSibling?.tagName).toBe("ARTICLE");
      expect(
        document.querySelector("aside nav[aria-label='On this page']"),
      ).toBeNull();
      expect(
        hrefs(
          within(details as HTMLElement).getByRole("navigation", {
            name: "On this page",
            hidden: true,
          }),
        ),
      ).toEqual(HREFS);
    });

    it("is a disclosure at full width, even on a wide window", async () => {
      stubWidth(80);
      localStorage.setItem(LAYOUT_WIDTH_KEY, "full");
      serve({
        "/domains/eng/engrams/alpha": () => detailResponse({ content: LONG }),
      });
      renderApp("/d/eng/e/alpha");
      const summary = await screen.findByText("On this page", {
        selector: "summary",
      });
      expect(summary.closest("details")).not.toBeNull();
      expect(
        document.querySelector("aside nav[aria-label='On this page']"),
      ).toBeNull();
    });

    it("is neither in the page nor the rail below md (the drawer has it)", async () => {
      stubWidth(30);
      serve({
        "/domains/eng/engrams/alpha": () => detailResponse({ content: LONG }),
      });
      renderApp("/d/eng/e/alpha");
      await screen.findByRole("heading", { name: "Alpha", level: 1 });
      await screen.findByRole("heading", { name: "Setup" });
      expect(document.querySelector("details")).toBeNull();
      expect(
        document.querySelector("aside nav[aria-label='On this page']"),
      ).toBeNull();
    });
  });

  it("draws no outline for a short page", async () => {
    serve();
    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha", level: 1 });
    expect(
      screen.queryByRole("navigation", { name: "On this page" }),
    ).toBeNull();
  });

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

  it("links a wikilink that names its domain by its canonical name or an alias", async () => {
    // Registered here as `moonbase`, called `moon` by its MANIFEST and `lunar`
    // before a rename: the server resolves all three spellings, and so must
    // the page, onto the local name every route carries.
    const listing = domainsResponse();
    serve({
      "/domains": () => ({
        ...listing,
        domains: [
          ...listing.domains,
          {
            name: "moonbase",
            kind: "file",
            canonical_name: "moon",
            aliases: ["lunar"],
            shadowed: false,
          },
        ],
      }),
      "/domains/eng/engrams/alpha": () =>
        detailResponse({
          content: BODY.replace(
            "Body prose linking",
            "See [[moon:Crater Base]] and [[lunar:Crater Base]]. Body prose linking",
          ),
          links: [
            {
              line: 7,
              resolved: true,
              target: { domain: "moon", target: "Crater Base" },
            },
            {
              line: 7,
              resolved: true,
              target: { domain: "lunar", target: "Crater Base" },
            },
          ],
        }),
      "/graph": () => {
        const graph = graphResponse();
        return {
          ...graph,
          nodes: [
            ...graph.nodes,
            {
              id: 3,
              domain: "moonbase",
              permalink: "crater-base",
              title: "Crater Base",
              status: "stable",
              type: "engram",
            },
          ],
        };
      },
    });

    renderApp("/d/eng/e/alpha");

    const body = await screen.findByRole("article");
    await waitFor(() => {
      const links = within(body).getAllByRole("link", { name: "Crater Base" });
      expect(links).toHaveLength(2);
      for (const link of links) {
        expect(link).toHaveAttribute("href", "/d/moonbase/e/crater-base");
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

    // Status and type are on the meta line under the title, once (and in
    // the printed title block, which the screen never shows).
    const shown = (await screen.findAllByText("decision")).filter(
      (element) => element.closest("[data-print]") === null,
    );
    expect(shown).toHaveLength(1);
    expect(shown[0]).toBeVisible();
    expect(screen.getByRole("link", { name: "#one" })).toHaveAttribute(
      "href",
      "/search?tags=one",
    );
    expect(screen.getByText("7")).toBeVisible();
    expect(screen.getByText(/2026-01-02/)).toBeVisible();
    expect(screen.getByText(/jordi \(human\)/)).toBeVisible();
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

  it("labels a successor written by permalink with its title", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({
          content: BODY.replace(
            "- superseded_by [[Beta]]",
            "- superseded_by [[notes/beta]]",
          ),
          relations: [
            {
              line: 10,
              rel_type: "superseded_by",
              resolved: true,
              target: { domain: null, target: "notes/beta" },
            },
          ],
        }),
    });

    renderApp("/d/eng/e/alpha");

    const banner = await screen.findByRole("note");
    await waitFor(() => {
      expect(
        within(banner).getByRole("link", { name: "Beta" }),
      ).toHaveAttribute("href", "/d/eng/e/notes/beta");
    });
    // The engine writes the permalink because it is the stable identity and
    // cannot be misread as a domain prefix. What a reader sees is the name.
    expect(banner).not.toHaveTextContent("notes/beta");
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
    // The domain is in the top bar: on screen the trail is folders and the
    // title. The leaf is the page the reader is on.
    expect(within(trail).queryByRole("link", { name: "eng" })).toBeNull();
    expect(within(trail).getByText("Alpha")).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  it("names what links here in the rail, each with its relation", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    const block = await screen.findByRole("region", { name: "Linked from" });
    const link = await within(block).findByRole("link", { name: "Beta" });
    expect(link).toHaveAttribute("href", "/d/eng/e/notes/beta");
    expect(block).toHaveTextContent("links_to");
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

    const panel = await screen.findByRole("region", { name: "Linked from" });
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
      "/domains/eng/inbound/alpha?page=1&limit=20",
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

    const button = await screen.findByRole("button", {
      name: "Copy crystalline:// address",
    });
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
    expect(button).toHaveAccessibleName("Copy crystalline:// address");
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
      await screen.findByRole("button", {
        name: "Copy crystalline:// address",
      }),
    );

    const outcome = screen.getByRole("status", { name: "Copy address result" });
    await waitFor(() => {
      expect(outcome).toHaveTextContent("Copy refused");
    });
  });

  it("puts Edit as a secondary button and everything else behind More actions", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    const edit = await screen.findByRole("link", { name: "Edit" });
    expect(edit).toHaveAttribute("href", "/d/eng/edit/alpha");
    // Secondary, not the filled primary (spec 3.1).
    expect(edit.className).not.toContain("bg-accent-700");
    expect(edit.className).toContain("border");
    // None of the rows is a button on the row any more.
    for (const name of [
      "Share link",
      "Download as Markdown",
      "Print view",
      "Move",
      "Retire",
    ]) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "More actions" }));

    // The utilities lead, then a rule, then Move and Retire.
    const rows = (await screen.findAllByRole("menuitem")).map(
      (row) => row.textContent,
    );
    expect(rows).toEqual([
      "Share link",
      "Download as Markdown",
      "Print view",
      "Move",
      "Retire",
    ]);
    expect(screen.getAllByRole("separator")).toHaveLength(1);
  });

  it("draws Retire in red and no other row", async () => {
    serve();
    renderApp("/d/eng/e/alpha");
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "More actions" }),
    );
    expect(
      (await screen.findByRole("menuitem", { name: "Retire" })).className,
    ).toContain("text-red-700");
    for (const name of [
      "Share link",
      "Download as Markdown",
      "Print view",
      "Move",
    ]) {
      expect(screen.getByRole("menuitem", { name }).className).not.toContain(
        "text-red",
      );
    }
  });

  it("opens with Enter, walks with the arrows and gives focus back on Escape", async () => {
    serve();
    renderApp("/d/eng/e/alpha");
    const user = userEvent.setup();
    const more = await screen.findByRole("button", { name: "More actions" });
    more.focus();
    await user.keyboard("{Enter}");
    const share = await screen.findByRole("menuitem", { name: "Share link" });
    await waitFor(() => {
      expect(share).toHaveFocus();
    });
    await user.keyboard("{ArrowDown}");
    expect(
      screen.getByRole("menuitem", { name: "Download as Markdown" }),
    ).toHaveFocus();
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
    expect(more).toHaveFocus();
  });

  it("offers an editor who may write the write rows and a read-only reader only the utilities", async () => {
    serve({ "/auth/me": () => meResponse({ anonymous: true }) });
    renderApp("/d/eng/e/alpha");
    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "More actions" }),
    );
    const rows = (await screen.findAllByRole("menuitem")).map(
      (row) => row.textContent,
    );
    expect(rows).toEqual(["Share link", "Download as Markdown", "Print view"]);
    expect(screen.queryByRole("separator")).toBeNull();
  });

  it("names the trail by folder and title, folders linking to their page", async () => {
    serve({
      "/domains/eng/engrams/deploy/night": () =>
        detailResponse({
          permalink: "deploy/night",
          title: "Night Deploys",
          path: "deploy/night.md",
          url: "crystalline://eng/deploy/night",
        }),
    });
    renderApp("/d/eng/e/deploy/night");
    const trail = await screen.findByRole("navigation", { name: "Breadcrumb" });
    expect(within(trail).getByRole("link", { name: "deploy" })).toHaveAttribute(
      "href",
      "/d/eng/engrams?path=deploy",
    );
    expect(within(trail).getByText("Night Deploys")).toHaveAttribute(
      "aria-current",
      "page",
    );
    // The domain is in the bar; the trail holds it for print only.
    expect(within(trail).queryByRole("link", { name: "eng" })).toBeNull();
  });

  it("puts status, type and the parsed description on one line under the title", async () => {
    // The file writes it as a folded block scalar; the page shows the engine's
    // parsed text and never the source's `>-`.
    serve({
      "/domains/eng/engrams/alpha": () => {
        const base = detailResponse();
        return detailResponse({
          content:
            "---\ndescription: >-\n  Why we deploy\n  at night\n---\n# Alpha\n\nBody.\n",
          frontmatter: {
            ...(base.frontmatter as Record<string, unknown>),
            description: "Why we deploy at night",
          },
        });
      },
    });
    renderApp("/d/eng/e/alpha");
    const meta = await screen.findByLabelText("About this engram");
    expect(meta).toHaveTextContent("superseded");
    expect(meta).toHaveTextContent("decision");
    const text = within(meta).getByText("Why we deploy at night");
    expect(text).toHaveAttribute("title", "Why we deploy at night");
    expect(text.className).toContain("line-clamp-2");
    expect(meta.textContent).not.toContain(">-");
  });

  it("names the ellipsis on hover without renaming it", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    const share = await screen.findByRole("button", { name: "More actions" });
    // Nothing hangs over the row until it is asked for, and when it is, the
    // words are the ones the control is already called. The browser's own
    // tooltip is gone: two labels for one button, drawn a beat apart, is
    // worse than either alone.
    expect(share).not.toHaveAttribute("title");
    expect(screen.queryByRole("tooltip")).toBeNull();

    const user = userEvent.setup();
    await user.hover(share);
    expect(
      await screen.findByRole("tooltip", {}, { timeout: 2000 }),
    ).toHaveTextContent("More actions");
    // The accessible name is still the label alone: a tooltip describes.
    expect(screen.getByRole("button", { name: "More actions" })).toBe(share);
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
    // The trail's row, then the title's own line, and that is the whole
    // header: no control row underneath the name any more. A line rather than
    // the heading alone, because the chip saying what changed stands on it -
    // and on a page the team already has, as here, the heading is all it
    // holds.
    const line = title.parentElement;
    expect(line?.previousElementSibling).toBe(row);
    expect(line?.lastElementChild).toBe(title);
  });

  it("runs a utility from its icon rather than from a second copy of it", async () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    serve();

    renderApp("/d/eng/e/alpha");

    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "More actions" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: "Print view" }),
    );

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

  it("keeps the graph folded away until somebody asks for it", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    // Folded, and unbuilt: the drawing is the heaviest thing this page can
    // load, and a reader who never opens it never pays for it.
    const toggle = await screen.findByRole("button", { name: /neighborhood/i });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByTestId("canvas")).toBeNull();
  });

  it("puts the two Explore rows in the rail, with the old names", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    const explore = await screen.findByRole("region", { name: "Explore" });
    // The rail holds the switches and the panels stay under the prose.
    expect(
      within(explore).getByRole("button", { name: "Show the neighborhood" }),
    ).toHaveAttribute("aria-expanded", "false");
    expect(
      within(explore).getByRole("button", {
        name: "Show what an agent is taught",
      }),
    ).toHaveAttribute("aria-controls", "engram-agents-eye-panel");
    expect(screen.queryByRole("region", { name: /agent/i })).toBeNull();
  });

  it("opens the Explore rows from the keyboard and hides them again", async () => {
    serve();

    renderApp("/d/eng/e/alpha");
    const row = await screen.findByRole("button", {
      name: "Show what an agent is taught",
    });

    row.focus();
    await userEvent.keyboard("{Enter}");
    expect(
      await screen.findByRole("region", { name: /agent/i }),
    ).toHaveTextContent("Route here for eng questions.");
    expect(
      screen.getByRole("button", { name: "Hide what an agent is taught" }),
    ).toHaveAttribute("aria-expanded", "true");

    await userEvent.keyboard(" ");
    expect(screen.queryByRole("region", { name: /agent/i })).toBeNull();
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
    // And the panels themselves are drawn on that first paint.
    expect(await screen.findByRole("region", { name: /agent/i })).toBeVisible();
    expect(await screen.findByTestId("canvas")).toBeVisible();
  });

  it("keeps each Explore row's target in the page while its panel is closed", async () => {
    serve();

    renderApp("/d/eng/e/alpha");
    const row = await screen.findByRole("button", {
      name: "Show the neighborhood",
    });

    const target = document.getElementById(
      row.getAttribute("aria-controls") ?? "",
    );
    expect(target).not.toBeNull();
    expect(target).toBeEmptyDOMElement();
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

/**
 * The same screen with the frame's full width turned on.
 *
 * What the details column holds is metadata, and a reader who asked for the
 * whole window asked for the prose instead of it. Two things in there are not
 * metadata though - the crystalline address, which nothing else on this screen
 * copies, and the files the engram carries, which a writer removes from that
 * list and nowhere else - so those two come with, and that is what is pinned
 * here beside the column's absence.
 */
describe("the engram page at full width", () => {
  /** The body, plus a file, so the attachments list has something to draw. */
  const WITH_FILE = BODY.replace(
    "Body prose",
    "The [deck](assets/deck.pdf) says more. Body prose",
  );

  function serveWithFile() {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ content: WITH_FILE }),
      "/domains/eng/attachments": () => ({
        attachments: [
          {
            path: "assets/deck.pdf",
            mime: "application/pdf",
            size: 2048,
            modified: "2026-08-18T10:00:00Z",
            sha256: "abc",
          },
        ],
      }),
    });
  }

  it("drops the column and keeps what only the column carried", async () => {
    localStorage.setItem("fluid.layout.width", "full");
    serveWithFile();

    renderApp("/d/eng/e/alpha");
    const main = await screen.findByRole("main");
    await screen.findByRole("heading", { name: "Alpha" });

    expect(screen.queryByRole("region", { name: "Details" })).toBeNull();
    expect(screen.queryByRole("region", { name: "Linked from" })).toBeNull();
    // One column at every width, so the body has the whole of it.
    expect(main.querySelector('[class*="lg:grid-cols-"]')).toBeNull();

    // The address control moves into the header strip beside the other
    // utilities rather than disappearing with the panel that held it: "Share
    // link" beside it copies the browser's URL, which is a different string.
    const copy = screen.getByRole("button", {
      name: "Copy crystalline:// address",
    });
    expect(copy.closest("header")).not.toBeNull();
    // And the files the engram carries stand under the body, where the graph
    // and the agent's eye already stand.
    expect(
      await screen.findByRole("region", { name: "Attachments" }),
    ).toBeVisible();
    // The Explore rows have no rail to stand in, so they stand in the column.
    const explore = screen.getByRole("region", { name: "Explore" });
    expect(explore.closest("aside")).toBeNull();
    expect(
      within(explore).getByRole("button", { name: "Show the neighborhood" }),
    ).toBeVisible();
  });

  // A draft standing where the team's page stands must never be read as that
  // page: this screen is where most readers land, and the only one a grantee
  // with no write right can reach at all.
  it("says whose draft a granted one is", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ draft: true, draft_owner: "alice" }),
    });

    renderApp("/d/eng/e/alpha");

    const marker = await screen.findByRole("status", { name: /draft/i });
    expect(marker).toHaveTextContent("alice's draft, shared with you");
    expect(marker).toHaveTextContent(/not been reviewed/i);
  });

  it("says that your own draft has not moved the shared tree", async () => {
    serve({
      "/domains/eng/engrams/alpha": () => detailResponse({ draft: true }),
    });

    renderApp("/d/eng/e/alpha");

    const marker = await screen.findByRole("status", { name: /draft/i });
    expect(marker).toHaveTextContent("Your private draft");
    expect(marker).toHaveTextContent(/share it for review/i);
  });

  it("marks nothing on a page the team already holds", async () => {
    serve();

    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha" });

    expect(screen.queryByRole("status", { name: /draft/i })).toBeNull();
  });

  it("keeps the column at the reading measure", async () => {
    serveWithFile();

    renderApp("/d/eng/e/alpha");
    const main = await screen.findByRole("main");

    expect(
      await screen.findByRole("region", { name: "Linked from" }),
    ).toBeVisible();
    expect(main.querySelector('[class*="lg:grid-cols-"]')).not.toBeNull();
    // Exactly one of it either way: the panel's copy is the only copy here.
    expect(
      screen.getAllByRole("button", { name: "Copy crystalline:// address" }),
    ).toHaveLength(1);
  });

  /**
   * Section anchors on the page they exist for.
   *
   * The renderer's own test pins the slug rule and the control; what is held
   * here is the part only this screen knows: which base the copied URL is
   * built on. It is the address the server spelled for this caller, so the
   * link a reader hands over and the link an agent hands over are the same
   * string, and the browser's own origin only where the server could not say.
   */
  describe("the section a URL names", () => {
    const SECTIONED = [
      "---",
      "title: Alpha",
      "---",
      "",
      "## Auth",
      "",
      "Prose.",
      "",
    ].join("\n");
    const PAGE = "https://kb.example.com/d/eng/e/alpha";

    function serveSectioned(overrides: Record<string, unknown> = {}) {
      serve({
        "/domains/eng/engrams/alpha": () =>
          detailResponse({ content: SECTIONED, ...overrides }),
      });
    }

    function clipboard() {
      const writeText = vi.fn<(text: string) => Promise<void>>(() =>
        Promise.resolve(),
      );
      Object.defineProperty(navigator, "clipboard", {
        value: { writeText },
        configurable: true,
      });
      return writeText;
    }

    it("arrives at the section a handed url names", async () => {
      const scrolled = vi
        .spyOn(Element.prototype, "scrollIntoView")
        .mockImplementation(() => undefined);
      try {
        serveSectioned({ web_url: PAGE });

        renderApp("/d/eng/e/alpha#auth");
        await screen.findByRole("heading", { name: "Alpha" });

        await waitFor(() => {
          expect(document.getElementById("auth")).not.toBeNull();
        });
        const section = document.getElementById("auth");
        await waitFor(() => {
          expect(scrolled.mock.instances).toContain(section);
        });
        expect(section).toHaveClass("section-flash");
      } finally {
        scrolled.mockRestore();
      }
    });

    it("copies the section url built from the server's web_url", async () => {
      const writeText = clipboard();
      serveSectioned({ web_url: PAGE });

      renderApp("/d/eng/e/alpha");

      await userEvent.click(
        await screen.findByRole("button", { name: "Link to this section" }),
      );

      expect(writeText).toHaveBeenCalledWith(`${PAGE}#auth`);
    });

    it("falls back to the browser's origin when the server gave no web_url", async () => {
      const writeText = clipboard();
      serveSectioned();

      renderApp("/d/eng/e/alpha");

      await userEvent.click(
        await screen.findByRole("button", { name: "Link to this section" }),
      );

      expect(writeText).toHaveBeenCalledWith(
        `${window.location.origin}/d/eng/e/alpha#auth`,
      );
    });

    it("share link copies the server's web_url when there is one", async () => {
      const writeText = clipboard();
      serveSectioned({ web_url: PAGE });

      renderApp("/d/eng/e/alpha");

      await userEvent.click(
        await screen.findByRole("button", { name: "More actions" }),
      );
      await userEvent.click(
        await screen.findByRole("menuitem", { name: "Share link" }),
      );

      expect(writeText).toHaveBeenCalledWith(PAGE);
    });

    it("draws the section links at full width too", async () => {
      const writeText = clipboard();
      localStorage.setItem(LAYOUT_WIDTH_KEY, "full");
      serveSectioned({ web_url: PAGE });

      renderApp("/d/eng/e/alpha");

      await userEvent.click(
        await screen.findByRole("button", { name: "Link to this section" }),
      );

      expect(writeText).toHaveBeenCalledWith(`${PAGE}#auth`);
    });
  });
});

/**
 * What this copy of the page holds that the team's copy does not.
 *
 * The server computes it and the page draws it: a chip beside the title
 * carrying the one word for it, a menu on that chip for looking, sharing and
 * putting it back, and nothing at all on a page the team already has. The
 * word is never re-derived here, which is why every case below states it in
 * the payload rather than in the fixture's shape.
 */
describe("what changed on this page", () => {
  it("draws no chip on a page the team already has", async () => {
    serve();

    renderApp("/d/eng/e/alpha");

    await screen.findByRole("heading", { name: "Alpha", level: 1 });
    expect(screen.queryByRole("button", { name: "Changed" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Added" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Draft" })).toBeNull();
  });

  it.each([
    ["modified", "Changed"],
    ["added", "Added"],
  ])(
    "draws the %s chip as a named menu button beside the title",
    async (word, label) => {
      serve({
        "/domains/eng/engrams/alpha": () =>
          detailResponse({ local_change: word }),
      });

      renderApp("/d/eng/e/alpha");

      const chip = await screen.findByRole("button", { name: label });
      expect(chip).toHaveAttribute("aria-haspopup", "menu");
      // Beside the title rather than inside it: the heading's own name is the
      // engram's name and nothing else, which is what every other reader of
      // this page - the smoke included - knows it by.
      const title = screen.getByRole("heading", { name: "Alpha", level: 1 });
      expect(title).toHaveTextContent("Alpha");
      expect(title.textContent).not.toContain(label);

      await userEvent.click(chip);
      expect(
        await screen.findByRole("menuitem", { name: "What changed" }),
      ).toBeVisible();
      // An editor on an instance-credential install: may write, may not share.
      expect(
        screen.queryByRole("menuitem", { name: "Share this change" }),
      ).toBeNull();
      expect(screen.getByRole("menuitem", { name: "Discard" })).toBeVisible();
    },
  );

  it("draws Draft for the reader's own draft", async () => {
    serve({
      "/domains/eng/engrams/alpha": () => detailResponse({ draft: true }),
    });

    renderApp("/d/eng/e/alpha");

    expect(await screen.findByRole("button", { name: "Draft" })).toBeVisible();
  });

  it("draws no chip on a draft somebody shared with the reader", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ draft: true, draft_owner: "vera" }),
    });

    renderApp("/d/eng/e/alpha");

    // The banner says whose work this is; the chip would offer to look at,
    // share and throw away an overlay this reader does not hold - a fetch of
    // nothing, a share of somebody else's work, and a refused discard.
    const marker = await screen.findByRole("status", { name: /draft/i });
    expect(marker).toHaveTextContent("vera's draft, shared with you");
    expect(screen.queryByRole("button", { name: "Draft" })).toBeNull();

    await userEvent.keyboard("{Meta>}k{/Meta}");
    expect(screen.queryByText("What changed on this page")).toBeNull();
  });

  it("offers Share this change only to a sharer, and Discard only to a writer", async () => {
    serve({
      "/auth/me": () =>
        meResponse({ user: userFixture({ role: "editor" }), can_share: true }),
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ local_change: "modified" }),
    });

    renderApp("/d/eng/e/alpha");

    await userEvent.click(
      await screen.findByRole("button", { name: "Changed" }),
    );
    expect(
      await screen.findByRole("menuitem", { name: "Share this change" }),
    ).toBeVisible();

    cleanup();
    apiMock.mockReset();
    serve({
      "/auth/me": () =>
        meResponse({ user: userFixture({ role: "admin" }), read_only: true }),
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ local_change: "modified" }),
    });

    renderApp("/d/eng/e/alpha");

    await userEvent.click(
      await screen.findByRole("button", { name: "Changed" }),
    );
    // Looking is everybody's; putting a file back is not offered where it
    // would be refused.
    expect(
      await screen.findByRole("menuitem", { name: "What changed" }),
    ).toBeVisible();
    expect(screen.queryByRole("menuitem", { name: "Discard" })).toBeNull();
  });

  describe("on a domain GitHub governs", () => {
    /** A viewer's session, with `row` folded into the listing's one domain. */
    function serveViewer(row: Record<string, unknown>) {
      serve({
        "/auth/me": () => meResponse({ user: userFixture({ role: "viewer" }) }),
        "/domains": () => ({
          ...domainsResponse(),
          domains: [{ ...domainsResponse().domains[0], ...row }],
        }),
        "/domains/eng/engrams/alpha": () =>
          detailResponse({ local_change: "modified" }),
      });
    }

    it("offers an instance viewer with GitHub write Edit and Share this change", async () => {
      serveViewer({ may_share: true });
      renderApp("/d/eng/e/alpha");
      expect(await screen.findByRole("link", { name: "Edit" })).toBeVisible();
      await userEvent.click(
        await screen.findByRole("button", { name: "Changed" }),
      );
      expect(
        await screen.findByRole("menuitem", { name: "Share this change" }),
      ).toBeVisible();
      expect(screen.getByRole("menuitem", { name: "Discard" })).toBeVisible();
    });

    it("offers a GitHub reader neither Edit nor Share this change", async () => {
      serveViewer({ may_share: false, read_only: true });
      renderApp("/d/eng/e/alpha");
      await userEvent.click(
        await screen.findByRole("button", { name: "Changed" }),
      );
      expect(
        await screen.findByRole("menuitem", { name: "What changed" }),
      ).toBeVisible();
      expect(
        screen.queryByRole("menuitem", { name: "Share this change" }),
      ).toBeNull();
      expect(screen.queryByRole("menuitem", { name: "Discard" })).toBeNull();
      expect(screen.queryByRole("link", { name: "Edit" })).toBeNull();
    });
  });

  it("opens the diff in a dialog titled by the path", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ local_change: "modified" }),
      "/domains/eng/changes/alpha.md": () => ({
        path: "alpha.md",
        kind: "modified",
        sha: "9f2c",
        binary: false,
        size_before: 8,
        size_after: 8,
        base: "old\n",
        current: "new\n",
        too_large: false,
      }),
    });

    renderApp("/d/eng/e/alpha");

    await userEvent.click(
      await screen.findByRole("button", { name: "Changed" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "What changed" }),
    );

    const dialog = await screen.findByRole("dialog", { name: "alpha.md" });
    expect(
      await within(dialog).findByRole("region", { name: "alpha.md" }),
    ).toBeInTheDocument();

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Close" }),
    );
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
  });

  it("opens the share dialog with only this path ticked", async () => {
    serve({
      "/auth/me": () => meResponse({ user: userFixture({ role: "admin" }) }),
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ local_change: "modified" }),
      "/domains/eng/sync": () => ({
        domain: "eng",
        mode: "github",
        repo: "acme/kb",
        branch: "main",
        local_changes: 2,
        open_proposals: [],
        declined_proposals: [],
        conflicts: [],
        connection: { connected: true },
      }),
      "/domains/eng/sync/changes": () => ({
        action: "create",
        effective_title: "t",
        changes: [
          { path: "alpha.md", kind: "modified", sha: "9f2c" },
          { path: "beta.md", kind: "added", sha: "51ab" },
        ],
      }),
    });

    renderApp("/d/eng/e/alpha");

    await userEvent.click(
      await screen.findByRole("button", { name: "Changed" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Share this change" }),
    );

    // The whole delta is still in front of the reader; the choice this page
    // opened with is the one file it is about.
    const dialog = await screen.findByRole("dialog", { name: /share/i });
    expect(
      await within(dialog).findByRole("checkbox", { name: "alpha.md" }),
    ).toBeChecked();
    expect(
      within(dialog).getByRole("checkbox", { name: "beta.md" }),
    ).not.toBeChecked();
  });

  it("discarding a modification reloads the page without the chip", async () => {
    let local: string | undefined = "modified";
    const discarded = vi.fn(() => ({
      domain: "eng",
      restored: ["alpha.md"],
      deleted: [],
      cleared: [],
      refused: [],
      reindexed: 1,
    }));
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse(local === undefined ? {} : { local_change: local }),
      "/domains/eng/changes/discard": (_path, init) => {
        if (init?.method === "POST") {
          local = undefined;
          return discarded();
        }
        return null;
      },
    });

    renderApp("/d/eng/e/alpha");

    await userEvent.click(
      await screen.findByRole("button", { name: "Changed" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Discard" }),
    );
    expect(
      await screen.findByText(
        "Discard the changes to alpha.md? Their changes are put back the way the team has them.",
      ),
    ).toBeVisible();

    await userEvent.click(
      screen.getByRole("button", { name: "Confirm discard" }),
    );
    await waitFor(() => {
      expect(discarded).toHaveBeenCalled();
    });
    // The digest the page is holding, which for a stored read is the file's
    // own: a copy somebody edited since is refused rather than thrown away.
    const call = apiMock.mock.calls.find(
      (entry) => entry[0] === "/domains/eng/changes/discard",
    );
    const body = call?.[1]?.body;
    if (typeof body !== "string") {
      throw new Error("the discard carried no JSON body");
    }
    expect(JSON.parse(body)).toEqual({
      paths: [{ path: "alpha.md", sha: "3f8a1c05e2" }],
    });

    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Changed" })).toBeNull();
    });
    expect(
      screen.getByRole("heading", { name: "Alpha", level: 1 }),
    ).toBeVisible();
  });

  it("puts the keyboard back on the chip when the question is dropped", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ local_change: "modified" }),
    });

    renderApp("/d/eng/e/alpha");

    const chip = await screen.findByRole("button", { name: "Changed" });
    await userEvent.click(chip);
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Discard" }),
    );
    await userEvent.click(
      await screen.findByRole("button", { name: "Cancel discard" }),
    );

    await waitFor(() => {
      expect(
        screen.queryByRole("button", { name: "Confirm discard" }),
      ).toBeNull();
    });
    expect(screen.getByRole("button", { name: "Changed" })).toHaveFocus();
  });

  it("discarding an addition goes to the domain page and says so", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ local_change: "added" }),
      "/domains/eng/changes/discard": (_path, init) =>
        init?.method === "POST"
          ? {
              domain: "eng",
              restored: [],
              deleted: ["alpha.md"],
              cleared: [],
              refused: [],
              reindexed: 1,
            }
          : null,
      "/domains/eng/manifest": () => ({ domain: "eng", markdown: "# eng\n" }),
      "/domains/eng/engrams": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
      "/vocabulary": () => ({
        domain: "eng",
        tags: [],
        categories: [],
        relation_types: [],
      }),
    });

    renderApp("/d/eng/e/alpha");

    await userEvent.click(await screen.findByRole("button", { name: "Added" }));
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Discard" }),
    );
    await userEvent.click(
      await screen.findByRole("button", { name: "Confirm discard" }),
    );

    // The page it was standing on is gone, so it lands where the file was and
    // the landing says what happened to it.
    expect(
      await screen.findByRole("heading", { name: "eng", level: 1 }),
    ).toBeVisible();
    expect(await screen.findByText("Discarded alpha.md.")).toBeVisible();
  });

  it("discarding an addition lands on the engram view when the domain opens on its Reflection", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ local_change: "added" }),
      "/domains/eng/changes/discard": (_path, init) =>
        init?.method === "POST"
          ? {
              domain: "eng",
              restored: [],
              deleted: ["alpha.md"],
              cleared: [],
              refused: [],
              reindexed: 1,
            }
          : null,
      "/domains/eng/reflection": () => ({
        domain: "eng",
        state: "on",
        home: "local",
        home_url: null,
        right: "curate",
        engrams_visible: true,
        pending_engrams: [],
      }),
      "/domains/eng/manifest": () => ({ domain: "eng", markdown: "# eng\n" }),
      "/domains/eng/engrams": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
      "/vocabulary": () => ({
        domain: "eng",
        tags: [],
        categories: [],
        relation_types: [],
      }),
    });

    renderApp("/d/eng/e/alpha");

    await userEvent.click(await screen.findByRole("button", { name: "Added" }));
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Discard" }),
    );
    await userEvent.click(
      await screen.findByRole("button", { name: "Confirm discard" }),
    );

    // The page it was standing on is gone, so it lands where the file was and
    // the landing says what happened to it.
    expect(
      await screen.findByRole("heading", { name: "eng", level: 1 }),
    ).toBeVisible();
    expect(await screen.findByText("Discarded alpha.md.")).toBeVisible();
  });

  it("discarding a draft of a page the team never had leaves the same way", async () => {
    serve({
      "/domains/eng/engrams/alpha": () => detailResponse({ draft: true }),
      // The reviewing arm clears an overlay row rather than deleting a file,
      // and names the kind the shared tree would have called it: "added" for
      // a draft standing where the team has no page at all.
      "/domains/eng/changes/discard": (_path, init) =>
        init?.method === "POST"
          ? {
              domain: "eng",
              restored: [],
              deleted: [],
              cleared: [{ path: "alpha.md", kind: "added" }],
              refused: [],
              reindexed: 0,
            }
          : null,
      "/domains/eng/manifest": () => ({ domain: "eng", markdown: "# eng\n" }),
      "/domains/eng/engrams": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
      "/vocabulary": () => ({
        domain: "eng",
        tags: [],
        categories: [],
        relation_types: [],
      }),
    });

    renderApp("/d/eng/e/alpha");

    await userEvent.click(await screen.findByRole("button", { name: "Draft" }));
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Discard" }),
    );
    await userEvent.click(
      await screen.findByRole("button", { name: "Confirm discard" }),
    );

    expect(
      await screen.findByRole("heading", { name: "eng", level: 1 }),
    ).toBeVisible();
    expect(await screen.findByText("Discarded alpha.md.")).toBeVisible();
  });

  it("shows a refusal in the strip and refetches the page", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ local_change: "modified" }),
      "/domains/eng/changes/discard": (_path, init) =>
        init?.method === "POST"
          ? {
              domain: "eng",
              restored: [],
              deleted: [],
              cleared: [],
              refused: [{ path: "alpha.md", reason: "changed_since" }],
              reindexed: 0,
            }
          : null,
    });

    renderApp("/d/eng/e/alpha");

    await userEvent.click(
      await screen.findByRole("button", { name: "Changed" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Discard" }),
    );
    await userEvent.click(
      await screen.findByRole("button", { name: "Confirm discard" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Changed since you looked.",
    );
    await waitFor(() => {
      expect(
        apiMock.mock.calls.filter(
          (entry) => entry[0] === "/domains/eng/engrams/alpha",
        ).length,
      ).toBeGreaterThan(1);
    });
  });

  it("lists What changed in the palette when the chip is shown", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ local_change: "modified" }),
    });

    renderApp("/d/eng/e/alpha");

    await screen.findByRole("button", { name: "Changed" });
    await userEvent.keyboard("{Meta>}k{/Meta}");

    // Looking is a palette row; a discard is not. A destructive act belongs
    // where it cannot be typed into by accident.
    expect(await screen.findByText("What changed on this page")).toBeVisible();
    expect(screen.queryByText("Discard this change")).toBeNull();
  });
});

describe("a domain's MANIFEST on the engram page", () => {
  /** The same engram, at `path`, on a domain of the given kind. */
  function serveAt(path: string, kind: "file" | "virtual") {
    serve({
      "/domains": () => ({
        ...domainsResponse(),
        domains: [{ ...domainsResponse().domains[0], kind }],
      }),
      "/domains/eng/engrams/alpha": () => detailResponse({ path }),
    });
  }

  it("the MANIFEST page offers no move, no permanent delete and no retire", async () => {
    serveAt("MANIFEST.md", "file");
    renderApp("/d/eng/e/alpha");
    await screen.findByRole("link", { name: "Edit" });
    // The menu holds the utilities only: no Move, no Retire.
    await userEvent.click(screen.getByRole("button", { name: "More actions" }));
    expect(
      await screen.findByRole("menuitem", { name: "Share link" }),
    ).toBeVisible();
    expect(screen.queryByRole("menuitem", { name: "Move" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: "Retire" })).toBeNull();
    await userEvent.keyboard("{Escape}");
    await userEvent.keyboard("{Meta>}k{/Meta}");
    expect(await screen.findByText("Edit engram")).toBeVisible();
    expect(screen.queryByText("Move engram")).toBeNull();
    expect(screen.queryByText("Retire engram")).toBeNull();
  });

  it("a virtual domain's MANIFEST keeps move and delete", async () => {
    serveAt("MANIFEST.md", "virtual");
    renderApp("/d/eng/e/alpha");
    await userEvent.click(
      await screen.findByRole("button", { name: "More actions" }),
    );
    expect(await screen.findByRole("menuitem", { name: "Move" })).toBeVisible();
    await userEvent.click(screen.getByRole("menuitem", { name: "Retire" }));
    const dialog = await screen.findByRole("dialog", { name: /retire/i });
    expect(
      within(dialog).getByRole("button", {
        name: "Delete permanently instead",
      }),
    ).toBeVisible();
    await userEvent.keyboard("{Escape}");
    await userEvent.keyboard("{Meta>}k{/Meta}");
    expect(await screen.findByText("Move engram")).toBeVisible();
    expect(screen.getByText("Retire engram")).toBeVisible();
  });

  it("an engram called manifest in a folder keeps move and delete", async () => {
    serveAt("notes/manifest.md", "file");
    renderApp("/d/eng/e/alpha");
    await userEvent.click(
      await screen.findByRole("button", { name: "More actions" }),
    );
    expect(await screen.findByRole("menuitem", { name: "Move" })).toBeVisible();
    await userEvent.click(screen.getByRole("menuitem", { name: "Retire" }));
    const dialog = await screen.findByRole("dialog", { name: /retire/i });
    expect(
      within(dialog).getByRole("button", {
        name: "Delete permanently instead",
      }),
    ).toBeVisible();
    await userEvent.keyboard("{Escape}");
    await userEvent.keyboard("{Meta>}k{/Meta}");
    expect(await screen.findByText("Move engram")).toBeVisible();
    expect(screen.getByText("Retire engram")).toBeVisible();
  });
});

describe("the reading page follows the stream", () => {
  let scrollTo: ReturnType<typeof vi.fn>;
  beforeEach(() => {
    FakeEventSource.instances = [];
    vi.stubGlobal("EventSource", FakeEventSource);
    // jsdom lays nothing out and implements no scrolling; the keeper's own
    // file pins where it scrolls to, this one only that the page runs it.
    scrollTo = vi.fn();
    vi.stubGlobal("scrollTo", scrollTo);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  /** The one stream the shell opened, the last one if it opened again. */
  function stream(): FakeEventSource {
    const source = FakeEventSource.instances.at(-1);
    if (!source) throw new Error("the shell opened no stream");
    return source;
  }

  /**
   * The browser having connected the shell's stream: a real `EventSource`
   * fires `open` before any message, and the leading tab asks the probe
   * who the stream is for before it passes a frame on.
   */
  async function connected(): Promise<void> {
    const source = stream();
    if (!source.opened) {
      act(() => {
        source.open();
      });
    }
    await act(async () => {
      for (let i = 0; i < 20; i++) await Promise.resolve();
    });
  }

  /** The line's live region, which stays mounted and is empty between changes. */
  function line(): HTMLElement {
    return screen.getByRole("status", { name: "Recent change" });
  }

  /** What the router says happened last, which is where a replace shows. */
  function Probe() {
    const how = useNavigationType();
    const { pathname } = useLocation();
    return <output data-testid="navigation">{`${how} ${pathname}`}</output>;
  }

  const frame = (overrides: Record<string, unknown> = {}) => ({
    domain: "eng",
    permalink: "alpha",
    path: "alpha.md",
    kind: "modified",
    from: null,
    checksum: "new1",
    actor: "ada",
    draft_of: null,
    ...overrides,
  });

  it("refetches in place and says who changed it, for a minute", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let served = detailResponse();
    serve({ "/domains/eng/engrams/alpha": () => served });
    renderApp(engramRoute("eng", "alpha"));
    await screen.findByRole("heading", { name: "Alpha" });
    // The region is there before anything changed, and says nothing.
    const region = line();
    expect(region.textContent).toBe("");
    served = detailResponse({
      content: BODY.replace("Body prose", "Revised prose"),
      checksum: "new1",
    });
    await connected();
    act(() => {
      stream().emit("engram", frame(), "1:1");
    });
    await screen.findByText(/Revised prose/);
    expect(line()).toHaveTextContent("Updated a moment ago by ada");
    // The same node the reader's screen reader was already listening to.
    expect(line()).toBe(region);
    // The page ran its scroll keeper over the refetch.
    expect(scrollTo).toHaveBeenCalled();
    act(() => {
      stream().emit("engram", frame({ actor: null, checksum: "new2" }), "1:2");
    });
    await waitFor(() => {
      expect(line()).toHaveTextContent(/^Updated a moment ago$/);
    });
    // A newer event resets the minute: most of one passes, the line stays.
    act(() => {
      vi.advanceTimersByTime(59_000);
    });
    expect(line()).toHaveTextContent("Updated a moment ago");
    act(() => {
      vi.advanceTimersByTime(1_000);
    });
    await waitFor(() => {
      expect(line().textContent).toBe("");
    });
    // And a change after the minute brings it back.
    act(() => {
      stream().emit("engram", frame({ checksum: "new3" }), "1:3");
    });
    await waitFor(() => {
      expect(line()).toHaveTextContent("Updated a moment ago by ada");
    });
  });

  it("redraws an attachment image when a files event brings new bytes, and leaves the others", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let a = "1".repeat(64);
    const b = "b".repeat(64);
    const row = (path: string, sha256: string) => ({
      path,
      mime: "image/png",
      size: 10,
      modified: "2026-01-01T00:00:00Z",
      sha256,
    });
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({
          content: `${BODY}\n![First](assets/a.png)\n\n![Second](assets/b.png)\n`,
        }),
      "/domains/eng/attachments": () => ({
        attachments: [row("assets/a.png", a), row("assets/b.png", b)],
      }),
    });
    renderApp(engramRoute("eng", "alpha"));
    const article = await screen.findByRole("article");
    const src = (name: string) =>
      within(article).getByRole("img", { name }).getAttribute("src");
    await waitFor(() => {
      expect(src("First")).toMatch(/assets\/a\.png$/);
    });
    // Let the first listing land: it sets the baseline, no version yet.
    await act(async () => {
      for (let i = 0; i < 20; i++) await Promise.resolve();
    });
    expect(src("First")).toMatch(/assets\/a\.png$/);
    a = "2".repeat(64);
    await connected();
    act(() => {
      stream().emit(
        "engram",
        frame({
          kind: "files",
          permalink: "assets/a.png",
          path: "assets/a.png",
          checksum: a,
        }),
        "1:1",
      );
    });
    await waitFor(() => {
      expect(src("First")).toMatch(/assets\/a\.png\?v=222222222222$/);
    });
    expect(src("Second")).toMatch(/assets\/b\.png$/);
  });

  it("keeps the drawn body mounted when the listing lands and when an event about another file refetches it", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let listed = 0;
    // The first listing waits, so the image is drawn before it lands.
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({ content: `${BODY}\n![First](assets/a.png)\n` }),
      "/domains/eng/attachments": async () => {
        await gate;
        listed += 1;
        return {
          attachments: [
            {
              path: "assets/a.png",
              mime: "image/png",
              size: 10,
              modified: "2026-01-01T00:00:00Z",
              sha256: "1".repeat(64),
            },
          ],
        };
      },
    });
    renderApp(engramRoute("eng", "alpha"));
    const article = await screen.findByRole("article");
    const drawn = await within(article).findByRole("img", { name: "First" });
    expect(listed).toBe(0);
    release();
    await waitFor(() => {
      expect(listed).toBeGreaterThan(0);
    });
    await act(async () => {
      for (let i = 0; i < 20; i++) await Promise.resolve();
    });
    // The listing landed: the same image element, not a remounted one.
    expect(within(article).getByRole("img", { name: "First" })).toBe(drawn);
    const before = listed;
    await connected();
    act(() => {
      stream().emit(
        "engram",
        frame({
          kind: "files",
          permalink: "assets/other.png",
          path: "assets/other.png",
          checksum: "9".repeat(64),
        }),
        "1:1",
      );
    });
    await waitFor(() => {
      expect(listed).toBeGreaterThan(before);
    });
    await act(async () => {
      for (let i = 0; i < 20; i++) await Promise.resolve();
    });
    expect(within(article).getByRole("img", { name: "First" })).toBe(drawn);
  });

  it("names the actor the way the details panel does", async () => {
    serve();
    renderApp(engramRoute("eng", "alpha"));
    await screen.findByRole("heading", { name: "Alpha" });
    await connected();
    act(() => {
      stream().emit(
        "engram",
        frame({ actor: "process:crystalline-cli", checksum: "3f8a1c05e2" }),
        "1:1",
      );
    });
    await waitFor(() => {
      expect(line()).toHaveTextContent(
        /^Updated a moment ago by crystalline-cli \(process\)$/,
      );
    });
  });

  it("holds the reader's place when the line comes and when it goes", async () => {
    // A browser without native scroll anchoring moves the text under a
    // reader who scrolled past the header; the page scrolls by what the
    // body moved instead. `top` is where the body's top edge sits.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let top = -500;
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(
      () => ({
        top,
        bottom: top + 20,
        left: 0,
        right: 0,
        width: 0,
        height: 20,
        x: 0,
        y: top,
        toJSON: () => ({}),
      }),
    );
    serve();
    renderApp(engramRoute("eng", "alpha"));
    await screen.findByRole("heading", { name: "Alpha" });
    // The line's height pushes the body down by 60px.
    top = -440;
    await connected();
    act(() => {
      stream().emit("engram", frame({ checksum: "3f8a1c05e2" }), "1:1");
    });
    expect(line()).toHaveTextContent("Updated a moment ago by ada");
    expect(scrollTo).toHaveBeenCalledWith(0, 60);
    scrollTo.mockClear();
    // The page scrolled, so the body is back where the reader had it; then
    // the line goes and takes its height with it.
    top = -500;
    window.dispatchEvent(new Event("scroll"));
    top = -560;
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    await waitFor(() => {
      expect(line().textContent).toBe("");
    });
    expect(scrollTo).toHaveBeenCalledWith(0, -60);
  });

  it("a delete refetches, the server answers 404 and the not-found face appears", async () => {
    let gone = false;
    serve({
      "/domains/eng/engrams/alpha": () => {
        if (gone) throw new ApiProblem(404, "Not Found", "no engram");
        return detailResponse();
      },
    });
    renderApp(engramRoute("eng", "alpha"));
    await screen.findByRole("heading", { name: "Alpha" });
    gone = true;
    await connected();
    act(() => {
      stream().emit(
        "engram",
        frame({ kind: "deleted", checksum: null }),
        "1:1",
      );
    });
    await screen.findByRole("heading", { name: "Engram not found" });
    // The not-found face, and not an error said first.
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("a move follows to the new address, replaces history and says so", async () => {
    serve({
      "/domains/eng/engrams/topics/alpha": () =>
        detailResponse({ permalink: "topics/alpha", path: "topics/alpha.md" }),
      "/domains/eng/inbound/topics/alpha": (path: string) =>
        inboundResponse(path),
    });
    render(
      <MemoryRouter
        initialEntries={["/d/eng", engramRoute("eng", "alpha")]}
        initialIndex={1}
      >
        <App />
        <Probe />
      </MemoryRouter>,
    );
    await screen.findByRole("heading", { name: "Alpha" });
    await connected();
    const region = line();
    // Everything the line ever said on the way: the old address must not
    // say "Updated" for the commit before it follows the move.
    const said: string[] = [];
    // Read off the mutation records rather than the live node: by the time
    // an observer runs, the follow may already have replaced the text.
    const inRegion = (node: Node) =>
      (node instanceof Element ? node : node.parentElement)?.closest(
        '[aria-label="Recent change"]',
      ) != null;
    const observer = new MutationObserver((records) => {
      for (const record of records) {
        for (const node of record.addedNodes) {
          // A text node written into the region, or a region mounted anew
          // (the line is keyed on the address, so a follow mounts one).
          const regions =
            node instanceof Element
              ? [
                  ...(node.matches('[aria-label="Recent change"]')
                    ? [node]
                    : []),
                  ...node.querySelectorAll('[aria-label="Recent change"]'),
                ]
              : inRegion(record.target)
                ? [node]
                : [];
          for (const region of regions) {
            if (region.textContent) said.push(region.textContent);
          }
        }
        if (inRegion(record.target) && record.oldValue) {
          said.push(record.oldValue);
        }
      }
    });
    observer.observe(document.body, {
      childList: true,
      subtree: true,
      characterData: true,
      characterDataOldValue: true,
    });
    act(() => {
      stream().emit(
        "engram",
        frame({
          permalink: "topics/alpha",
          path: "topics/alpha.md",
          kind: "moved",
          from: { path: "alpha.md", permalink: "alpha" },
        }),
        "1:1",
      );
    });
    await waitFor(() => {
      expect(line()).toHaveTextContent("Moved here a moment ago by ada");
    });
    observer.disconnect();
    // The region the screen reader already knew, announcing the move: not a
    // new one mounted with its text already in it.
    expect(line()).toBe(region);
    expect(said.length).toBeGreaterThan(0);
    expect(said.every((text) => text.startsWith("Moved here"))).toBe(true);
    expect(screen.getByTestId("navigation")).toHaveTextContent(
      `REPLACE ${engramRoute("eng", "topics/alpha")}`,
    );
    // The new address's detail replaces the placeholder the follow kept
    // (the old address's text) when its own answer lands.
    expect(
      await within(
        screen.getByRole("navigation", { name: "Breadcrumb" }),
      ).findByText("topics"),
    ).toBeInTheDocument();
  });

  describe("keeps the old text on screen only while following a move", () => {
    let go: NavigateFunction = () => undefined;
    function Go() {
      go = useNavigate();
      return null;
    }

    /** The reading page of alpha, with beta's detail never answering. */
    async function onAlphaWithBetaPending() {
      serve({
        "/domains/eng/engrams/beta": () => new Promise<never>(() => undefined),
      });
      render(
        <MemoryRouter initialEntries={[engramRoute("eng", "alpha")]}>
          <App />
          <Go />
        </MemoryRouter>,
      );
      await screen.findByRole("heading", { name: "Alpha" });
      await waitFor(() => {
        expect(document.body.textContent).toContain("Body prose");
      });
    }

    it("a plain link to another engram shows its loading state, never the page it left", async () => {
      // Catches the placeholder handed to any address change: beta's page
      // would stand there with alpha's text under beta's address.
      await onAlphaWithBetaPending();
      act(() => {
        void go(engramRoute("eng", "beta"));
      });
      expect(
        await screen.findByRole("status", { name: "Loading the engram" }),
      ).toBeInTheDocument();
      expect(document.body.textContent).not.toContain("Body prose");
      expect(screen.queryByRole("heading", { name: "Alpha" })).toBeNull();
    });

    it("a follow that names another old address does not borrow this one's text", async () => {
      // Catches the placeholder keyed on "any follow" rather than on the
      // address the follow came from.
      await onAlphaWithBetaPending();
      act(() => {
        void go(engramRoute("eng", "beta"), {
          replace: true,
          state: { followedFrom: "gamma" },
        });
      });
      expect(
        await screen.findByRole("status", { name: "Loading the engram" }),
      ).toBeInTheDocument();
      expect(document.body.textContent).not.toContain("Body prose");
    });
  });

  it("says the reader's own move once, with the dialog's counts", async () => {
    // The dialog lands here with what it rewrote; the stream's frame for the
    // same move follows it and must not add a second line saying the same.
    serve({
      "/domains/eng/engrams/topics/alpha": () =>
        detailResponse({ permalink: "topics/alpha", path: "topics/alpha.md" }),
      "/domains/eng/inbound/topics/alpha": (path: string) =>
        inboundResponse(path),
    });
    render(
      <MemoryRouter
        initialEntries={[
          {
            pathname: engramRoute("eng", "topics/alpha"),
            state: { moved: { references: 2, engrams: 1 } },
          },
        ]}
      >
        <App />
      </MemoryRouter>,
    );
    await screen.findByRole("heading", { name: "Alpha" });
    await connected();
    act(() => {
      stream().emit(
        "engram",
        frame({
          permalink: "topics/alpha",
          path: "topics/alpha.md",
          kind: "moved",
          from: { path: "alpha.md", permalink: "alpha" },
        }),
        "1:1",
      );
    });
    expect(screen.getByRole("status", { name: "Moved" })).toHaveTextContent(
      "Rewrote 2 references",
    );
    // Give the frame every chance to have drawn something.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(line().textContent).toBe("");
    // Only that one frame: somebody else's change afterwards has its line.
    act(() => {
      stream().emit(
        "engram",
        frame({
          permalink: "topics/alpha",
          path: "topics/alpha.md",
          actor: "bob",
          checksum: "later1",
        }),
        "1:2",
      );
    });
    await waitFor(() => {
      expect(line()).toHaveTextContent("Updated a moment ago by bob");
    });
  });

  it("never flashes a change whose minute passed before the reader got there", async () => {
    // The page and its region stay mounted across a link to another engram,
    // so the new page's old change must be settled before it paints.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    serve({
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
          {
            permalink: "gamma",
            title: "Gamma",
            type: "engram",
            path: "gamma.md",
          },
        ],
      }),
      "/domains/eng/engrams/gamma": () =>
        detailResponse({
          permalink: "gamma",
          title: "Gamma",
          path: "gamma.md",
          content: "# Gamma\n\nOther prose.\n",
        }),
      "/domains/eng/inbound/gamma": (path: string) => inboundResponse(path),
    });
    // Gamma first, so its page is in the cache and the way back to it
    // renders at once, with no skeleton in between to remount the line.
    renderApp(engramRoute("eng", "gamma"));
    await screen.findByText(/Other prose/);
    await userEvent.click(
      within(
        screen.getByRole("navigation", { name: /engrams|domain/i }),
      ).getByRole("link", { name: "Alpha" }),
    );
    await screen.findByRole("heading", { name: "Alpha" });
    await connected();
    act(() => {
      stream().emit(
        "engram",
        frame({ permalink: "gamma", path: "gamma.md", checksum: "g1" }),
        "1:1",
      );
    });
    act(() => {
      vi.advanceTimersByTime(61_000);
    });
    const said: string[] = [];
    // Read off the records: a flash that was cleared again before the
    // observer ran still left its text in them.
    const observer = new MutationObserver((records) => {
      for (const record of records) {
        const inRegion =
          (record.target instanceof Element
            ? record.target
            : record.target.parentElement
          )?.closest('[aria-label="Recent change"]') != null;
        if (!inRegion) continue;
        for (const node of record.addedNodes) {
          if (node.textContent) said.push(node.textContent);
        }
        if (record.oldValue) said.push(record.oldValue);
      }
    });
    observer.observe(document.body, {
      childList: true,
      subtree: true,
      characterData: true,
      characterDataOldValue: true,
    });
    await userEvent.click(
      within(
        screen.getByRole("navigation", { name: /engrams|domain/i }),
      ).getByRole("link", { name: "Gamma" }),
    );
    await screen.findByText(/Other prose/);
    observer.disconnect();
    expect(said).toEqual([]);
    expect(line().textContent).toBe("");
  });
});

describe("the printed engram", () => {
  it("prints a title block with the facts the rail holds on screen", async () => {
    serve();
    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha", level: 1 });
    const block = document.querySelector<HTMLElement>("header.print-title");
    expect(block).not.toBeNull();
    // On paper only: hidden on screen and from assistive technology there.
    expect(block).toHaveAttribute("hidden");
    expect(block).toHaveAttribute("data-print");
    expect(block).toHaveTextContent("eng");
    expect(block).toHaveTextContent("Alpha");
    expect(block).toHaveTextContent("Statussuperseded");
    expect(block).toHaveTextContent("Typedecision");
    expect(block).toHaveTextContent("Tagsone, two");
    expect(block).toHaveTextContent("crystalline://eng/alpha");
    expect(block).toHaveTextContent(/Printed \w/);
    // The stamp in the corner opens the engram's page, not its name.
    await waitFor(() => {
      expect(
        block?.querySelector("svg[data-qr-text]")?.getAttribute("data-qr-text"),
      ).toBe(`${window.location.origin}/d/eng/e/alpha`);
    });
    // The page margins carry the domain, the title and the address.
    const margins = document.querySelector("style[data-print-page]");
    expect(margins?.textContent).toContain('@top-left { content: "eng"; }');
    expect(margins?.textContent).toContain('@top-right { content: "Alpha"; }');
    expect(margins?.textContent).toContain(
      '@bottom-left { content: "crystalline://eng/alpha"; }',
    );
    expect(margins?.textContent).toMatch(
      /@bottom-right \{ content: "Printed [^"]+ \u00b7 " counter\(page\) " of " counter\(pages\); \}/,
    );
    // The first page keeps its page number alone: the title block says the rest.
    expect(margins?.textContent).toContain(
      '@page :first {\n  @top-left { content: ""; }',
    );
  });

  it("ends with a Related section: what links here and the files, by name and size", async () => {
    serve({
      "/domains/eng/engrams/alpha": () =>
        detailResponse({
          content: `${BODY}\nSee [the deck](assets/deck.pdf) and ![chart](assets/chart.png).\n`,
        }),
      "/domains/eng/attachments": () => ({
        attachments: [
          {
            path: "assets/deck.pdf",
            mime: "application/pdf",
            size: 2048,
            modified: "2026-01-01T00:00:00Z",
          },
        ],
      }),
    });
    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha", level: 1 });
    const related = await waitFor(() => {
      const found = document.querySelector<HTMLElement>(
        'section[aria-label="Related"]',
      );
      expect(found).not.toBeNull();
      return found as HTMLElement;
    });
    expect(related).toHaveAttribute("hidden");
    expect(related).toHaveAttribute("data-print");
    const lists = () => [...related.querySelectorAll("h3")];
    await waitFor(() => {
      expect(lists().map((h) => h.textContent)).toEqual([
        "Linked from",
        "Attachments",
      ]);
    });
    const [linked, files] = lists().map(
      (h) => h.parentElement as HTMLElement,
    ) as [HTMLElement, HTMLElement];
    await waitFor(() => {
      expect(linked).toHaveTextContent("Betalinks_to");
    });
    // A plain line, not a link: links are text on paper.
    expect(linked.querySelector("a")).toBeNull();
    await waitFor(() => {
      expect(files).toHaveTextContent("deck.pdf2.0 KiB");
    });
    expect(files).toHaveTextContent("chart.pngmissing");
  });

  it("keeps the chrome, the header and the rail off the paper", async () => {
    serve();
    renderApp("/d/eng/e/alpha");
    await screen.findByRole("heading", { name: "Alpha", level: 1 });
    for (const bar of screen.getAllByRole("banner")) {
      expect(bar.className).toContain("print:hidden");
    }
    expect(
      screen.getByRole("link", { name: "Edit" }).closest("header")?.className,
    ).toContain("print:hidden");
    expect(
      screen.getByRole("heading", { name: "Linked from" }).closest("aside")
        ?.className,
    ).toContain("print:hidden");
  });
});

describe("the aside beside the body", () => {
  it("draws each panel once: no two siblings share a key", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      serve();
      renderApp("/d/eng/e/alpha");
      await screen.findByRole("heading", { name: "Alpha", level: 1 });
      await waitFor(() => {
        expect(screen.getAllByText("Linked from")).toHaveLength(1);
      });
      const twins = errors.mock.calls.filter((call) =>
        String(call[0]).includes("same key"),
      );
      expect(twins).toEqual([]);
    } finally {
      errors.mockRestore();
    }
  });
});

describe("task boxes on the engram page", () => {
  const TASKS = `${BODY}\n- [ ] ship it\n- [x] write it\n`;

  /** Every PUT the mock has seen, as body and If-Match pairs. */
  function puts(): { content: string; ifMatch: string | undefined }[] {
    return apiMock.mock.calls
      .filter(([, init]) => init?.method === "PUT")
      .map(([, init]) => ({
        content: (JSON.parse(init?.body as string) as { content: string })
          .content,
        ifMatch: (init?.headers as Record<string, string> | undefined)?.[
          "If-Match"
        ],
      }));
  }

  /** How many times the detail was read. */
  function reads(): number {
    return apiMock.mock.calls.filter(
      ([path, init]) =>
        path === "/domains/eng/engrams/alpha" && init?.method !== "PUT",
    ).length;
  }

  it("ticks a box and saves the whole file with the page's checksum", async () => {
    const ticked = TASKS.replace("- [ ] ship it", "- [x] ship it");
    serve({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT"
          ? detailResponse({ content: ticked, checksum: "c2" })
          : detailResponse({ content: TASKS }),
    });
    renderApp("/d/eng/e/alpha");
    const box = await screen.findByRole("checkbox", { name: "ship it" });
    expect(box).toBeEnabled();
    await userEvent.click(box);
    await waitFor(() => {
      expect(puts()).toEqual([{ content: ticked, ifMatch: '"3f8a1c05e2"' }]);
    });
    await waitFor(() => {
      expect(screen.getByRole("checkbox", { name: "ship it" })).toBeChecked();
    });
    // The page shows the frontmatter-free body still: nothing else moved.
    expect(screen.getByRole("checkbox", { name: "write it" })).toBeChecked();
  });

  it("says so and reads the engram again when it changed underneath", async () => {
    serve({
      "/domains/eng/engrams/alpha": (_path, init) => {
        if (init?.method === "PUT") {
          throw new ApiProblem(
            412,
            "precondition failed",
            "stale edit: engram changed since it was read",
            { current_etag: '"srv9"', current_content: TASKS },
          );
        }
        return detailResponse({ content: TASKS });
      },
    });
    renderApp("/d/eng/e/alpha");
    await userEvent.click(
      await screen.findByRole("checkbox", { name: "ship it" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Someone else saved this engram first. Try again.",
    );
    await waitFor(() => {
      expect(reads()).toBeGreaterThanOrEqual(2);
    });
    expect(screen.getByRole("checkbox", { name: "ship it" })).not.toBeChecked();
  });

  it("says so and reads the engram again when the box is not where the page drew it", async () => {
    serve({
      "/domains/eng/engrams/alpha": () => detailResponse({ content: TASKS }),
    });
    renderApp("/d/eng/e/alpha");
    const box = await screen.findByRole("checkbox", { name: "ship it" });
    await waitFor(() => {
      expect(box).toBeEnabled();
    });
    tickControl.refuse = true;
    try {
      await userEvent.click(box);
    } finally {
      tickControl.refuse = false;
    }
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Someone else saved this engram first. Try again.",
    );
    await waitFor(() => {
      expect(reads()).toBeGreaterThanOrEqual(2);
    });
    expect(puts()).toEqual([]);
  });

  it("sends one save for a double click", async () => {
    const ticked = TASKS.replace("- [ ] ship it", "- [x] ship it");
    let land: (value: unknown) => void = () => undefined;
    serve({
      "/domains/eng/engrams/alpha": (_path, init) =>
        init?.method === "PUT"
          ? new Promise((resolve) => {
              land = resolve;
            })
          : detailResponse({ content: TASKS }),
    });
    renderApp("/d/eng/e/alpha");
    const box = await screen.findByRole("checkbox", { name: "ship it" });
    await waitFor(() => {
      expect(box).toBeEnabled();
    });
    await userEvent.dblClick(box);
    await waitFor(() => {
      expect(puts()).toHaveLength(1);
    });
    land(detailResponse({ content: ticked, checksum: "c2" }));
    await waitFor(() => {
      expect(screen.getByRole("checkbox", { name: "ship it" })).toBeChecked();
    });
    expect(puts()).toHaveLength(1);
  });

  it("disables the box with the reason in a domain this reader may not change", async () => {
    serve({
      "/domains": () => ({
        ...domainsResponse(),
        domains: [{ ...domainsResponse().domains[0], read_only: true }],
      }),
      "/domains/eng/engrams/alpha": () => detailResponse({ content: TASKS }),
    });
    renderApp("/d/eng/e/alpha");
    const box = await screen.findByRole("checkbox", { name: "ship it" });
    await waitFor(() => {
      expect(box).toBeDisabled();
    });
    expect(box).toHaveAccessibleDescription(
      "You can read this page but not change it",
    );
  });

  it("disables the box for the anonymous viewer and says why", async () => {
    serve({
      "/auth/me": () => meResponse({ anonymous: true }),
      "/domains/eng/engrams/alpha": () => detailResponse({ content: TASKS }),
    });
    renderApp("/d/eng/e/alpha");
    const box = await screen.findByRole("checkbox", { name: "ship it" });
    await waitFor(() => {
      expect(box).toBeDisabled();
    });
    expect(box).toHaveAccessibleDescription("Sign in to change this page");
    expect(puts()).toEqual([]);
  });
});
