/**
 * The account administration screen.
 *
 * What is pinned here is the part an admin has to be able to trust: that the
 * screen exists for nobody else, that a change to a role shows up before the
 * server has answered and un-shows itself when the server refuses, and that
 * the refusal on screen is the server's own sentence rather than a house
 * message pasted over it. NOT_LAST_ADMIN is the one that matters most: it is
 * the guard that keeps an instance administrable, and an admin who is told
 * "something went wrong" instead learns nothing about how to get past it.
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

/** Two accounts: the admin asking, and a disabled editor to act on. */
function usersListing() {
  return {
    users: [
      {
        name: "root",
        display: "Root",
        email: null,
        role: "admin",
        reflection: "default",
        disabled: false,
        last_seen: "2026-08-08T09:00:00Z",
      },
      {
        name: "eddy",
        display: "Eddy",
        email: null,
        role: "editor",
        reflection: "default",
        disabled: true,
        last_seen: null,
      },
      {
        name: "nora",
        display: "Nora",
        email: null,
        role: "none",
        reflection: "contributor",
        disabled: false,
        last_seen: null,
      },
    ],
  };
}

/** The app, signed in as `root` in the given role, with the listing served. */
function serveAs(
  role: "admin" | "editor",
  routes: Record<string, Answer> = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "root", role }) }),
      "/domains": domainsResponse,
      "/users": () => usersListing(),
      ...routes,
    }),
  );
}

