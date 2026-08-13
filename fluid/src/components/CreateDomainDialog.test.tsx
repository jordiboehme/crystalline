/**
 * Registering a domain, from the frame.
 *
 * Three kinds of domain go in through one form, and what is pinned here is
 * what an admin has to be able to trust about it: that the mode decides what is
 * asked for, that a field nobody filled in is left out of the request rather
 * than sent as an empty string, that team mode says so plainly when this
 * instance has no GitHub connection to register against, that a refusal arrives
 * in the server's own words with the form still standing, and that the whole
 * door exists for admins alone.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
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

/** The GitHub connection, in whichever of its two states a test needs. */
function githubStatus(connected: boolean) {
  return {
    enabled: true,
    connected,
    user: connected ? "octo" : null,
    token_store: connected ? "keyring" : null,
    pending: null,
    error: null,
  };
}

/** The app, signed in as `root` in the given role. */
function serveAs(
  role: "admin" | "editor",
  routes: Record<string, Answer> = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "root", role }) }),
      "/domains": domainsResponse,
      "/activity": () => ({ timeframe: "7d", items: [] }),
      // The screen the flows below are opened from, which is any screen
      // outside a domain that is not the home screen. It draws nothing this
      // suite asserts on; it is here to be somewhere the frame's own launcher
      // is offered.
      "/users": () => ({ users: [] }),
      ...routes,
    }),
  );
}

/** The body of the request the app sent to `path` with `method`, parsed. */
function sentBody(path: string, method: string): unknown {
  const call = apiMock.mock.calls.find(
    ([sent, init]) => sent === path && init?.method === method,
  );
  if (!call) {
    throw new Error(`no ${method} to ${path}`);
  }
  const body = call[1]?.body;
  if (typeof body !== "string") {
    throw new Error(`the ${method} to ${path} carried no JSON body`);
  }
  return JSON.parse(body) as unknown;
}

/** How many times the domain listing was read. */
function listingReads(): number {
  return apiMock.mock.calls.filter(
    ([path, init]) => path === "/domains" && init?.method === undefined,
  ).length;
}

/** Every call the app made to the GitHub connection. */
function settingsCalls(): unknown[] {
  return apiMock.mock.calls.filter(([path]) =>
    String(path).startsWith("/settings/github"),
  );
}

/** Whether anything was registered at all. */
function registrations(): number {
  return apiMock.mock.calls.filter(
    ([path, init]) => path === "/domains" && init?.method === "POST",
  ).length;
}

/**
 * Open the dialog from the frame's own launcher.
 *
 * Scoped to the sidebar deliberately, and opened from a screen that is not the
 * home screen: home is the one place the frame's launcher yields to the
 * screen's own (see the pair of tests at the bottom), so the flows below drive
 * the frame from `/users`, where the sidebar is the only way in.
 */
async function openFromSidebar(): Promise<HTMLElement> {
  const sidebar = within(
    await screen.findByRole("navigation", { name: "Domains" }),
  );
  await userEvent.click(
    await sidebar.findByRole("button", { name: "New domain" }),
  );
  return screen.findByRole("dialog", { name: /new domain/i });
}

beforeEach(() => {
  apiMock.mockReset();
  localStorage.clear();
});

