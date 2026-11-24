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

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api } from "../api/client";
import type { MeResponse } from "../api/model";
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
  probe: Parameters<typeof meResponse>[0] = {},
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
    expect(
      screen.getByText(
        "The server did not say whether its update check is on.",
      ),
    ).toBeInTheDocument();
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

  it("keeps Users the current page at an account's address", async () => {
    serveAs("admin", { "/users": () => ({ users: [], total: 0 }) });
    renderApp("/admin/users/eddy");
    expect(
      within(await adminNav()).getByRole("link", { name: "Users" }),
    ).toHaveAttribute("aria-current", "page");
    expect(
      screen.getByRole("combobox", { name: "Administration page" }),
    ).toHaveValue("/admin/users");
  });

  it("keeps the current page with a trailing slash", async () => {
    serveAs("admin");
    renderApp("/admin/general/");
    expect(
      within(await adminNav()).getByRole("link", { name: "General" }),
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

/** The probe's `update_check`, on by default and last run three hours ago. */
function checkState(
  overrides: Partial<NonNullable<MeResponse["update_check"]>> = {},
): NonNullable<MeResponse["update_check"]> {
  return {
    on: true,
    off_reason: null,
    install: "tarball",
    last_checked: new Date(Date.now() - 3 * 3600 * 1000).toISOString(),
    last_check: "up_to_date",
    ...overrides,
  };
}

/** The Updates row's text, once the page is drawn. */
async function updatesRow(): Promise<HTMLElement> {
  const term = await screen.findByText("Updates", { selector: "dt" });
  const row = term.parentElement;
  if (row === null) {
    throw new Error("the Updates term has no row");
  }
  return row;
}

describe("the Updates row of the General page", () => {
  it("says the check is on, when it last ran and that nothing is newer", async () => {
    serveAs("admin", {}, { update_check: checkState() });
    renderApp("/admin/general");

    expect(await updatesRow()).toHaveTextContent(
      "Update check on. Last checked 3 hours ago. No newer release.",
    );
  });

  it("says a check that is on has not run yet", async () => {
    serveAs(
      "admin",
      {},
      {
        update_check: checkState({ last_checked: null, last_check: null }),
      },
    );
    renderApp("/admin/general");

    expect(await updatesRow()).toHaveTextContent(
      "Update check on. It has not checked yet.",
    );
  });

  it("says when the check ran when the server sent a result it does not know", async () => {
    serveAs(
      "admin",
      {},
      // A result word from a newer server, which this page does not know.
      { update_check: checkState({ last_check: "later" as never }) },
    );
    renderApp("/admin/general");

    const row = await updatesRow();
    expect(row).toHaveTextContent("Update check on. Last checked 3 hours ago.");
    expect(row).not.toHaveTextContent("has not checked");
    expect(row).not.toHaveTextContent("newer release");
    expect(row).not.toHaveTextContent("failed");
  });

  it("says the last check failed", async () => {
    serveAs(
      "admin",
      {},
      { update_check: checkState({ last_check: "failed" }) },
    );
    renderApp("/admin/general");

    expect(await updatesRow()).toHaveTextContent(
      "Update check on. Last checked 3 hours ago. The check failed.",
    );
  });

  it("keeps the release, its notes and its command when one is offered", async () => {
    serveAs(
      "admin",
      {},
      {
        update_available: "0.36.0",
        update_check: checkState({ last_check: "newer" }),
      },
    );
    renderApp("/admin/general");

    const row = await updatesRow();
    expect(row).toHaveTextContent("Update check on. Last checked 3 hours ago.");
    expect(row).not.toHaveTextContent("does not offer");
    expect(within(row).getByText("0.36.0 is available.")).toBeInTheDocument();
    expect(
      within(row).getByRole("link", { name: /Release notes/ }),
    ).toHaveAttribute(
      "href",
      "https://github.com/jordiboehme/crystalline/releases/tag/v0.36.0",
    );
    expect(within(row).getByText("crystalline update now")).toBeInTheDocument();
  });

  it("says a newer release is out but not offered, and how to see why", async () => {
    serveAs("admin", {}, { update_check: checkState({ last_check: "newer" }) });
    renderApp("/admin/general");

    const row = await updatesRow();
    expect(row).toHaveTextContent(
      "A newer release is out, but this server does not offer it yet.",
    );
    expect(within(row).getByText("crystalline status")).toBeInTheDocument();
    expect(within(row).queryByText(/is available/)).not.toBeInTheDocument();
  });

  it("says a check that is not turned on is off, with the command to copy", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>(() =>
      Promise.resolve(),
    );
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    serveAs(
      "admin",
      {},
      {
        update_check: checkState({
          on: false,
          off_reason: "not_turned_on",
          last_checked: null,
          last_check: null,
        }),
      },
    );
    renderApp("/admin/general");

    const row = await updatesRow();
    expect(row).toHaveTextContent(
      /Update check off\.\s*Turn it on on the server:\s*crystalline update auto on/,
    );
    expect(row).not.toHaveTextContent("/etc/default/crystalline");
    // No switch: the check is changed on the server only.
    expect(within(row).queryByRole("switch")).not.toBeInTheDocument();
    expect(within(row).queryByRole("checkbox")).not.toBeInTheDocument();
    await userEvent.click(
      within(row).getByRole("button", { name: "Copy the command" }),
    );
    expect(writeText).toHaveBeenCalledWith("crystalline update auto on");
    await waitFor(() => {
      expect(within(row).getByRole("status")).toHaveTextContent("Copied");
    });
  });

  it("names /etc/default/crystalline for a .deb install", async () => {
    serveAs(
      "admin",
      {},
      {
        update_check: checkState({
          on: false,
          off_reason: "not_turned_on",
          install: "deb",
          last_checked: null,
          last_check: null,
        }),
      },
    );
    renderApp("/admin/general");

    const row = await updatesRow();
    expect(row).toHaveTextContent("crystalline update auto on");
    expect(row).toHaveTextContent(
      "The crystalline service of a .deb install reads its settings from /etc/default/crystalline: set CRYSTALLINE_UPDATE_CHECK=true there, then run: sudo systemctl restart crystalline",
    );
  });

  it.each([
    [
      "no_update_notifier",
      "Update check off: NO_UPDATE_NOTIFIER is set on the server. It keeps the check off whatever update.check says. Once it is removed, the check follows update.check: crystalline update auto on turns it on.",
    ],
    [
      "ci",
      "Update check off: CI is set on the server. It keeps the check off whatever update.check says. Once it is removed, the check follows update.check: crystalline update auto on turns it on.",
    ],
    [
      "docker",
      "Update check off: this server runs in Docker. To update, pull the new image and start the container again.",
    ],
    [
      "desktop_extension",
      "Update check off: this is the Claude Desktop-only bundle. To update, open the newer .mcpb file by hand.",
    ],
    [
      "no_state_folder",
      "Update check off: the server has no state folder to keep a check in.",
    ],
  ] as const)(
    "says why a check forced off is off: %s",
    async (reason, text) => {
      serveAs(
        "admin",
        {},
        {
          update_check: checkState({
            on: false,
            off_reason: reason,
            last_checked: null,
            last_check: null,
          }),
        },
      );
      renderApp("/admin/general");

      const row = await updatesRow();
      expect(row).toHaveTextContent(text);
      expect(
        within(row).queryByRole("button", { name: "Copy the command" }),
      ).not.toBeInTheDocument();
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
