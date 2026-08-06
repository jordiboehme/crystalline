/**
 * One domain: what it is for, what is in it, and the two states that must never
 * look alike - a domain nobody registered, and a registered domain with nothing
 * in it yet. The first is a wrong address and the second is an invitation, so
 * the screen says which one happened rather than showing one empty box for
 * both.
 *
 * The browse view and the filter view come from two endpoints on purpose: the
 * tree owns navigation by folder, and the engram listing owns the frontmatter
 * filters. The screen names which one is on screen instead of blending them.
 */

import { screen, waitFor } from "@testing-library/react";
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

const MANIFEST = ["---", "title: eng", "---", "", "## When to Use", ""].join(
  "\n",
);

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

/** The frontmatter view: one retired engram, so the fade has something to do. */
function engramsResponse() {
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

beforeEach(() => {
  apiMock.mockReset();
});

describe("the domain screen", () => {
  it("renders the manifest and the engrams at the root of the domain", async () => {
    serve();

    renderApp("/d/eng");

    expect(await screen.findByRole("heading", { name: "eng" })).toBeVisible();
    // From the manifest markdown, which means it was fetched and rendered
    // rather than shown as source.
    expect(
      await screen.findByRole("heading", { name: "When to Use" }),
    ).toBeVisible();
    const row = await screen.findByRole("link", { name: /Alpha/ });
    expect(row).toHaveAttribute("href", "/d/eng/e/alpha");
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
    });

    renderApp("/d/eng");

    expect(await screen.findByText(/no engrams yet/)).toBeVisible();
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
    expect(await screen.findByRole("link", { name: /Alpha/ })).toBeVisible();
  });

  it("opens a folder into its own list", async () => {
    serve();

    renderApp("/d/eng");
    await screen.findByRole("link", { name: /Alpha/ });

    await userEvent.click(await screen.findByRole("button", { name: "notes" }));

    expect(await screen.findByRole("link", { name: /Beta/ })).toHaveAttribute(
      "href",
      "/d/eng/e/notes/beta",
    );
    await waitFor(() => {
      expect(requested().some((path) => path.includes("path=notes"))).toBe(
        true,
      );
    });
  });

  it("switches to the whole domain when a tag is filtered on", async () => {
    serve();

    renderApp("/d/eng");
    await screen.findByRole("link", { name: /Alpha/ });

    await userEvent.click(await screen.findByRole("button", { name: /#eng/ }));

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
