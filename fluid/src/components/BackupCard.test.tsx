/**
 * The backup card: the archive round trip, in a box of its own at the foot of
 * the domain page.
 *
 * Mounted through the domain screen, the way the members card and the sync
 * card are, because what is under test is compositional: both halves are
 * admin-only endpoints, so what the card draws follows the signed-in
 * account's role rather than anything the card is handed.
 */

import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import type { Role } from "../api/model";
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

function serve(routes: Record<string, Answer> = {}, role: Role = "editor") {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role }) }),
      "/domains": domainsResponse,
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

beforeEach(() => {
  apiMock.mockReset();
  localStorage.clear();
});

describe("the backup card", () => {
  it("offers the archive round trip to an admin", async () => {
    serve({}, "admin");

    renderApp("/d/eng");
    const card = await screen.findByRole("region", { name: "Backup" });

    // A link the browser saves rather than a fetch the app holds in memory:
    // the download is a cookie-authenticated GET, so the anchor is the whole
    // mechanism, and `download` is what makes it a save rather than a
    // navigation into a zip.
    const download = within(card).getByRole("link", {
      name: "Download archive",
    });
    expect(download).toHaveAttribute("href", "/api/v1/domains/eng/archive");
    expect(download).toHaveAttribute("download");
    expect(
      within(card).getByRole("button", { name: "Import archive" }),
    ).toBeVisible();
  });

  it("opens the import dialog from its own button", async () => {
    serve({}, "admin");

    renderApp("/d/eng");
    const card = await screen.findByRole("region", { name: "Backup" });

    await userEvent.click(
      within(card).getByRole("button", { name: "Import archive" }),
    );

    expect(
      await screen.findByRole("dialog", { name: /import/i }),
    ).toBeVisible();
  });

  it("draws no card at all below admin", async () => {
    serve();

    renderApp("/d/eng");
    await screen.findByRole("heading", { level: 1, name: "eng" });

    // Both endpoints are admin-only, so neither control is drawn for anybody
    // who would be refused at it, and the box they live in goes with them.
    expect(screen.queryByRole("region", { name: "Backup" })).toBeNull();
    expect(screen.queryByRole("link", { name: "Download archive" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Import archive" })).toBeNull();
  });
});
