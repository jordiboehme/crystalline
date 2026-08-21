/**
 * The move dialog: a destination path prefilled with the engram's own
 * permalink and an optional target domain, landing on the engram at its new
 * address once the move answers.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";

// Above roughly load average 33 this file's slower tests exceed the 5000 ms
// default (a threshold effect measured 2026-08-14, plans history); the raise
// keeps a loaded machine from reading as a failure. Never raise the global
// default to hide this.
vi.setConfig({ testTimeout: 15000 });

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

/** The detail payload, in the engine's own shape - mirrors EngramEditor.test.tsx. */
function detailResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    permalink: "alpha",
    title: "Alpha",
    url: "crystalline://eng/alpha",
    path: "alpha.md",
    content:
      "---\ntitle: Alpha\npermalink: alpha\nstatus: stable\ntype: engram\n---\n\n# Alpha\n\nA rule.\n",
    checksum: "3f8a1c05e2",
    frontmatter: { engram_type: "engram", status: "stable", tags: [] },
    observations: [],
    relations: [],
    links: [],
    ...overrides,
  };
}

function serve(
  routes: Record<string, (path: string, init?: RequestInit) => unknown> = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture() }),
      "/domains": domainsResponse,
      "/domains/eng/engrams/alpha": () => detailResponse(),
      "/graph": () => ({ nodes: [], edges: [], truncated: false, hidden: 0 }),
      ...routes,
    }),
  );
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the move dialog", () => {
  it("moves to the picked destination and navigates to the new address", async () => {
    const moved = vi.fn(() => ({
      from: { domain: "eng", permalink: "alpha", path: "alpha.md" },
      to: { domain: "eng", path: "guides/alpha.md" },
      cross_domain: false,
      links_rewritten: 3,
    }));
    serve({
      "/domains/eng/move": (_path, init) =>
        init?.method === "POST" ? moved() : null,
      "/domains/eng/engrams/guides/alpha": () =>
        detailResponse({ permalink: "guides/alpha" }),
    });
    renderApp("/d/eng/e/alpha");
    // The header carries Edit alone; retirement and the move are rows in
    // its overflow menu.
    await userEvent.click(
      await screen.findByRole("button", { name: "More actions" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Move" }),
    );
    const dialog = await screen.findByRole("dialog", { name: /move/i });
    await userEvent.clear(within(dialog).getByLabelText("Destination path"));
    await userEvent.type(
      within(dialog).getByLabelText("Destination path"),
      "guides/alpha",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Move engram" }),
    );
    await waitFor(() => {
      expect(moved).toHaveBeenCalled();
    });
    // Landed on the engram at its new address, which the screen says as the
    // trail it now lives under: the folder it was moved into is a crumb of
    // its own rather than half of a permalink string.
    //
    // The trail is re-queried on every attempt rather than captured once: a
    // find that resolves before the navigation completes hands back the OLD
    // page's breadcrumb, and that node never receives "guides" no matter how
    // long the assertion waits.
    await waitFor(() => {
      const trail = screen.getByRole("navigation", { name: "Breadcrumb" });
      expect(within(trail).getByText("guides")).toBeInTheDocument();
    });
    // A move that left nothing behind says nothing: the dialog is gone and
    // there is no notice to dismiss.
    expect(screen.queryByRole("dialog", { name: /move/i })).toBeNull();
  });

  it("holds the dialog open on an attachment warning and travels only on the button", async () => {
    const moved = vi.fn(() => ({
      from: { domain: "eng", permalink: "alpha", path: "alpha.md" },
      to: { domain: "eng", path: "guides/alpha.md" },
      cross_domain: false,
      links_rewritten: 0,
      attachment_warnings: [
        "assets/2026/08/shot.png is referenced but stayed in eng",
      ],
    }));
    serve({
      "/domains/eng/move": (_path, init) =>
        init?.method === "POST" ? moved() : null,
      "/domains/eng/engrams/guides/alpha": () =>
        detailResponse({ permalink: "guides/alpha" }),
    });
    renderApp("/d/eng/e/alpha");
    await userEvent.click(
      await screen.findByRole("button", { name: "More actions" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Move" }),
    );
    const dialog = await screen.findByRole("dialog", { name: /move/i });
    await userEvent.clear(within(dialog).getByLabelText("Destination path"));
    await userEvent.type(
      within(dialog).getByLabelText("Destination path"),
      "guides/alpha",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Move engram" }),
    );

    // The move landed; what it left behind is on screen, in the one surface
    // still mounted. Navigating now would take the notice with it.
    await waitFor(() => {
      expect(moved).toHaveBeenCalled();
    });
    expect(within(dialog).getByRole("alert")).toHaveTextContent(
      "assets/2026/08/shot.png is referenced but stayed in eng",
    );
    // Nobody has travelled: the new address has not been read, which is the
    // first thing landing on it would do.
    expect(reads("/domains/eng/engrams/guides/alpha")).toBe(0);

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Continue to the engram" }),
    );
    await waitFor(() => {
      const landed = screen.getByRole("navigation", { name: "Breadcrumb" });
      expect(within(landed).getByText("guides")).toBeInTheDocument();
    });
    expect(screen.queryByRole("dialog", { name: /move/i })).toBeNull();
  });

  it("moves the tree on, so the sidebar shows the new address", async () => {
    const moved = vi.fn(() => ({
      from: { domain: "eng", permalink: "alpha", path: "alpha.md" },
      to: { domain: "eng", path: "guides/alpha.md" },
      cross_domain: false,
      links_rewritten: 0,
    }));
    serve({
      "/domains/eng/move": (_path, init) =>
        init?.method === "POST" ? moved() : null,
      "/domains/eng/engrams/guides/alpha": () =>
        detailResponse({ permalink: "guides/alpha" }),
      "/domains/eng/tree": () => ({
        domain: "eng",
        path: "/",
        folders: [],
        engrams: [
          {
            permalink: "alpha",
            title: "Alpha",
            type: "engram",
            status: "stable",
            path: "alpha.md",
          },
        ],
      }),
    });
    renderApp("/d/eng/e/alpha");
    await screen.findByRole("link", { name: "Alpha" });
    // The tree is fresh for a minute, so nothing but an invalidation can make
    // it be asked for again.
    const before = trees().length;

    await userEvent.click(
      await screen.findByRole("button", { name: "More actions" }),
    );
    await userEvent.click(
      await screen.findByRole("menuitem", { name: "Move" }),
    );
    const dialog = await screen.findByRole("dialog", { name: /move/i });
    await userEvent.clear(within(dialog).getByLabelText("Destination path"));
    await userEvent.type(
      within(dialog).getByLabelText("Destination path"),
      "guides/alpha",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Move engram" }),
    );

    await waitFor(() => {
      expect(moved).toHaveBeenCalled();
    });
    // A move is exactly the write that changes the shape of a domain, so the
    // tree is read again rather than left pointing at where the engram was.
    await waitFor(() => {
      expect(trees().length).toBeGreaterThan(before);
    });
  });
});

/** How many times one route was asked for. */
function reads(route: string): number {
  return apiMock.mock.calls.filter(([path]) => path === route).length;
}

/** Every read of this domain's tree, in order. */
function trees(): string[] {
  return apiMock.mock.calls
    .map(([path]) => path)
    .filter((path) => path.startsWith("/domains/eng/tree"));
}
