/**
 * Who may read a team domain, asked in the create dialog.
 *
 * On an instance with a GitHub App, access to a team domain follows GitHub,
 * and nobody should open a confidential repository by accident: the dialog
 * asks who may read it, with no answer preselected, and the create stays
 * disabled until one is chosen. Without an App the dialog is what it was.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

/** A connected GitHub, and an App configured or not. */
function serveWithApp(appConfigured: boolean, created: () => unknown) {
  const routes: Record<string, Answer> = {
    "/auth/me": () =>
      meResponse({ user: userFixture({ name: "root", role: "admin" }) }),
    "/domains": (_path, init) =>
      init?.method === "POST" ? created() : domainsResponse(),
    "/activity": () => ({ timeframe: "7d", items: [] }),
    "/users": () => ({ users: [] }),
    "/settings/github": () => ({
      enabled: true,
      connected: true,
      user: "octo",
      token_store: "keyring",
      pending: null,
      error: null,
    }),
    "/me/github-link": () => ({
      linked: false,
      login: null,
      github_id: null,
      host: null,
      linked_at: null,
      token_live: false,
      pending: null,
      error: null,
      app_configured: appConfigured,
    }),
  };
  apiMock.mockImplementation(answersFor(routes));
}

/** The body of the POST to `/domains`, parsed. */
function sentCreate(): unknown {
  const call = apiMock.mock.calls.find(
    ([path, init]) => path === "/domains" && init?.method === "POST",
  );
  const body = call?.[1]?.body;
  if (typeof body !== "string") {
    throw new Error("no POST to /domains");
  }
  return JSON.parse(body) as unknown;
}

/** Open the dialog from the sidebar, in team mode, with a repository typed. */
async function openTeamMode(): Promise<HTMLElement> {
  const sidebar = within(
    await screen.findByRole("navigation", { name: "Domains" }),
  );
  await userEvent.click(
    await sidebar.findByRole("button", { name: "New domain" }),
  );
  const dialog = await screen.findByRole("dialog", { name: /new domain/i });
  await userEvent.click(
    within(dialog).getByRole("radio", { name: "GitHub team" }),
  );
  await userEvent.type(
    await within(dialog).findByLabelText("Repository"),
    "acme/kb",
  );
  return dialog;
}

beforeEach(() => {
  apiMock.mockReset();
  localStorage.clear();
});

describe("who may read a team domain", () => {
  it("connect dialog requires a readers choice", async () => {
    const created = vi.fn(() => ({ domain: "kb", root: "/srv/kb/kb" }));
    serveWithApp(true, created);
    renderApp("/users");
    const dialog = await openTeamMode();

    const group = await within(dialog).findByRole("radiogroup", {
      name: "Who may read this domain?",
    });
    const shared = within(group).getByRole("radio", {
      name: "Everyone signed in",
    });
    const private_ = within(group).getByRole("radio", {
      name: "Only people with access on GitHub",
    });
    // Nothing chosen for the person, and nothing to create until they choose.
    expect(shared).not.toBeChecked();
    expect(private_).not.toBeChecked();
    const create = within(dialog).getByRole("button", {
      name: "Create domain",
    });
    expect(create).toBeDisabled();
    // The readers question replaces the Private checkbox here: on a domain
    // whose access follows GitHub, private means GitHub's readers.
    expect(within(dialog).queryByLabelText("Private")).toBeNull();

    await userEvent.click(private_);
    await waitFor(() => {
      expect(create).toBeEnabled();
    });
    await userEvent.click(create);
    await waitFor(() => {
      expect(created).toHaveBeenCalled();
    });
    expect(sentCreate()).toEqual({
      mode: "github",
      repo: "acme/kb",
      readers: "private",
    });
  });

  it("asks nothing without an App", async () => {
    const created = vi.fn(() => ({ domain: "kb", root: "/srv/kb/kb" }));
    serveWithApp(false, created);
    renderApp("/users");
    const dialog = await openTeamMode();

    const create = within(dialog).getByRole("button", {
      name: "Create domain",
    });
    await waitFor(() => {
      expect(create).toBeEnabled();
    });
    expect(
      within(dialog).queryByRole("radiogroup", {
        name: "Who may read this domain?",
      }),
    ).toBeNull();
    expect(within(dialog).getByLabelText("Private")).toBeVisible();
    await userEvent.click(create);
    await waitFor(() => {
      expect(created).toHaveBeenCalled();
    });
    expect(sentCreate()).toEqual({ mode: "github", repo: "acme/kb" });
  });
});
