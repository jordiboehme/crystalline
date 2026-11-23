/**
 * The Administration area: its frame, its pages and the two addresses its
 * pages had before it.
 *
 * Pinned: `/admin` opens General, the navigation marks the page on screen
 * and nothing else, an unknown page under the area is not found inside the
 * frame, a non-admin gets the not-found screen at every address of the area
 * with no navigation drawn and nothing asked of the server, and the old
 * addresses lead an admin on with their query while a non-admin is refused
 * in place.
 */

import { screen, within } from "@testing-library/react";
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

/** The app signed in as `ada` in `role`, with `routes` served beside the frame's. */
function serveAs(
  role: "admin" | "editor",
  routes: Record<string, Answer> = {},
  probe: { update_available?: string | null; version?: string } = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role }), ...probe }),
      "/domains": domainsResponse,
      "/activity": () => ({ timeframe: "7d", count: 0, engrams: [] }),
      ...routes,
    }),
  );
}

/** Every request the app sent to `prefix`, by path. */
function callsTo(prefix: string): string[] {
  return apiMock.mock.calls
    .map(([path]) => String(path))
    .filter((path) => path.startsWith(prefix));
}

/** The side navigation, which jsdom draws at every width. */
async function adminNav() {
  return screen.findByRole("navigation", { name: "Administration" });
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the Administration area", () => {
  it("opens on General, marked as the current page", async () => {
    serveAs("admin", {}, { version: "0.35.0" });
    renderApp("/admin");

    expect(
      await screen.findByRole("heading", { level: 1, name: "General" }),
    ).toBeInTheDocument();
    const nav = await adminNav();
    expect(within(nav).getByRole("link", { name: "General" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    for (const other of ["Users", "Agents", "Invites", "GitHub connection"]) {
      expect(
        within(nav).getByRole("link", { name: other }),
      ).not.toHaveAttribute("aria-current");
    }
    expect(screen.getByText("Crystalline 0.35.0")).toBeInTheDocument();
    expect(screen.getByText(/No newer release is known/)).toBeInTheDocument();
    expect(screen.getByText(`${window.location.origin}/`)).toBeInTheDocument();
    expect(document.title).toBe("Administration - Fluid");
  });

  it("names every page at its address and links out to Maintenance", async () => {
    serveAs("admin");
    renderApp("/admin/general");

    const nav = await adminNav();
    const hrefs = Object.fromEntries(
      within(nav)
        .getAllByRole("link")
        .map((link) => [link.textContent, link.getAttribute("href")]),
    );
    expect(hrefs).toEqual({
      General: "/admin/general",
      Users: "/admin/users",
      Agents: "/admin/agents",
      Invites: "/admin/invites",
      "GitHub connection": "/admin/github",
      Maintenance: "/maintenance",
    });
    // The select that stands in for the navigation at drawer width names
    // the same pages and shows the one on screen.
    expect(
      screen.getByRole("combobox", { name: "Administration page" }),
    ).toHaveValue("/admin/general");
  });

  it("shows the newer release the server's update check found", async () => {
    serveAs("admin", {}, { version: "0.35.0", update_available: "0.36.0" });
    renderApp("/admin/general");

    expect(await screen.findByText("0.36.0 is available.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Release notes/ })).toHaveAttribute(
      "href",
      "https://github.com/jordiboehme/crystalline/releases/tag/v0.36.0",
    );
    expect(screen.getByText("crystalline update now")).toBeInTheDocument();
  });

  it("moves between its pages from the navigation", async () => {
    serveAs("admin");
    renderApp("/admin/general");

    const user = userEvent.setup();
    await user.click(
      within(await adminNav()).getByRole("link", { name: "Agents" }),
    );
    expect(
      await screen.findByRole("heading", { level: 1, name: "Agents" }),
    ).toBeInTheDocument();
    expect(
      within(await adminNav()).getByRole("link", { name: "Agents" }),
    ).toHaveAttribute("aria-current", "page");

    await user.selectOptions(
      screen.getByRole("combobox", { name: "Administration page" }),
      "Invites",
    );
    expect(
      await screen.findByRole("heading", { level: 1, name: "Invites" }),
    ).toBeInTheDocument();
  });

  it("draws the users page inside the frame", async () => {
    serveAs("admin", { "/users": () => ({ users: [] }) });
    renderApp("/admin/users");

    expect(
      await screen.findByRole("heading", { level: 1, name: "Users" }),
    ).toBeInTheDocument();
    expect(
      within(await adminNav()).getByRole("link", { name: "Users" }),
    ).toHaveAttribute("aria-current", "page");
  });

  it("answers an unknown page under the area with not-found, inside the frame", async () => {
    serveAs("admin");
    renderApp("/admin/nonsense");

    expect(
      await screen.findByText(/nothing is filed at this address/i),
    ).toBeInTheDocument();
    const nav = await adminNav();
    expect(within(nav).queryByRole("link", { current: "page" })).toBeNull();
  });

  it.each(["/admin", "/admin/general", "/admin/users", "/admin/github"])(
    "is not there for a non-admin at %s",
    async (address) => {
      serveAs("editor");
      renderApp(address);

      expect(
        await screen.findByText(/nothing is filed at this address/i),
      ).toBeInTheDocument();
      expect(
        screen.queryByRole("navigation", { name: "Administration" }),
      ).toBeNull();
      expect(callsTo("/users")).toHaveLength(0);
      expect(callsTo("/settings/github")).toHaveLength(0);
      expect(document.title).toBe("Fluid");
    },
  );
});

describe("the command palette", () => {
  it("offers an admin the area and its users page", async () => {
    serveAs("admin");
    renderApp("/");
    await screen.findByRole("heading", { name: "Home" });

    const user = userEvent.setup();
    await user.keyboard("{Control>}k{/Control}");
    const palette = await screen.findByRole("dialog", {
      name: /command palette/i,
    });
    expect(
      await within(palette).findByRole("option", { name: "Users" }),
    ).toBeInTheDocument();
    await user.click(
      within(palette).getByRole("option", { name: "Administration" }),
    );
    expect(
      await screen.findByRole("heading", { level: 1, name: "General" }),
    ).toBeInTheDocument();
  });
});

describe("the addresses admin pages had before the area", () => {
  it("leads /users on to the users page", async () => {
    serveAs("admin", { "/users": () => ({ users: [] }) });
    renderApp("/users");

    expect(
      await screen.findByRole("heading", { level: 1, name: "Users" }),
    ).toBeInTheDocument();
    expect(
      within(await adminNav()).getByRole("link", { name: "Users" }),
    ).toHaveAttribute("aria-current", "page");
  });

  it("leads /settings/github on with its query", async () => {
    serveAs("admin", {
      "/settings/github": () => ({
        enabled: true,
        connected: true,
        user: null,
        token_store: "GitHub App",
        pending: null,
        error: null,
        app: {
          app_id: 7,
          slug: "crystalline-test",
          source: "file",
          install_url: "https://github.example/apps/x/installations/new",
          webhook_secret_set: false,
          error: null,
        },
      }),
    });
    renderApp("/settings/github?app=created");

    // The landing the page reads from `?app=created` is only there if the
    // query survived the move.
    expect(
      await screen.findByText(/The GitHub App is created/),
    ).toBeInTheDocument();
    expect(
      within(await adminNav()).getByRole("link", { name: "GitHub connection" }),
    ).toHaveAttribute("aria-current", "page");
  });

  it.each(["/users", "/settings/github"])(
    "answers %s with not-found for a non-admin, in place",
    async (address) => {
      serveAs("editor");
      renderApp(address);

      expect(
        await screen.findByText(/nothing is filed at this address/i),
      ).toBeInTheDocument();
      expect(
        screen.queryByRole("navigation", { name: "Administration" }),
      ).toBeNull();
      expect(callsTo("/users")).toHaveLength(0);
      expect(callsTo("/settings/github")).toHaveLength(0);
    },
  );
});
