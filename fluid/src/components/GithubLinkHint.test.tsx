/**
 * The hint an account with no GitHub link gets where a write button would
 * be on a shared domain GitHub governs: it reads there as everyone signed
 * in does, and writes only once GitHub knows who it is.
 */

import { screen, waitFor } from "@testing-library/react";
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

const UNLINKED_HINT =
  "Connect GitHub on your profile to contribute, and to open the private domains you can read on GitHub.";

/** A viewer-level listing row: this account may not write `eng`. */
function readOnlyListing() {
  const listing = domainsResponse();
  return {
    ...listing,
    domains: listing.domains.map((row) => ({ ...row, read_only: true })),
  };
}

function serve(members: Record<string, unknown>, linked: boolean) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "ada", role: "editor" }) }),
      "/domains": readOnlyListing,
      "/domains/eng/manifest": () => ({ domain: "eng", markdown: "# eng\n" }),
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
      "/vocabulary": () => ({
        domain: "eng",
        tags: [],
        categories: [],
        relation_types: [],
      }),
      "/domains/eng/members": () => ({
        owner: null,
        visibility: "shared",
        members: [],
        overridden: [],
        ...members,
      }),
      "/me/github-link": () => ({
        linked,
        app_configured: true,
        token_live: linked,
      }),
    }),
  );
}

/** Whether the app asked for `path`. */
function asked(path: string): boolean {
  return apiMock.mock.calls.some(([sent]) => sent === path);
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the unlinked hint beside the write button", () => {
  it("shows on a shared GitHub domain to an account with no link", async () => {
    serve({ governance: "github", github_people: [] }, false);
    renderApp("/d/eng/engrams");
    expect(await screen.findByText(UNLINKED_HINT)).toBeVisible();
  });

  it("stays away where Crystalline decides, and never asks for the link", async () => {
    serve({ governance: "crystalline" }, false);
    renderApp("/d/eng/engrams");
    await screen.findByRole("region", { name: "Engrams" });
    await waitFor(() => {
      expect(asked("/domains/eng/members")).toBe(true);
    });
    expect(screen.queryByText(UNLINKED_HINT)).toBeNull();
    expect(asked("/me/github-link")).toBe(false);
  });

  it("stays away once the account is linked", async () => {
    serve({ governance: "github", github_people: [] }, true);
    renderApp("/d/eng/engrams");
    await waitFor(() => {
      expect(asked("/me/github-link")).toBe(true);
    });
    await screen.findByRole("region", { name: "Engrams" });
    expect(screen.queryByText(UNLINKED_HINT)).toBeNull();
  });
});
