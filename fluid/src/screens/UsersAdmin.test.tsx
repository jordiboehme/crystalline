/**
 * The account administration screen, built for hundreds of accounts.
 *
 * What is pinned here is the part an admin has to be able to trust: that the
 * screen exists for nobody else; that the search, filters, sort and page live
 * in the address and a bad value there falls back quietly; that the table
 * holds no edit field and a row opens the account at its own address; that a
 * change shows before the server has answered and un-shows itself when the
 * server refuses, in the server's own sentence (NOT_LAST_ADMIN above all);
 * and that a bulk change asks with the count first and reports every account,
 * refusals first.
 */

import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { User } from "../api/model";
import type { Answer } from "../test/harness";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";
import {
  DEFAULT_USERS_REQUEST,
  narrowed,
  readUsersSearch,
  writeUsersSearch,
} from "./usersQuery";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

/** The admin asking, a disabled editor with a GitHub link, and a none. */
function accounts(): User[] {
  return [
    {
      name: "root",
      display: "Root",
      email: null,
      role: "admin",
      reflection: "default",
      disabled: false,
      last_seen: "2026-08-08T09:00:00Z",
      sign_in: "password",
      github_linked: false,
    },
    {
      name: "eddy",
      display: "Eddy",
      email: "eddy@example.com",
      role: "editor",
      reflection: "default",
      disabled: true,
      last_seen: null,
      sign_in: "oidc",
      github_linked: true,
    },
    {
      name: "nora",
      display: "Nora",
      email: null,
      role: "none",
      reflection: "contributor",
      disabled: false,
      last_seen: null,
      sign_in: "token_only",
      github_linked: false,
    },
  ];
}

function account(name: string): User {
  const found = accounts().find((row) => row.name === name);
  if (!found) throw new Error(`no fixture ${name}`);
  return found;
}

/**
 * The listing as the server would answer the path: a `q` keeps the accounts
 * whose name starts with it, anything else answers every account.
 */
function listingFor(path: string, rows: User[] = accounts()) {
  const q = new URLSearchParams(path.split("?")[1] ?? "").get("q");
  const users = q === null ? rows : rows.filter((u) => u.name.startsWith(q));
  return { users, total: users.length };
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
      "/users": (path) => listingFor(path),
      ...routes,
    }),
  );
}

/** Every listing request the screen sent, as its query parameters. */
function listingCalls(): URLSearchParams[] {
  return apiMock.mock.calls
    .map(([path]) => path)
    .filter((path) => path.startsWith("/users?") && path.includes("limit="))
    .map((path) => new URLSearchParams(path.split("?")[1]));
}

/** The last listing request, as its query parameters. */
function lastListing(): URLSearchParams {
  const last = listingCalls().at(-1);
  if (!last) throw new Error("no listing request");
  return last;
}

/** The table row an account is drawn in. */
async function rowFor(name: string): Promise<HTMLElement> {
  const table = await screen.findByRole("table");
  const link = await within(table).findByRole("link", { name });
  const row = link.closest("tr");
  if (!row) throw new Error(`no row for ${name}`);
  return row;
}

/** The detail panel, once it shows the account. */
async function panelFor(name: string): Promise<HTMLElement> {
  const back = await screen.findByRole("link", { name: "Back to users" });
  const panel = back.closest("section");
  if (!panel) throw new Error("no panel");
  await within(panel).findByText(name);
  return panel;
}

/** Open an account from its row. */
async function openFromRow(name: string): Promise<HTMLElement> {
  await userEvent.click(within(await rowFor(name)).getByRole("link", { name }));
  return panelFor(name);
}

/** The users screen's own search field. */
function searchField(): HTMLElement {
  return within(
    screen.getByRole("search", { name: "Find accounts" }),
  ).getByRole("searchbox");
}