describe("registering a domain", () => {
  it("creates a local domain and navigates there", async () => {
    const created = vi.fn(() => ({ domain: "notes", root: "/srv/kb/notes" }));
    serveAs("admin", {
      "/domains": (_path, init) =>
        init?.method === "POST" ? created() : domainsResponse(),
      "/domains/notes/manifest": () => ({
        domain: "notes",
        markdown: "# notes",
      }),
      "/domains/notes/tree": () => ({
        domain: "notes",
        path: "/",
        folders: [],
        engrams: [],
      }),
      "/domains/notes/engrams": () => ({
        mode: "text",
        total: 0,
        page: 1,
        limit: 50,
        count: 0,
        hits: [],
      }),
      "/vocabulary": () => ({
        domain: "notes",
        tags: [],
        categories: [],
        relation_types: [],
      }),
    });
    renderApp("/users");

    const dialog = await openFromSidebar();
    const before = listingReads();
    await userEvent.type(within(dialog).getByLabelText("Name"), "notes");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create domain" }),
    );

    await waitFor(() => {
      expect(created).toHaveBeenCalled();
    });
    // Local is the mode a form nobody touched is in, and the request carries
    // nothing else: no empty repository, no empty branch.
    expect(sentBody("/domains", "POST")).toEqual({
      mode: "local",
      name: "notes",
    });
    // The flow ends where the domain now is.
    expect(
      await screen.findByRole("heading", { level: 1, name: "notes" }),
    ).toBeVisible();
    // And the listing every screen reads is read again, so the sidebar holds
    // what was just registered.
    await waitFor(() => {
      expect(listingReads()).toBeGreaterThan(before);
    });
  });

  it("creates a virtual domain", async () => {
    const created = vi.fn(() => ({ domain: "scratch", root: null }));
    serveAs("admin", {
      "/domains": (_path, init) =>
        init?.method === "POST" ? created() : domainsResponse(),
    });
    renderApp("/users");

    const dialog = await openFromSidebar();
    await userEvent.click(
      within(dialog).getByRole("radio", { name: "Virtual" }),
    );
    await userEvent.type(within(dialog).getByLabelText("Name"), "scratch");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create domain" }),
    );

    await waitFor(() => {
      expect(created).toHaveBeenCalled();
    });
    expect(sentBody("/domains", "POST")).toEqual({
      mode: "virtual",
      name: "scratch",
    });
  });

  it("team mode without a connection disables submit and links settings", async () => {
    serveAs("admin", {
      "/settings/github": () => githubStatus(false),
    });
    renderApp("/users");

    const dialog = await openFromSidebar();
    await userEvent.click(
      within(dialog).getByRole("radio", { name: "GitHub team" }),
    );

    // The way out of it, in the dialog rather than in a sentence that only
    // says no.
    const link = await within(dialog).findByRole("link", {
      name: /connect github in settings/i,
    });
    expect(link).toHaveAttribute("href", "/settings/github");
    await waitFor(() => {
      expect(
        within(dialog).getByRole("button", { name: "Create domain" }),
      ).toBeDisabled();
    });
    await userEvent.type(
      within(dialog).getByLabelText("Repository"),
      "acme/kb",
    );
    expect(
      within(dialog).getByRole("button", { name: "Create domain" }),
    ).toBeDisabled();
    expect(registrations()).toBe(0);
  });

  it("team mode when connected posts repo, branch and path", async () => {
    const created = vi.fn(() => ({ domain: "kb", root: "/srv/kb/kb" }));
    serveAs("admin", {
      "/settings/github": () => githubStatus(true),
      "/domains": (_path, init) =>
        init?.method === "POST" ? created() : domainsResponse(),
    });
    renderApp("/users");

    const dialog = await openFromSidebar();
    await userEvent.click(
      within(dialog).getByRole("radio", { name: "GitHub team" }),
    );
    await userEvent.type(
      await within(dialog).findByLabelText("Repository"),
      "acme/kb",
    );
    await userEvent.type(within(dialog).getByLabelText("Branch"), "main");
    await userEvent.type(
      within(dialog).getByLabelText("Folder in the repository"),
      "docs",
    );
    await waitFor(() => {
      expect(
        within(dialog).getByRole("button", { name: "Create domain" }),
      ).toBeEnabled();
    });
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create domain" }),
    );

    await waitFor(() => {
      expect(created).toHaveBeenCalled();
    });
    // The name was left to the repository, so the request does not carry an
    // empty one: an omitted field and a field set to nothing are different
    // requests, and only one of them is what was meant.
    expect(sentBody("/domains", "POST")).toEqual({
      mode: "github",
      repo: "acme/kb",
      branch: "main",
      path: "docs",
    });
  });

  it("stays put when the report names nothing, rather than navigating to a broken route", async () => {
    // The real server always defaults a team domain's name to the
    // repository's own, so a report with no `domain` field should never
    // actually arrive here - this stands in for a server bug or a future
    // mode that could omit it, and pins that the dialog does not navigate
    // into a route built from an empty segment if one ever does.
    const created = vi.fn(() => ({}));
    serveAs("admin", {
      "/settings/github": () => githubStatus(true),
      "/domains": (_path, init) =>
        init?.method === "POST" ? created() : domainsResponse(),
    });
    renderApp("/users");

    const dialog = await openFromSidebar();
    await userEvent.click(
      within(dialog).getByRole("radio", { name: "GitHub team" }),
    );
    await userEvent.type(
      await within(dialog).findByLabelText("Repository"),
      "acme/kb",
    );
    await waitFor(() => {
      expect(
        within(dialog).getByRole("button", { name: "Create domain" }),
      ).toBeEnabled();
    });
    const before = listingReads();

    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create domain" }),
    );

    await waitFor(() => {
      expect(created).toHaveBeenCalled();
    });
    // The request carried no name either: team mode leaves it out entirely
    // when the field is empty.
    expect(sentBody("/domains", "POST")).toEqual({
      mode: "github",
      repo: "acme/kb",
    });
    // The domain was still created, so the dialog closes and the listing
    // every sidebar and switcher draws from is invalidated exactly as a
    // named create does - nothing here reads as a failure.
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: /new domain/i })).toBeNull();
    });
    await waitFor(() => {
      expect(listingReads()).toBeGreaterThan(before);
    });
    // But there is no navigation: an empty name has no route to navigate to,
    // so the screen the admin was already on is exactly where they stay.
    expect(
      await screen.findByRole("heading", { level: 1, name: "Users" }),
    ).toBeVisible();
  });

  it("shows a server refusal in its words", async () => {
    serveAs("admin", {
      "/domains": (_path, init) => {
        if (init?.method === "POST") {
          throw new ApiProblem(
            409,
            "conflict",
            "domain 'notes' is already registered",
          );
        }
        return domainsResponse();
      },
    });
    renderApp("/users");

    const dialog = await openFromSidebar();
    await userEvent.type(within(dialog).getByLabelText("Name"), "notes");
    await userEvent.click(
      within(dialog).getByRole("button", { name: "Create domain" }),
    );

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent(/already registered/);
    // The form is still standing, with what was typed still in it: the fix is
    // a different name, not everything over again.
    expect(within(dialog).getByLabelText("Name")).toHaveValue("notes");
  });

  it("offers the launcher and the palette entry to admins only", async () => {
    serveAs("editor");
    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    expect(screen.queryByRole("button", { name: "New domain" })).toBeNull();
    await userEvent.keyboard("{Meta>}k{/Meta}");
    await screen.findByRole("option", { name: /keyboard shortcuts/i });
    expect(screen.queryByRole("option", { name: /create domain/i })).toBeNull();
  });

  it("offers the palette entry to an admin", async () => {
    serveAs("admin");
    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    await userEvent.keyboard("{Meta>}k{/Meta}");
    await userEvent.click(
      await screen.findByRole("option", { name: /create domain/i }),
    );

    expect(
      await screen.findByRole("dialog", { name: /new domain/i }),
    ).toBeInTheDocument();
  });

  it("hands the launcher to the home screen there, and keeps it everywhere else", async () => {
    serveAs("admin");
    const home = renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    // One act, one control: the home screen carries the launcher beside the
    // heading it acts on, so the sidebar's yields - the rule `DomainNav`
    // already follows one level down for "New engram".
    expect(
      within(screen.getByRole("navigation", { name: "Domains" })).queryByRole(
        "button",
        { name: "New domain" },
      ),
    ).toBeNull();
    expect(
      within(screen.getByRole("main")).getByRole("button", {
        name: "New domain",
      }),
    ).toBeVisible();

    // Nowhere else does it yield: every other screen outside a domain draws
    // this listing with no launcher of its own, and the sidebar is the only
    // way in there.
    home.unmount();
    renderApp("/users");
    expect(
      await within(
        await screen.findByRole("navigation", { name: "Domains" }),
      ).findByRole("button", { name: "New domain" }),
    ).toBeVisible();
  });

  it("asks GitHub nothing while the form is in a local mode", async () => {
    serveAs("admin", { "/settings/github": () => githubStatus(true) });
    renderApp("/users");

    const dialog = await openFromSidebar();
    // Local, and then the other mode that has no repository either: the probe
    // belongs to team mode alone, and an instance's GitHub connection is not
    // something to go asking about because a dialog opened.
    await userEvent.type(within(dialog).getByLabelText("Name"), "scratch");
    await userEvent.click(
      within(dialog).getByRole("radio", { name: "Virtual" }),
    );

    expect(settingsCalls()).toHaveLength(0);
  });
});
