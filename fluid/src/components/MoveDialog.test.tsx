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
    await userEvent.click(await screen.findByRole("button", { name: "Move" }));
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
    const trail = await screen.findByRole("navigation", { name: "Breadcrumb" });
    await waitFor(() => {
      expect(within(trail).getByText("guides")).toBeInTheDocument();
    });
  });
});