/** The form that adds an account. */
function createForm(): HTMLElement {
  return screen.getByRole("region", { name: "Add an account" });
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

describe("the users address", () => {
  it("reads every value it knows and writes back only what is not a default", () => {
    const search =
      "q=ed&role=editor&state=disabled&kind=token_only&github=true&seen_after=2026-01-02&seen_before=2026-03-04&sort=last_seen&dir=desc&page=3&size=25";
    const read = readUsersSearch(new URLSearchParams(search));
    expect(read).toEqual({
      q: "ed",
      role: "editor",
      state: "disabled",
      kind: "token_only",
      github: true,
      seenAfter: "2026-01-02",
      seenBefore: "2026-03-04",
      sort: "last_seen",
      dir: "desc",
      page: 3,
      size: 25,
    });
    expect(writeUsersSearch(read).toString()).toBe(search);
    expect(writeUsersSearch(DEFAULT_USERS_REQUEST).toString()).toBe("");
  });

  it("falls back to the defaults for anything it does not know", () => {
    expect(
      readUsersSearch(
        new URLSearchParams(
          "role=god&state=gone&kind=magic&github=yes&seen_after=2026-02-30&seen_before=soon&sort=age&dir=up&page=-1&size=7",
        ),
      ),
    ).toEqual(DEFAULT_USERS_REQUEST);
    expect(readUsersSearch(new URLSearchParams("page=0")).page).toBe(1);
    expect(readUsersSearch(new URLSearchParams("page=2.5")).page).toBe(1);
  });

  it("starts again at the first page when what is listed changes", () => {
    const onPage4 = { ...DEFAULT_USERS_REQUEST, page: 4 };
    expect(narrowed(onPage4, { role: "admin" })).toMatchObject({
      role: "admin",
      page: 1,
    });
  });
});

describe("the users table", () => {
  it("lists accounts with role, state, sign-in and last seen, and no edit field", async () => {
    serveAs("admin");
    renderApp("/admin/users");

    const eddy = await rowFor("eddy");
    expect(within(eddy).getByText("Disabled")).toBeInTheDocument();
    expect(within(eddy).getByText("SSO")).toBeInTheDocument();
    expect(within(eddy).getByText("Never")).toBeInTheDocument();
    const nora = await rowFor("nora");
    expect(within(nora).getByText("Token only")).toBeInTheDocument();
    const root = await rowFor("root");
    expect(within(root).getByText("2026-08-08")).toBeInTheDocument();
    const table = screen.getByRole("table");
    expect(within(table).queryAllByRole("combobox")).toHaveLength(0);
    expect(within(table).queryAllByRole("textbox")).toHaveLength(0);
    expect(lastListing().toString()).toBe("sort=name&dir=asc&page=1&limit=50");
  });

  it("asks the server for what the address says", async () => {
    serveAs("admin");
    renderApp(
      "/admin/users?q=ed&role=editor&kind=oidc&github=true&seen_after=2026-01-02&sort=last_seen&dir=desc&page=2&size=25",
    );

    await waitFor(() => {
      expect(listingCalls().length).toBeGreaterThan(0);
    });
    const sent = listingCalls()[0];
    expect(sent?.get("q")).toBe("ed");
    expect(sent?.get("role")).toBe("editor");
    expect(sent?.get("kind")).toBe("oidc");
    expect(sent?.get("github")).toBe("true");
    expect(sent?.get("seen_after")).toBe("2026-01-02T00:00:00Z");
    expect(sent?.get("sort")).toBe("last_seen");
    expect(sent?.get("dir")).toBe("desc");
    expect(sent?.get("page")).toBe("2");
    expect(sent?.get("limit")).toBe("25");
    expect(searchField()).toHaveValue("ed");
    // Sorting by last seen says that paging can shift.
    expect(
      screen.getByText(/an account that signs in while you page can move/),
    ).toBeInTheDocument();
  });

  it("falls back to the defaults without an error for a bad address", async () => {
    serveAs("admin");
    renderApp("/admin/users?sort=age&page=nope&size=7&role=god");

    await rowFor("eddy");
    expect(listingCalls()[0]?.toString()).toBe(
      "sort=name&dir=asc&page=1&limit=50",
    );
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("sorts by a column header, and flips the direction on a second click", async () => {
    const user = userEvent.setup();
    serveAs("admin");
    renderApp("/admin/users");
    await rowFor("eddy");

    await user.click(screen.getByRole("button", { name: "Display name" }));
    await waitFor(() => {
      expect(lastListing().get("sort")).toBe("display");
    });
    expect(lastListing().get("dir")).toBe("asc");
    expect(
      screen.getByRole("button", { name: "Display name" }).closest("th"),
    ).toHaveAttribute("aria-sort", "ascending");

    await user.click(screen.getByRole("button", { name: "Display name" }));
    await waitFor(() => {
      expect(lastListing().get("dir")).toBe("desc");
    });
    expect(
      screen.getByRole("button", { name: "Display name" }).closest("th"),
    ).toHaveAttribute("aria-sort", "descending");

    // Last seen starts with the most recent.
    await user.click(screen.getByRole("button", { name: "Last seen" }));
    await waitFor(() => {
      expect(lastListing().get("sort")).toBe("last_seen");
    });
    expect(lastListing().get("dir")).toBe("desc");
  });

  it("offers a sort on every column", async () => {
    serveAs("admin");
    renderApp("/admin/users");
    const table = await screen.findByRole("table");
    for (const name of [
      "Name",
      "Display name",
      "Role",
      "State",
      "Sign-in",
      "Last seen",
    ]) {
      expect(within(table).getByRole("button", { name })).toBeInTheDocument();
    }
  });

  it("searches after a pause, and from the first page", async () => {
    const user = userEvent.setup();
    serveAs("admin");
    renderApp("/admin/users?page=3");
    await screen.findByRole("search", { name: "Find accounts" });

    await user.type(searchField(), "ed");
    // Not yet: the list waits for the typing to stop.
    expect(listingCalls().some((call) => call.get("q") === "ed")).toBe(false);
    await waitFor(() => {
      expect(lastListing().get("q")).toBe("ed");
    });
    expect(lastListing().get("page")).toBe("1");
    // The address changed under the field, and the typing goes on there.
    expect(searchField()).toHaveFocus();
  });

  it("searches at once on Enter", async () => {
    const user = userEvent.setup();
    serveAs("admin");
    renderApp("/admin/users");
    await screen.findByRole("search", { name: "Find accounts" });

    await user.type(searchField(), "no{Enter}");
    // At once, without the pause the typing waits for.
    expect(lastListing().get("q")).toBe("no");
  });

  it("filters by role, state, sign-in, GitHub and last seen", async () => {
    const user = userEvent.setup();
    serveAs("admin");
    renderApp("/admin/users?page=2");
    const find = await screen.findByRole("search", { name: "Find accounts" });

    await user.selectOptions(within(find).getByLabelText("Role"), "editor");
    await waitFor(() => {
      expect(lastListing().get("role")).toBe("editor");
    });
    expect(lastListing().get("page")).toBe("1");
    await user.selectOptions(within(find).getByLabelText("State"), "active");
    await user.selectOptions(
      within(find).getByLabelText("Sign-in"),
      "token_only",
    );
    await user.selectOptions(within(find).getByLabelText("GitHub"), "false");
    await user.type(within(find).getByLabelText("Seen before"), "2026-05-06");
    await waitFor(() => {
      expect(lastListing().get("seen_before")).toBe("2026-05-06T00:00:00Z");
    });
    const sent = lastListing();
    expect(sent.get("state")).toBe("active");
    expect(sent.get("kind")).toBe("token_only");
    expect(sent.get("github")).toBe("false");

    await user.click(
      within(find).getByRole("button", { name: "Clear filters" }),
    );
    await waitFor(() => {
      expect(lastListing().toString()).toBe(
        "sort=name&dir=asc&page=1&limit=50",
      );
    });
  });

  it("pages with the total, and changes the page size", async () => {
    const user = userEvent.setup();
    const many = Array.from({ length: 50 }, (_, index) => ({
      ...account("nora"),
      name: `user${String(index).padStart(3, "0")}`,
    }));
    serveAs("admin", {
      "/users": () => ({ users: many, total: 120 }),
    });
    renderApp("/admin/users");

    const pages = await screen.findByRole("navigation", { name: "Pages" });
    expect(within(pages).getByText("1 to 50 of 120")).toBeInTheDocument();
    expect(within(pages).getByText("Page 1 of 3")).toBeInTheDocument();
    expect(
      within(pages).getByRole("button", { name: "Previous page" }),
    ).toBeDisabled();

    await user.click(within(pages).getByRole("button", { name: "Next page" }));
    await waitFor(() => {
      expect(lastListing().get("page")).toBe("2");
    });
    await user.selectOptions(within(pages).getByLabelText("Per page"), "25");
    await waitFor(() => {
      expect(lastListing().get("limit")).toBe("25");
    });
    expect(lastListing().get("page")).toBe("1");
  });

  it("goes to the last page there is when the address asks past the end", async () => {
    serveAs("admin");
    renderApp("/admin/users?page=9");

    await waitFor(() => {
      expect(lastListing().get("page")).toBe("1");
    });
    await rowFor("eddy");
  });

  it("says when nothing matches", async () => {
    serveAs("admin", { "/users": () => ({ users: [], total: 0 }) });
    renderApp("/admin/users?q=zz");
    expect(await screen.findByText("No account matches.")).toBeInTheDocument();
  });

  it("teaches what to do when no account is listed", async () => {
    serveAs("admin", { "/users": () => ({ users: [], total: 0 }) });
    renderApp("/admin/users");

    expect(await screen.findByText(/no accounts yet/i)).toBeInTheDocument();
  });

  it("is not there for a non-admin", async () => {
    serveAs("editor");
    renderApp("/admin/users");

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
      apiMock.mock.calls.filter(([path]) => path.startsWith("/users")),
    ).toHaveLength(0);
    // And the frame does not advertise a screen this session cannot open.
    expect(
      screen.queryByRole("link", { name: /users/i }),
    ).not.toBeInTheDocument();
  });

  it("is not there for a non-admin at an account's address either", async () => {
    serveAs("editor");
    renderApp("/admin/users/eddy");

    expect(
      await screen.findByText(/nothing is filed at this address/i),
    ).toBeInTheDocument();
    expect(
      apiMock.mock.calls.filter(([path]) => path.startsWith("/users")),
    ).toHaveLength(0);
  });

  it("offers the nav entry to an admin", async () => {
    serveAs("admin");
    renderApp("/admin/users");

    // Inside the identity menu rather than on an icon of its own: who you are
    // and the accounts only you may administer are one subject, and the frame
    // gathers them in one place.
    await userEvent.click(
      await screen.findByRole("button", { name: "Ada Lovelace" }),
    );
    expect(
      await screen.findByRole("menuitem", { name: "Users" }),
    ).toHaveAttribute("href", "/admin/users");
  });
});

describe("an account's detail panel", () => {
  it("opens from the row at its own address, keeping the list's search", async () => {
    serveAs("admin");
    renderApp("/admin/users?role=editor");

    const row = await rowFor("eddy");
    expect(within(row).getByRole("link", { name: "eddy" })).toHaveAttribute(
      "href",
      "/admin/users/eddy?role=editor",
    );
    const panel = await openFromRow("eddy");
    expect(
      within(panel).getByRole("heading", { name: "Eddy" }),
    ).toBeInTheDocument();
    expect(
      within(panel).getByRole("link", { name: "Back to users" }),
    ).toHaveAttribute("href", "/admin/users?role=editor");
    expect(within(panel).getByText("SSO")).toBeInTheDocument();
    expect(within(panel).getByText("eddy@example.com")).toBeInTheDocument();
    // The Users entry of the area stays the current page.
    expect(
      screen
        .getByRole("navigation", { name: "Administration" })
        .querySelector('[aria-current="page"]'),
    ).toHaveTextContent("Users");
  });

  it("finds a linked account by its exact name", async () => {
    const eddy2 = { ...account("eddy"), name: "eddy2", display: "Eddy Two" };
    serveAs("admin", {
      "/users": (path) => listingFor(path, [eddy2, ...accounts()]),
    });
    renderApp("/admin/users/eddy");

    const panel = await panelFor("eddy");
    expect(
      within(panel).getByRole("heading", { name: "Eddy" }),
    ).toBeInTheDocument();
    expect(apiMock).toHaveBeenCalledWith("/users?q=eddy");
  });

  it("says when there is no such account", async () => {
    serveAs("admin");
    renderApp("/admin/users/ghost");

    expect(
      await screen.findByRole("heading", { name: "No such account" }),
    ).toBeInTheDocument();
    expect(screen.getByText("There is no account named ghost.")).toBeVisible();
  });

  it("sets the Reflection level beside the role, and shows curator for editors", async () => {
    serveAs("admin");
    renderApp("/admin/users/nora");

    const nora = await panelFor("nora");
    expect(
      within(nora).getByRole("combobox", { name: "Role for nora" }),
    ).toHaveValue("none");
    expect(
      within(nora).getByRole("combobox", { name: "Reflection for nora" }),
    ).toHaveValue("contributor");
    const roles = within(
      within(nora).getByRole("combobox", { name: "Role for nora" }),
    )
      .getAllByRole("option")
      .map((option) => option.textContent);
    expect(roles).toEqual(["none", "viewer", "editor", "admin"]);

    const eddy = await openFromRow("eddy");
    expect(within(eddy).getByText("curator")).toBeVisible();
    expect(
      within(eddy).queryByRole("combobox", { name: "Reflection for eddy" }),
    ).toBeNull();
  });

  it("sends a Reflection change as a patch of its own", async () => {
    serveAs("admin", {
      "/users/nora": () => ({
        user: { ...account("nora"), reflection: "reader" },
      }),
    });
    renderApp("/admin/users/nora");

    const nora = await panelFor("nora");
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
      "/users/eddy": () => ({ user: account("eddy") }),
    });
    renderApp("/admin/users/eddy");

    const eddy = await panelFor("eddy");
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

  it("surfaces a NOT_LAST_ADMIN refusal in the server's words", async () => {
    serveAs("admin", {
      "/users/root": () => {
        throw new ApiProblem(
          409,
          "conflict",
          "refusing to demote the last admin ('root'): add or enable another admin first",
        );
      },
    });
    renderApp("/admin/users/root");

    const panel = await panelFor("root");
    const roleSelect = within(panel).getByRole("combobox", {
      name: "Role for root",
    });
    await userEvent.selectOptions(roleSelect, "viewer");

    expect(
      await screen.findByText(/add or enable another admin first/i),
    ).toBeInTheDocument();
    // And the optimistic change is taken back: the panel and the row say what
    // the server still holds, not what was asked for.
    await waitFor(() => {
      expect(roleSelect).toHaveValue("admin");
    });
    expect(within(await rowFor("root")).getByText("admin")).toBeInTheDocument();
  });

  it("takes a refused change back even before the panel has read the account", async () => {
    serveAs("admin", {
      // The panel's own read never answers, so it shows the table's row.
      "/users": (path) =>
        path.includes("limit=")
          ? listingFor(path)
          : new Promise(() => undefined),
      "/users/root": () => {
        throw new ApiProblem(
          409,
          "conflict",
          "add or enable another admin first",
        );
      },
    });
    renderApp("/admin/users/root");

    const panel = await panelFor("root");
    const roleSelect = within(panel).getByRole("combobox", {
      name: "Role for root",
    });
    await userEvent.selectOptions(roleSelect, "viewer");
    expect(await screen.findByText(/another admin first/)).toBeInTheDocument();
    await waitFor(() => {
      expect(roleSelect).toHaveValue("admin");
    });
  });

  it("shows a role change in the panel and the row before the server has answered", async () => {
    // A patch the test holds open, so what the screen shows in the meantime
    // is the optimistic copy rather than anything the server has said.
    const gate: { settle: () => void } = { settle: () => undefined };
    serveAs("admin", {
      "/users/eddy": () =>
        new Promise((resolve) => {
          gate.settle = () => {
            resolve({ user: { ...account("eddy"), role: "admin" } });
          };
        }),
    });
    renderApp("/admin/users/eddy");

    const panel = await panelFor("eddy");
    const roleSelect = within(panel).getByRole("combobox", {
      name: "Role for eddy",
    });
    await userEvent.selectOptions(roleSelect, "admin");

    expect(roleSelect).toHaveValue("admin");
    expect(within(await rowFor("eddy")).getByText("admin")).toBeInTheDocument();
    act(() => {
      gate.settle();
    });
  });

  it("reactivates a disabled account, and never offers the caller their own", async () => {
    serveAs("admin", {
      "/users/eddy": () => ({ user: { ...account("eddy"), disabled: false } }),
    });
    renderApp("/admin/users/root");
    await panelFor("root");

    // The caller cannot lock themselves out from here: the server refuses it,
    // and the screen does not offer it.
    expect(
      screen.queryByRole("button", { name: /deactivate root/i }),
    ).not.toBeInTheDocument();

    await openFromRow("eddy");
    await userEvent.click(
      screen.getByRole("button", { name: "Reactivate eddy" }),
    );
    await waitFor(() => {
      expect(sentBody("/users/eddy", "PATCH")).toEqual({ disabled: false });
    });
  });

  it("replaces a password and says what that cost", async () => {
    const reset = vi.fn(() => ({ user: account("eddy") }));
    serveAs("admin", { "/users/eddy/password": () => reset() });
    renderApp("/admin/users/eddy");
    const panel = await panelFor("eddy");

    await userEvent.click(
      within(panel).getByRole("button", { name: "Reset password" }),
    );
    await userEvent.type(
      screen.getByLabelText("New password for eddy"),
      "correct horse",
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Set password for eddy" }),
    );

    await waitFor(() => {
      expect(reset).toHaveBeenCalled();
    });
    expect(sentBody("/users/eddy/password", "POST")).toEqual({
      password: "correct horse",
    });
    expect(await screen.findByText(/signed out/i)).toBeInTheDocument();
  });

  it("renames an account's display", async () => {
    serveAs("admin", {
      "/users/eddy": () => ({
        user: { ...account("eddy"), display: "Edward" },
      }),
    });
    renderApp("/admin/users/eddy");
    const panel = await panelFor("eddy");

    const save = within(panel).getByRole("button", {
      name: "Save display for eddy",
    });
    expect(save).toBeDisabled();
    const display = within(panel).getByRole("textbox", {
      name: "Display name for eddy",
    });
    await userEvent.clear(display);
    await userEvent.type(display, "Edward");
    await userEvent.click(save);

    await waitFor(() => {
      expect(sentBody("/users/eddy", "PATCH")).toEqual({ display: "Edward" });
    });
  });

  it("unlinks GitHub and passes on the warning", async () => {
    serveAs("admin", {
      "/users/eddy/github-link": () => ({
        linked: false,
        unlinked: true,
        warning: "unlinked here; revoke it in your GitHub settings",
      }),
    });
    renderApp("/admin/users/eddy");
    const panel = await panelFor("eddy");
    expect(within(panel).getByText("Linked")).toBeInTheDocument();

    await userEvent.click(
      within(panel).getByRole("button", { name: "Unlink eddy from GitHub" }),
    );
    expect(
      await screen.findByText(/revoke it in your GitHub settings/),
    ).toHaveAttribute("role", "alert");
    expect(
      apiMock.mock.calls.some(
        ([path, init]) =>
          path === "/users/eddy/github-link" && init?.method === "DELETE",
      ),
    ).toBe(true);
  });

  it("offers no unlink for an account without a GitHub link", async () => {
    serveAs("admin");
    renderApp("/admin/users/nora");
    const panel = await panelFor("nora");
    expect(within(panel).getByText("Not linked")).toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: /unlink/i })).toBeNull();
  });

  it("names how to set the write list", async () => {
    serveAs("admin");
    renderApp("/admin/users/nora");
    const panel = await panelFor("nora");
    expect(
      within(panel).getByText("crystalline users writes nora"),
    ).toBeInTheDocument();
  });

  it("asks twice before deleting, then goes back to the list", async () => {
    const deleted = vi.fn(() => undefined);
    serveAs("admin", {
      "/users/eddy": (_path, init) =>
        init?.method === "DELETE" ? deleted() : undefined,
    });
    renderApp("/admin/users/eddy?role=editor");
    const panel = await panelFor("eddy");

    await userEvent.click(
      within(panel).getByRole("button", { name: "Delete eddy" }),
    );
    expect(deleted).not.toHaveBeenCalled();

    await userEvent.click(
      screen.getByRole("button", { name: "Confirm delete of eddy" }),
    );
    await waitFor(() => {
      expect(deleted).toHaveBeenCalled();
    });
    expect(await screen.findByText("Deleted eddy.")).toBeInTheDocument();
    await waitFor(() => {
      expect(
        screen.queryByRole("link", { name: "Back to users" }),
      ).not.toBeInTheDocument();
    });
  });

  it("takes back the confirmation on escape, with the focus", async () => {
    serveAs("admin");
    renderApp("/admin/users/eddy");
    await panelFor("eddy");

    const trigger = screen.getByRole("button", { name: "Delete eddy" });
    await userEvent.click(trigger);
    expect(
      screen.getByRole("button", { name: "Confirm delete of eddy" }),
    ).toBeInTheDocument();

    await userEvent.keyboard("{Escape}");
    expect(
      screen.queryByRole("button", { name: "Confirm delete of eddy" }),
    ).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });
});