/** The table row an account is drawn in. */
async function rowFor(name: string): Promise<HTMLElement> {
  const cell = await screen.findByText(name);
  const row = cell.closest("tr");
  if (!row) {
    throw new Error(`no row for ${name}`);
  }
  return row;
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

beforeEach(() => {
  apiMock.mockReset();
});

describe("the user admin screen", () => {
  it("ends a removed account's drafts only once the name is typed twice", async () => {
    const user = userEvent.setup();
    serveAs("admin", {
      "/users/ada/drafts": () => ({
        account: "ada",
        domains: [{ domain: "team", discarded: 2, left: [] }],
      }),
    });
    renderApp("/users");
    const end = await screen.findByRole("button", { name: "End these drafts" });
    expect(end).toBeDisabled();

    await user.type(screen.getByLabelText("Removed account's name"), "ada");
    expect(end).toBeDisabled();
    await user.type(screen.getByLabelText("Type the name again"), "adb");
    expect(end).toBeDisabled();
    await user.clear(screen.getByLabelText("Type the name again"));
    await user.type(screen.getByLabelText("Type the name again"), "ada");
    expect(end).toBeEnabled();
    await user.click(end);

    expect(await screen.findByText(/team: 2 ended/)).toBeVisible();
    expect(
      apiMock.mock.calls.some(
        ([sent, init]) =>
          sent === "/users/ada/drafts" && init?.method === "DELETE",
      ),
    ).toBe(true);
  });

  it("says what stayed when another instance hosts a domain or it failed half way", async () => {
    const user = userEvent.setup();
    serveAs("admin", {
      "/users/ada/drafts": () => ({
        account: "ada",
        domains: [
          {
            domain: "far",
            discarded: 1,
            left: [],
            not_hosted: true,
            where: "Another instance hosts this domain. Run this there",
          },
          { domain: "team", discarded: 3, left: [], error: "disk full" },
        ],
      }),
    });
    renderApp("/users");
    await user.type(
      await screen.findByLabelText("Removed account's name"),
      "ada",
    );
    await user.type(screen.getByLabelText("Type the name again"), "ada");
    await user.click(screen.getByRole("button", { name: "End these drafts" }));
    const said = await screen.findByText(/team: 3 ended/);
    expect(said).toHaveAttribute("role", "alert");
    expect(said).toHaveTextContent("Another instance hosts this domain");
    expect(said).toHaveTextContent("disk full");
  });

  it("shows the server's refusal for a name with an account", async () => {
    const user = userEvent.setup();
    serveAs("admin", {
      "/users/eddy/drafts": () => {
        throw new ApiProblem(409, "conflict", "'eddy' is an account");
      },
    });
    renderApp("/users");
    await user.type(
      await screen.findByLabelText("Removed account's name"),
      "eddy",
    );
    await user.type(screen.getByLabelText("Type the name again"), "eddy");
    await user.click(screen.getByRole("button", { name: "End these drafts" }));
    expect(await screen.findByText("'eddy' is an account")).toBeVisible();
  });

  it("sets the Reflection level beside the role, and shows curator for editors", async () => {
    serveAs("admin");
    renderApp("/users");

    const nora = await rowFor("nora");
    expect(
      within(nora).getByRole("combobox", { name: "Role for nora" }),
    ).toHaveValue("none");
    expect(
      within(nora).getByRole("combobox", { name: "Reflection for nora" }),
    ).toHaveValue("contributor");
    const eddy = await rowFor("eddy");
    expect(within(eddy).getByText("curator")).toBeVisible();
    expect(
      within(eddy).queryByRole("combobox", { name: "Reflection for eddy" }),
    ).toBeNull();
    const roles = within(
      within(nora).getByRole("combobox", { name: "Role for nora" }),
    )
      .getAllByRole("option")
      .map((option) => option.textContent);
    expect(roles).toEqual(["none", "viewer", "editor", "admin"]);
  });

  it("sends a Reflection change as a patch of its own", async () => {
    serveAs("admin", {
      "/users/nora": (_path, init) =>
        init?.method === "PATCH"
          ? { user: { ...usersListing().users[2], reflection: "reader" } }
          : usersListing(),
    });
    renderApp("/users");

    const nora = await rowFor("nora");
    await userEvent.selectOptions(
      within(nora).getByRole("combobox", { name: "Reflection for nora" }),
      "reader",
    );
    await waitFor(() => {
      expect(sentBody("/users/nora", "PATCH")).toEqual({
        reflection: "reader",
      });
    });
  });

  it("moves an account to none with a Reflection level in the same patch", async () => {
    serveAs("admin", {
      "/users/eddy": () => ({ user: usersListing().users[1] }),
    });
    renderApp("/users");

    const eddy = await rowFor("eddy");
    await userEvent.selectOptions(
      within(eddy).getByRole("combobox", { name: "Role for eddy" }),
      "none",
    );
    await waitFor(() => {
      expect(sentBody("/users/eddy", "PATCH")).toEqual({
        role: "none",
        reflection: "reader",
      });
    });
  });

  it("resets a chosen Reflection level when the new account's role curates", async () => {
    serveAs("admin", {
      "/users": (_path, init) =>
        init?.method === "POST"
          ? { user: usersListing().users[1] }
          : usersListing(),
    });
    renderApp("/users");

    await rowFor("nora");
    await userEvent.type(screen.getByLabelText("Login name"), "kim");
    await userEvent.type(screen.getByLabelText("Password"), "pw");
    await userEvent.selectOptions(
      screen.getByLabelText("Reflection"),
      "contributor",
    );
    await userEvent.selectOptions(screen.getByLabelText("Role"), "editor");
    expect(screen.getByLabelText("Reflection")).toHaveTextContent("curator");
    await userEvent.click(screen.getByRole("button", { name: "Add user" }));
    await waitFor(() => {
      expect(sentBody("/users", "POST")).toEqual({
        name: "kim",
        password: "pw",
        role: "editor",
      });
    });
  });

  it("adds an account at none with a Reflection level, never with nothing", async () => {
    serveAs("admin", {
      "/users": (_path, init) =>
        init?.method === "POST"
          ? { user: usersListing().users[2] }
          : usersListing(),
    });
    renderApp("/users");

    await rowFor("nora");
    await userEvent.type(screen.getByLabelText("Login name"), "kim");
    await userEvent.type(screen.getByLabelText("Password"), "pw");
    await userEvent.selectOptions(screen.getByLabelText("Role"), "none");
    expect(screen.getByLabelText("Reflection")).toHaveValue("reader");
    await userEvent.click(screen.getByRole("button", { name: "Add user" }));
    await waitFor(() => {
      expect(sentBody("/users", "POST")).toEqual({
        name: "kim",
        password: "pw",
        role: "none",
        reflection: "reader",
      });
    });
  });

  it("lists accounts with role, state and last seen", async () => {
    serveAs("admin");
    renderApp("/users");

    expect(
      await screen.findByRole("heading", { name: /users/i }),
    ).toBeInTheDocument();
    expect(await screen.findByText("eddy")).toBeInTheDocument();
    expect(screen.getByText(/disabled/i)).toBeInTheDocument();
    expect(screen.getAllByText(/never/i)).toHaveLength(2);
    expect(screen.getByText("2026-08-08")).toBeInTheDocument();
  });

  it("is not there for a non-admin", async () => {
    serveAs("editor");
    renderApp("/users");

    await waitFor(() => {
      expect(
        screen.queryByRole("heading", { name: /users/i }),
      ).not.toBeInTheDocument();
    });
    // The address leaks nothing: it is the not-found screen, not a shell with
    // an empty table in it.
    expect(
      await screen.findByText(/nothing is filed at this address/i),
    ).toBeInTheDocument();
    expect(
      apiMock.mock.calls.filter(([path]) => path === "/users"),
    ).toHaveLength(0);
    // And the frame does not advertise a screen this session cannot open.
    expect(
      screen.queryByRole("link", { name: /users/i }),
    ).not.toBeInTheDocument();
  });

  it("offers the nav entry to an admin", async () => {
    serveAs("admin");
    renderApp("/users");

    // Inside the identity menu rather than on an icon of its own: who you are
    // and the accounts only you may administer are one subject, and the frame
    // gathers them in one place.
    await userEvent.click(
      await screen.findByRole("button", { name: "Ada Lovelace" }),
    );
    expect(
      await screen.findByRole("menuitem", { name: "Users" }),
    ).toHaveAttribute("href", "/users");
  });

  it("teaches what to do when no account is listed", async () => {
    serveAs("admin", { "/users": () => ({ users: [] }) });
    renderApp("/users");

    expect(await screen.findByText(/no accounts yet/i)).toBeInTheDocument();
  });

  it("surfaces a NOT_LAST_ADMIN refusal in the server's words", async () => {
    serveAs("admin", {
      "/users/root": (_path, init) => {
        if (init?.method === "PATCH") {
          throw new ApiProblem(
            409,
            "conflict",
            "refusing to demote the last admin ('root'): add or enable another admin first",
          );
        }
        return usersListing();
      },
    });
    renderApp("/users");

    const row = await rowFor("root");
    const roleSelect = within(row).getByLabelText(/role/i);
    await userEvent.selectOptions(roleSelect, "viewer");

    expect(
      await screen.findByText(/add or enable another admin first/i),
    ).toBeInTheDocument();
    // And the optimistic change is taken back: the row says what the server
    // still holds, not what was asked for.
    await waitFor(() => {
      expect(roleSelect).toHaveValue("admin");
    });
  });

  it("shows a role change before the server has answered", async () => {
    // A patch the test holds open, so what the row shows in the meantime is
    // the optimistic copy rather than anything the server has said.
    const gate: { settle: () => void } = { settle: () => undefined };
    serveAs("admin", {
      "/users/eddy": (_path, init) => {
        if (init?.method === "PATCH") {
          return new Promise((resolve) => {
            gate.settle = () => {
              resolve({
                user: {
                  name: "eddy",
                  display: "Eddy",
                  email: null,
                  role: "admin",
                  disabled: true,
                  last_seen: null,
                },
              });
            };
          });
        }
        return usersListing();
      },
    });
    renderApp("/users");

    const row = await rowFor("eddy");
    const roleSelect = within(row).getByLabelText(/role/i);
    await userEvent.selectOptions(roleSelect, "admin");

    expect(roleSelect).toHaveValue("admin");
    gate.settle();
  });

  it("creates an account through the form", async () => {
    const created = vi.fn(() => ({
      user: {
        name: "bob",
        display: "Bob",
        email: null,
        role: "viewer",
        disabled: false,
        last_seen: null,
      },
    }));
    serveAs("admin", {
      "/users": (_path, init) =>
        init?.method === "POST" ? created() : usersListing(),
    });
    renderApp("/users");
    await rowFor("root");

    await userEvent.type(screen.getByLabelText(/login name/i), "bob");
    await userEvent.type(screen.getByLabelText(/password/i), "hunter2");
    await userEvent.click(screen.getByRole("button", { name: /add user/i }));

    await waitFor(() => {
      expect(created).toHaveBeenCalled();
    });
    expect(sentBody("/users", "POST")).toMatchObject({
      name: "bob",
      password: "hunter2",
      role: "viewer",
    });
  });

  it("keeps what was typed when the server refuses a new account", async () => {
    serveAs("admin", {
      "/users": (_path, init) => {
        if (init?.method === "POST") {
          throw new ApiProblem(
            409,
            "conflict",
            "an account named 'bob' already exists",
          );
        }
        return usersListing();
      },
    });
    renderApp("/users");
    await rowFor("root");

    const nameField = screen.getByLabelText(/login name/i);
    await userEvent.type(nameField, "bob");
    await userEvent.type(screen.getByLabelText(/password/i), "hunter2");
    await userEvent.click(screen.getByRole("button", { name: /add user/i }));

    expect(await screen.findByText(/already exists/i)).toBeInTheDocument();
    // The fix for a name in use is a different name, not typing it all again.
    expect(nameField).toHaveValue("bob");
  });

  it("asks twice before deleting", async () => {
    const deleted = vi.fn(() => undefined);
    serveAs("admin", {
      "/users/eddy": (_path, init) =>
        init?.method === "DELETE" ? deleted() : usersListing(),
    });
    renderApp("/users");
    await rowFor("eddy");

    await userEvent.click(screen.getByRole("button", { name: /delete eddy/i }));
    expect(deleted).not.toHaveBeenCalled();

    await userEvent.click(
      screen.getByRole("button", { name: /confirm delete/i }),
    );
    await waitFor(() => {
      expect(deleted).toHaveBeenCalled();
    });
  });

  it("takes back the confirmation on escape, with the focus", async () => {
    serveAs("admin");
    renderApp("/users");
    await rowFor("eddy");

    const trigger = screen.getByRole("button", { name: /delete eddy/i });
    await userEvent.click(trigger);
    expect(
      screen.getByRole("button", { name: /confirm delete/i }),
    ).toBeInTheDocument();

    await userEvent.keyboard("{Escape}");
    expect(
      screen.queryByRole("button", { name: /confirm delete/i }),
    ).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("reactivates a disabled account, and never offers the caller their own", async () => {
    serveAs("admin", {
      "/users/eddy": (_path, init) =>
        init?.method === "PATCH"
          ? {
              user: {
                name: "eddy",
                display: "Eddy",
                email: null,
                role: "editor",
                disabled: false,
                last_seen: null,
              },
            }
          : usersListing(),
    });
    renderApp("/users");
    await rowFor("eddy");

    // The caller cannot lock themselves out from here: the server refuses it,
    // and the screen does not offer it.
    expect(
      screen.queryByRole("button", { name: /deactivate root/i }),
    ).not.toBeInTheDocument();

    await userEvent.click(
      screen.getByRole("button", { name: /reactivate eddy/i }),
    );
    await waitFor(() => {
      expect(sentBody("/users/eddy", "PATCH")).toEqual({ disabled: false });
    });
  });

  it("replaces a password and says what that cost", async () => {
    const reset = vi.fn(() => ({
      user: {
        name: "eddy",
        display: "Eddy",
        email: null,
        role: "editor",
        disabled: true,
        last_seen: null,
      },
    }));
    serveAs("admin", { "/users/eddy/password": () => reset() });
    renderApp("/users");
    const row = await rowFor("eddy");

    await userEvent.click(
      within(row).getByRole("button", { name: /reset password/i }),
    );
    await userEvent.type(
      screen.getByLabelText(/new password for eddy/i),
      "correct horse",
    );
    await userEvent.click(
      screen.getByRole("button", { name: /set password for eddy/i }),
    );

    await waitFor(() => {
      expect(reset).toHaveBeenCalled();
    });
    expect(await screen.findByText(/signed out/i)).toBeInTheDocument();
  });

  it("renames an account's display through the row", async () => {
    serveAs("admin", {
      "/users/eddy": (_path, init) =>
        init?.method === "PATCH"
          ? {
              user: {
                name: "eddy",
                display: "Edward",
                email: null,
                role: "editor",
                disabled: true,
                last_seen: null,
              },
            }
          : usersListing(),
    });
    renderApp("/users");
    const row = await rowFor("eddy");

    const display = within(row).getByLabelText(/display name for eddy/i);
    await userEvent.clear(display);
    await userEvent.type(display, "Edward");
    await userEvent.click(
      within(row).getByRole("button", { name: /save display for eddy/i }),
    );

    await waitFor(() => {
      expect(sentBody("/users/eddy", "PATCH")).toEqual({ display: "Edward" });
    });
  });
});
