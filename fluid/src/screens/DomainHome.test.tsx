/**
 * One domain: what it is for, what is in it, and the two states that must never
 * look alike - a domain nobody registered, and a registered domain with nothing
 * in it yet. The first is a wrong address and the second is an invitation, so
 * the screen says which one happened rather than showing one empty box for
 * both.
 *
 * The browse view and the filter view are the same paged endpoint asked two
 * different questions - one folder, or one set of frontmatter filters across
 * the whole domain - and the screen names which one is on screen instead of
 * blending them. The tree is what the folder navigation above the list is
 * drawn from, and nothing else.
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

const MANIFEST = [
  "---",
  "title: eng",
  "---",
  "",
  "# eng",
  "",
  "What this domain is for, in one paragraph.",
  "",
  "## When to Use",
  "",
  "- Route here for eng questions.",
  "",
].join("\n");

/** The tree, which answers with the folder that was asked for. */
function treeResponse(path: string) {
  if (path.includes("path=notes")) {
    return {
      domain: "eng",
      path: "notes",
      folders: [],
      engrams: [
        {
          permalink: "notes/beta",
          title: "Beta",
          type: "engram",
          path: "notes/beta.md",
        },
      ],
    };
  }
  return {
    domain: "eng",
    path: "/",
    folders: ["notes"],
    engrams: [
      { permalink: "alpha", title: "Alpha", type: "engram", path: "alpha.md" },
    ],
  };
}

/**
 * The listing, which is what both views of this screen page: the frontmatter
 * one across the whole domain, and the folder one scoped by `path`. The
 * filtered answer carries a retired engram, so the fade has something to do.
 */
function engramsResponse(path: string) {
  if (path.includes("tags=eng")) {
    return {
      mode: "text",
      total: 1,
      page: 1,
      limit: 50,
      count: 1,
      hits: [
        {
          domain: "eng",
          permalink: "gamma",
          title: "Gamma",
          engram_type: "decision",
          kind: "engram",
          status: "deprecated",
          tags: ["eng"],
          score: 1,
          snippet: "A decision that no longer holds.",
        },
      ],
    };
  }
  if (path.includes("path=notes")) {
    return {
      mode: "text",
      total: 1,
      page: 1,
      limit: 50,
      count: 1,
      hits: [
        {
          domain: "eng",
          permalink: "notes/beta",
          title: "Beta",
          engram_type: "engram",
          kind: "engram",
          status: "stable",
          tags: [],
        },
      ],
    };
  }
  return {
    mode: "text",
    total: 2,
    page: 1,
    limit: 50,
    count: 2,
    hits: [
      {
        domain: "eng",
        permalink: "alpha",
        title: "Alpha",
        engram_type: "engram",
        kind: "engram",
        status: "stable",
        tags: [],
      },
      {
        domain: "eng",
        permalink: "notes/beta",
        title: "Beta",
        engram_type: "engram",
        kind: "engram",
        status: "stable",
        tags: [],
      },
    ],
  };
}

function vocabularyResponse() {
  return {
    domain: "eng",
    tags: [{ name: "eng", engrams: 3, observations: 5 }],
    categories: [],
    relation_types: [],
  };
}

function serve(routes: Record<string, (path: string) => unknown> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      "/domains/eng/manifest": () => ({ domain: "eng", markdown: MANIFEST }),
      "/domains/eng/tree": treeResponse,
      "/domains/eng/engrams": engramsResponse,
      "/vocabulary": vocabularyResponse,
      ...routes,
    }),
  );
}

/** Every path the app asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => call[0]);
}

/**
 * The screen itself, without the frame around it.
 *
 * Inside a domain the sidebar draws the same folders and the same engrams as
 * navigation, so a query for one of them has to say which of the two it means.
 */
