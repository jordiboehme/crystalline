/**
 * The team sync card, and the one thing a share surface cannot work out for
 * itself: what a reviewing domain is holding that no share would pick up.
 *
 * In review mode a write joins its author's own draft and never reaches the
 * folder the team shares, so `0 pending local changes` is true and reads as
 * "there is nothing here" over a pile of unshared work. The card says what this
 * session is holding, and - where the server sends it, which is its answer to
 * whether this caller may see it - who else is holding anything.
 *
 * Names and counts throughout. A draft is unshared by definition, so nothing
 * here ever draws a path or a line of somebody's text.
 */

import { screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import type { Answer } from "../test/harness";
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

/** The per-domain sync report, as the route flattens it. */
function syncResponse(overrides: Record<string, unknown> = {}) {
  return {
    domain: "eng",
    mode: "github",
    repo: "acme/eng",
    branch: "main",
    last_checked: "2026-09-10T09:00:00Z",
    local_changes: 0,
    open_proposals: [],
    declined_proposals: [],
    conflicts: [],
    behind: false,
    probe_error: null,
    connection: { connected: true },
    ...overrides,
  };
}

function serve(routes: Record<string, Answer> = {}) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "ada", role: "admin" }) }),
      "/domains": () => domainsResponse(),
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
      }),
      ...routes,
    }),
  );
}

describe("SyncCard", () => {
  beforeEach(() => {
    apiMock.mockReset();
  });

  it("says what this session is holding and who else is drafting", async () => {
    serve({
      "/domains/eng/sync": () =>
        syncResponse({
          my_drafts: 2,
          drafts: [
            { actor: "ada", entries: 2 },
            { actor: "bo", entries: 1 },
          ],
        }),
    });
    renderApp("/d/eng");

    const region = await screen.findByRole("region", { name: "Team sync" });
    expect(
      await within(region).findByText(/2 draft changes of yours await sharing/),
    ).toBeInTheDocument();

    const table = within(region).getByRole("table", {
      name: "Drafts held here",
    });
    expect(within(table).getByText("ada")).toBeInTheDocument();
    expect(within(table).getByText("bo")).toBeInTheDocument();

    // The header says it too, and says it off the domain listing rather than
    // off this card's read, so a member who cannot reach the sync route still
    // gets it - pinned in `DomainHome.test.tsx`, where that listing is the
    // fixture.
    expect(screen.queryByText(/You have 2 draft changes here/)).toBeNull();
  });

  it("tells a caller their own count and nobody else's", async () => {
    serve({
      "/domains/eng/sync": () => syncResponse({ my_drafts: 1 }),
    });
    renderApp("/d/eng");

    const region = await screen.findByRole("region", { name: "Team sync" });
    expect(
      await within(region).findByText(/1 draft change of yours awaits sharing/),
    ).toBeInTheDocument();
    expect(
      within(region).queryByRole("table", { name: "Drafts held here" }),
    ).not.toBeInTheDocument();
  });

  it("says nothing about drafts on a domain that takes changes directly", async () => {
    serve({ "/domains/eng/sync": () => syncResponse() });
    renderApp("/d/eng");

    const region = await screen.findByRole("region", { name: "Team sync" });
    expect(
      await within(region).findByText(/pending local changes/),
    ).toBeInTheDocument();
    expect(within(region).queryByText(/draft/)).not.toBeInTheDocument();
    expect(screen.queryByText(/draft changes here/)).not.toBeInTheDocument();
  });
});