describe("bulk changes", () => {
  it("selects accounts on the page, but never the caller's own", async () => {
    const user = userEvent.setup();
    serveAs("admin");
    renderApp("/admin/users");
    const root = await rowFor("root");
    expect(within(root).queryByRole("checkbox")).toBeNull();

    await user.click(
      screen.getByRole("checkbox", {
        name: "Select every account on this page",
      }),
    );
    expect(screen.getByRole("checkbox", { name: "Select eddy" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Select nora" })).toBeChecked();
    expect(screen.getByText("2 accounts selected")).toBeInTheDocument();

    await user.click(screen.getByRole("checkbox", { name: "Select nora" }));
    expect(screen.getByText("1 account selected")).toBeInTheDocument();
    expect(
      screen.getByRole("checkbox", {
        name: "Select every account on this page",
      }),
    ).toHaveProperty("indeterminate", true);
  });

  it("asks with the count, then reports every account, refusals first", async () => {
    const user = userEvent.setup();
    serveAs("admin", {
      "/users/bulk": () => ({
        results: [
          { name: "eddy", done: true },
          {
            name: "nora",
            done: false,
            message: "refusing to disable the last admin",
          },
        ],
      }),
    });
    renderApp("/admin/users");
    await rowFor("eddy");
    await user.click(screen.getByRole("checkbox", { name: "Select eddy" }));
    await user.click(screen.getByRole("checkbox", { name: "Select nora" }));

    const bar = screen.getByRole("region", { name: "Selected accounts" });
    await user.click(within(bar).getByRole("button", { name: "Deactivate" }));
    expect(within(bar).getByText("Deactivate for 2 accounts?")).toBeVisible();
    expect(apiMock.mock.calls.some(([path]) => path === "/users/bulk")).toBe(
      false,
    );
    await user.click(
      within(bar).getByRole("button", { name: "Deactivate 2 accounts" }),
    );

    await waitFor(() => {
      expect(sentBody("/users/bulk", "POST")).toEqual({
        names: ["eddy", "nora"],
        action: "deactivate",
      });
    });
    const results = await screen.findByRole("alert", {
      name: "Bulk results",
    });
    expect(
      within(results).getByText("Deactivate: 1 done, 1 refused."),
    ).toBeVisible();
    const items = within(results)
      .getAllByRole("listitem")
      .map((item) => item.textContent);
    expect(items).toEqual([
      "nora: refusing to disable the last admin",
      "eddy: done",
    ]);
    // The selection is used up.
    expect(
      screen.getByRole("checkbox", { name: "Select eddy" }),
    ).not.toBeChecked();
  });

  it("changes the role of the selection", async () => {
    const user = userEvent.setup();
    serveAs("admin", {
      "/users/bulk": () => ({ results: [{ name: "nora", done: true }] }),
    });
    renderApp("/admin/users");
    await rowFor("nora");
    await user.click(screen.getByRole("checkbox", { name: "Select nora" }));

    const bar = screen.getByRole("region", { name: "Selected accounts" });
    await user.selectOptions(
      within(bar).getByLabelText("New role for the selected accounts"),
      "editor",
    );
    await user.click(within(bar).getByRole("button", { name: "Set role" }));
    expect(
      within(bar).getByText("Set role editor for 1 account?"),
    ).toBeVisible();
    await user.click(
      within(bar).getByRole("button", { name: "Set role editor 1 account" }),
    );
    await waitFor(() => {
      expect(sentBody("/users/bulk", "POST")).toEqual({
        names: ["nora"],
        action: "role",
        role: "editor",
      });
    });
    expect(
      await screen.findByRole("status", { name: "Bulk results" }),
    ).toHaveTextContent("Set role editor: 1 done.");
  });

  it("asks nothing when the confirm is cancelled", async () => {
    const user = userEvent.setup();
    serveAs("admin");
    renderApp("/admin/users");
    await rowFor("eddy");
    await user.click(screen.getByRole("checkbox", { name: "Select eddy" }));
    const bar = screen.getByRole("region", { name: "Selected accounts" });
    await user.click(within(bar).getByRole("button", { name: "Reactivate" }));
    await user.click(within(bar).getByRole("button", { name: "Cancel" }));
    expect(within(bar).queryByText(/for 1 account\?/)).toBeNull();
    expect(apiMock.mock.calls.some(([path]) => path === "/users/bulk")).toBe(
      false,
    );
  });

  it("never counts or sends a tick that is not on the page shown", async () => {
    const user = userEvent.setup();
    const later: { answer: () => void } = { answer: () => undefined };
    const other = { ...account("nora"), name: "olga" };
    serveAs("admin", {
      "/users": (path) => {
        if (new URLSearchParams(path.split("?")[1]).get("page") !== "2") {
          return { users: accounts(), total: 120 };
        }
        return new Promise((resolve) => {
          later.answer = () => {
            resolve({ users: [other], total: 120 });
          };
        });
      },
    });
    renderApp("/admin/users");
    await rowFor("eddy");
    await user.click(screen.getByRole("button", { name: "Next page" }));
    await waitFor(() => {
      expect(lastListing().get("page")).toBe("2");
    });
    // The first page is still drawn while the second loads; a tick there
    // belongs to no row of the page that arrives.
    await user.click(screen.getByRole("checkbox", { name: "Select nora" }));
    act(() => {
      later.answer();
    });
    await rowFor("olga");
    expect(screen.queryByText(/selected$/)).toBeNull();
  });

  it("starts with nothing selected on another page or sort", async () => {
    const user = userEvent.setup();
    serveAs("admin");
    renderApp("/admin/users");
    await rowFor("eddy");
    await user.click(screen.getByRole("checkbox", { name: "Select eddy" }));
    expect(screen.getByText("1 account selected")).toBeInTheDocument();

    await user.click(
      within(screen.getByRole("table")).getByRole("button", { name: "Role" }),
    );
    await waitFor(() => {
      expect(lastListing().get("sort")).toBe("role");
    });
    expect(screen.queryByText("1 account selected")).toBeNull();
    expect(
      screen.getByRole("checkbox", { name: "Select eddy" }),
    ).not.toBeChecked();
  });
});

describe("adding accounts and ending drafts", () => {
  it("creates an account through the form and opens it", async () => {
    const created = vi.fn(() => ({
      user: { ...account("nora"), name: "bob" },
    }));
    serveAs("admin", {
      "/users": (path, init) =>
        init?.method === "POST" ? created() : listingFor(path),
    });
    renderApp("/admin/users");
    await rowFor("root");

    const form = createForm();
    await userEvent.type(within(form).getByLabelText("Login name"), "bob");
    await userEvent.type(within(form).getByLabelText("Password"), "hunter2");
    await userEvent.click(
      within(form).getByRole("button", { name: "Add user" }),
    );

    await waitFor(() => {
      expect(created).toHaveBeenCalled();
    });
    expect(sentBody("/users", "POST")).toMatchObject({
      name: "bob",
      password: "hunter2",
      role: "viewer",
    });
    expect(await screen.findByText("Added bob.")).toBeInTheDocument();
    await waitFor(() => {
      expect(apiMock).toHaveBeenCalledWith("/users?q=bob");
    });
  });

  it("keeps what was typed when the server refuses a new account", async () => {
    serveAs("admin", {
      "/users": (path, init) => {
        if (init?.method === "POST") {
          throw new ApiProblem(
            409,
            "conflict",
            "an account named 'bob' already exists",
          );
        }
        return listingFor(path);
      },
    });
    renderApp("/admin/users");
    await rowFor("root");

    const form = createForm();
    const nameField = within(form).getByLabelText("Login name");
    await userEvent.type(nameField, "bob");
    await userEvent.type(within(form).getByLabelText("Password"), "hunter2");
    await userEvent.click(
      within(form).getByRole("button", { name: "Add user" }),
    );

    expect(await screen.findByText(/already exists/i)).toBeInTheDocument();
    // The fix for a name in use is a different name, not typing it all again.
    expect(nameField).toHaveValue("bob");
  });

  it("resets a chosen Reflection level when the new account's role curates", async () => {
    serveAs("admin", {
      "/users": (path, init) =>
        init?.method === "POST" ? { user: account("eddy") } : listingFor(path),
    });
    renderApp("/admin/users");

    await rowFor("nora");
    const form = createForm();
    await userEvent.type(within(form).getByLabelText("Login name"), "kim");
    await userEvent.type(within(form).getByLabelText("Password"), "pw");
    await userEvent.selectOptions(
      within(form).getByLabelText("Reflection"),
      "contributor",
    );
    await userEvent.selectOptions(
      within(form).getByLabelText("Role"),
      "editor",
    );
    expect(within(form).getByLabelText("Reflection")).toHaveTextContent(
      "curator",
    );
    await userEvent.click(
      within(form).getByRole("button", { name: "Add user" }),
    );
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
      "/users": (path, init) =>
        init?.method === "POST" ? { user: account("nora") } : listingFor(path),
    });
    renderApp("/admin/users");

    await rowFor("nora");
    const form = createForm();
    await userEvent.type(within(form).getByLabelText("Login name"), "kim");
    await userEvent.type(within(form).getByLabelText("Password"), "pw");
    await userEvent.selectOptions(within(form).getByLabelText("Role"), "none");
    expect(within(form).getByLabelText("Reflection")).toHaveValue("reader");
    await userEvent.click(
      within(form).getByRole("button", { name: "Add user" }),
    );
    await waitFor(() => {
      expect(sentBody("/users", "POST")).toEqual({
        name: "kim",
        password: "pw",
        role: "none",
        reflection: "reader",
      });
    });
  });

  it("ends a removed account's drafts only once the name is typed twice", async () => {
    const user = userEvent.setup();
    serveAs("admin", {
      "/users/ada/drafts": () => ({
        account: "ada",
        domains: [{ domain: "team", discarded: 2, left: [] }],
      }),
    });
    renderApp("/admin/users");
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
    renderApp("/admin/users");
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

  it("does not say a name holds no drafts when its kept texts could not be read", async () => {
    const user = userEvent.setup();
    serveAs("admin", {
      "/users/gone/drafts": () => ({
        account: "gone",
        domains: [],
        error: "the kept texts of domain 'team' could not be read",
      }),
    });
    renderApp("/admin/users");
    await user.type(
      await screen.findByLabelText("Removed account's name"),
      "gone",
    );
    await user.type(screen.getByLabelText("Type the name again"), "gone");
    await user.click(screen.getByRole("button", { name: "End these drafts" }));
    const said = await screen.findByText(/could not be read/);
    expect(said).toHaveAttribute("role", "alert");
    expect(screen.queryByText(/holds no drafts/)).toBeNull();
  });

  it("shows the server's refusal for a name with an account", async () => {
    const user = userEvent.setup();
    serveAs("admin", {
      "/users/eddy/drafts": () => {
        throw new ApiProblem(409, "conflict", "'eddy' is an account");
      },
    });
    renderApp("/admin/users");
    await user.type(
      await screen.findByLabelText("Removed account's name"),
      "eddy",
    );
    await user.type(screen.getByLabelText("Type the name again"), "eddy");
    await user.click(screen.getByRole("button", { name: "End these drafts" }));
    expect(await screen.findByText("'eddy' is an account")).toBeVisible();
  });
});