async function screenBody(): Promise<HTMLElement> {
  return screen.findByRole("main");
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the domain screen", () => {
  it("renders the manifest's lede and the engrams at the root of the domain", async () => {
    serve();

    renderApp("/d/eng");

    expect(await screen.findByRole("heading", { name: "eng" })).toBeVisible();
    // The manifest is one paragraph here and a link to the rest: what a
    // reader arriving in a domain needs is what it is for, then its engrams,
    // and a whole document in between put the list below the fold.
    expect(
      await screen.findByText("What this domain is for, in one paragraph."),
    ).toBeVisible();
    expect(
      screen.getByRole("link", { name: "Read the MANIFEST" }),
    ).toHaveAttribute("href", "/d/eng/manifest");
    expect(screen.queryByRole("heading", { name: "When to Use" })).toBeNull();
    const row = await within(await screenBody()).findByRole("link", {
      name: /Alpha/,
    });
    expect(row).toHaveAttribute("href", "/d/eng/e/alpha");
  });

  it("says a manifest with no prose at all is there without quoting nothing", async () => {
    serve({
      "/domains/eng/manifest": () => ({
        domain: "eng",
        markdown: "---\ntitle: eng\n---\n\n# eng\n",
      }),
    });

    renderApp("/d/eng");

    // Headings and frontmatter are not a lede. The link is still the way in:
    // an empty summary is not the same fact as a missing MANIFEST.
    expect(
      await screen.findByRole("link", { name: "Read the MANIFEST" }),
    ).toBeVisible();
    expect(screen.queryByText(/no MANIFEST yet/)).toBeNull();
  });

  it("launches a new engram from the heading of the list it lands in", async () => {
    serve();

    renderApp("/d/eng");
    const body = await screenBody();

    // The primary tier: it is the one thing a writer comes to a domain to do,
    // and the sidebar's launcher never co-renders with it.
    const launcher = await within(body).findByRole("button", {
      name: "New engram",
    });
    expect(launcher.className).toContain("bg-accent-700");
  });

  it("says a domain nobody registered is not here", async () => {
    serve({
      "/domains/ghost/tree": () => {
        throw new ApiProblem(
          404,
          "not found",
          "no domain 'ghost' is registered",
        );
      },
      "/domains/ghost/manifest": () => {
        throw new ApiProblem(
          404,
          "not found",
          "no domain 'ghost' is registered",
        );
      },
    });

    renderApp("/d/ghost");

    expect(
      await screen.findByRole("heading", { name: "Domain not found" }),
    ).toBeVisible();
    // Distinct from an empty domain: nothing here invites the reader to add
    // an engram to a domain that does not exist.
    expect(screen.queryByText(/no engrams yet/)).toBeNull();
  });

  it("says an empty domain is empty, and not that it is missing", async () => {
    serve({
      "/domains/eng/tree": () => ({
        domain: "eng",
        path: "/",
        folders: [],
        engrams: [],
      }),
      "/domains/eng/engrams": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
    });

    renderApp("/d/eng");

    // Scoped to the screen: the sidebar says the same thing about the same
    // empty domain, which is the frame's own line rather than this one.
    expect(
      await within(await screenBody()).findByText(/no engrams yet/),
    ).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "Domain not found" }),
    ).toBeNull();
  });

  it("treats a missing manifest as a gap, not as a missing domain", async () => {
    serve({
      "/domains/eng/manifest": () => {
        throw new ApiProblem(404, "not found", "no MANIFEST in domain 'eng'");
      },
    });

    renderApp("/d/eng");

    expect(await screen.findByText(/no MANIFEST yet/)).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "Domain not found" }),
    ).toBeNull();
    // The engrams are still listed: the manifest is one panel, not the screen.
    expect(
      await within(await screenBody()).findByRole("link", { name: /Alpha/ }),
    ).toBeVisible();
  });

  it("opens a folder into its own list", async () => {
    serve();

    renderApp("/d/eng");
    const body = await screenBody();
    await within(body).findByRole("link", { name: /Alpha/ });

    await userEvent.click(within(body).getByRole("button", { name: "notes" }));

    expect(
      await within(body).findByRole("link", { name: /Beta/ }),
    ).toHaveAttribute("href", "/d/eng/e/notes/beta");
    await waitFor(() => {
      expect(requested().some((path) => path.includes("path=notes"))).toBe(
        true,
      );
    });
  });

  it("pages a folder from the listing rather than from the tree", async () => {
    serve({
      "/domains/eng/engrams": (path) =>
        path.includes("path=notes")
          ? {
              mode: "text",
              total: 620,
              page: 1,
              limit: 50,
              count: 1,
              hits: [
                {
                  domain: "eng",
                  permalink: "notes/beta",
                  title: "Beta",
                  engram_type: "engram",
                  kind: "engram",
                  status: "stable",
                  tags: [],
                },
              ],
            }
          : engramsResponse(path),
    });

    // Straight to the folder, because the whole of this screen's state is its
    // URL: the same link somebody sends, and the same address the back button
    // returns to.
    renderApp("/d/eng?path=notes");
    const body = await screenBody();

    expect(
      await within(body).findByRole("link", { name: /Beta/ }),
    ).toBeVisible();
    // The listing endpoint, scoped and paged, rather than the tree's own rows:
    // a folder of six hundred engrams costs one page here.
    await waitFor(() => {
      expect(
        requested().some(
          (path) =>
            path.startsWith("/domains/eng/engrams?") &&
            path.includes("path=notes") &&
            path.includes("limit=50"),
        ),
      ).toBe(true);
    });
    // And the count is the envelope's, not the number of rows in hand.
    expect(
      await within(body).findByText(/620 engrams in this folder/),
    ).toBeVisible();
  });

  it("keeps a filter across the whole domain while a folder is open", async () => {
    serve();

    renderApp("/d/eng?path=notes");
    const body = await screenBody();
    await within(body).findByRole("link", { name: /Beta/ });

    await userEvent.click(within(body).getByRole("button", { name: /#eng/ }));

    await screen.findByRole("link", { name: /Gamma/ });
    // The filtered view is the whole domain, every folder included, and it
    // says so. Scoping it to the folder being browsed would be a different
    // feature, not a side effect of paging the browse view.
    expect(screen.getByText(/whole domain/i)).toBeVisible();
    const filtered = requested().filter(
      (path) =>
        path.startsWith("/domains/eng/engrams?") && path.includes("tags=eng"),
    );
    expect(filtered.length).toBeGreaterThan(0);
    expect(filtered.every((path) => !path.includes("path="))).toBe(true);
  });

  it("switches to the whole domain when a tag is filtered on", async () => {
    serve();

    renderApp("/d/eng");
    const body = await screenBody();
    await within(body).findByRole("link", { name: /Alpha/ });

    await userEvent.click(within(body).getByRole("button", { name: /#eng/ }));

    // The frontmatter view answers, and it is the one that carries status.
    const row = await screen.findByRole("link", { name: /Gamma/ });
    expect(row).toHaveTextContent("deprecated");
    await waitFor(() => {
      expect(
        requested().some(
          (path) =>
            path.startsWith("/domains/eng/engrams") &&
            path.includes("tags=eng"),
        ),
      ).toBe(true);
    });
    // And the screen says which view is on screen, so the switch is not a
    // silent one.
    expect(screen.getByText(/whole domain/i)).toBeVisible();
  });
});
