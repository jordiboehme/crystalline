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
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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

  describe("Last checked", () => {
    afterEach(() => {
      vi.useRealTimers();
    });

    it("says not yet when there is no check on record", async () => {
      serve({
        "/domains/eng/sync": () => syncResponse({ last_checked: null }),
      });
      renderApp("/d/eng");

      const region = await screen.findByRole("region", { name: "Team sync" });
      const dd = within(region).getByText("not yet");
      expect(dd.tagName).toBe("DD");
      expect(dd).not.toHaveAttribute("title");
    });

    it("shows the local date and time, how long ago, and (stale) after it", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      vi.setSystemTime(new Date("2026-08-10T08:13:00Z"));
      serve({
        "/domains/eng/sync": () =>
          syncResponse({
            last_checked: "2026-08-10T08:00:00Z",
            probe_error: "no route to github.com",
          }),
      });
      renderApp("/d/eng");

      const region = await screen.findByRole("region", { name: "Team sync" });
      const parsed = new Date("2026-08-10T08:00:00Z");
      const pad = (n: number) => String(n).padStart(2, "0");
      const instant = `${String(parsed.getFullYear())}-${pad(parsed.getMonth() + 1)}-${pad(parsed.getDate())} ${pad(parsed.getHours())}:${pad(parsed.getMinutes())}`;

      const dd = within(region).getByText(`${instant}, 13 minutes ago (stale)`);
      expect(dd.tagName).toBe("DD");
      expect(dd).toHaveAttribute("title", "2026-08-10T08:00:00Z");
    });

    it("ticks the relative phrase once a minute without a refetch", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      vi.setSystemTime(new Date("2026-08-10T08:00:30Z"));
      const sync = vi.fn(() =>
        syncResponse({ last_checked: "2026-08-10T08:00:00Z" }),
      );
      serve({ "/domains/eng/sync": sync });
      renderApp("/d/eng");

      const region = await screen.findByRole("region", { name: "Team sync" });
      expect(within(region).getByText(/, just now$/)).toBeInTheDocument();
      expect(sync).toHaveBeenCalledTimes(1);

      await vi.advanceTimersByTimeAsync(60_000);

      expect(within(region).getByText(/, 1 minute ago$/)).toBeInTheDocument();
      // The tick redraws the sentence; it never asks the server again.
      expect(sync).toHaveBeenCalledTimes(1);
    });
  });
});
