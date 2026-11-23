/**
 * The people picker in the Members card, and the invite links beside it.
 *
 * Mounted through the domain settings screen, as the members card's own
 * tests are: what is under test is what the card asks the search route,
 * which accounts it lets a person pick and why not, and when it offers an
 * invite link for someone who has no account yet.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { Role } from "../api/model";
import type { Answer } from "../test/harness";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";
import { unavailableReason } from "./PeoplePicker";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

/** The members read of a private `eng` owned by `ada`, with `mem` a member. */
function membersResponse(overrides: Record<string, unknown> = {}) {
  return {
    owner: "ada",
    visibility: "private",
    governance: "crystalline",
    members: [
      {
        principal: "mem",
        level: "editor",
        reflection: "default",
        added_by: "ada",
        added_at: "2026-09-01T00:00:00Z",
      },
    ],
    overridden: [],
    can_invite: false,
    ...overrides,
  };
}

/** Three accounts: the owner, a member, and one free to add. */
const HITS = [
  { name: "ada", display: "Ada Lovelace", member: true },
  { name: "mem", display: "Mem Ber", member: true },
  { name: "ann", display: "Ann Other", member: false },
];

function serve(
  routes: Record<string, Answer> = {},
  name = "ada",
  role: Role = "editor",
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ name, role }) }),
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
      "/domains/eng/members": () => membersResponse(),
      "/accounts/search": () => ({ results: HITS }),
      ...routes,
    }),
  );
}

/** Every search path the card asked for. */
function searches(): string[] {
  return apiMock.mock.calls
    .map(([path]) => path)
    .filter((path) => path.startsWith("/accounts/search"));
}

function sentBody(path: string, method: string): unknown {
  const call = apiMock.mock.calls.find(
    ([sent, init]) => sent === path && init?.method === method,
  );
  if (!call) {
    throw new Error(`no ${method} to ${path}`);
  }
  return JSON.parse(call[1]?.body as string) as unknown;
}

async function membersCard(): Promise<HTMLElement> {
  return screen.findByRole("region", { name: "Members" });
}

/** An invite link fixture, pending, made by `ada` for `eng`. */
function inviteFixture(overrides: Record<string, unknown> = {}) {
  return {
    id: 7,
    role: "none",
    domain: "eng",
    level: "editor",
    label: "for ann",
    local_account: false,
    created_by: "ada",
    created_at: "2026-11-20T10:00:00Z",
    expires_at: "2026-11-27T10:00:00Z",
    state: "pending",
    ...overrides,
  };
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("unavailableReason", () => {
  it("words why an account cannot be picked", () => {
    const [owner, member, free] = HITS as [
      (typeof HITS)[0],
      (typeof HITS)[0],
      (typeof HITS)[0],
    ];
    expect(unavailableReason(owner, "ada", "member")).toBe("owner");
    expect(unavailableReason(member, "ada", "member")).toBe("already a member");
    expect(unavailableReason(free, "ada", "member")).toBeNull();
    expect(unavailableReason(owner, "ada", "owner")).toBe("owner");
    expect(unavailableReason(member, "ada", "owner")).toBeNull();
  });
});

describe("the people picker", () => {
  it("searches the domain from two characters and adds the account picked", async () => {
    serve({
      "/domains/eng/members/ann": (_path, init) => {
        if (init?.method === "PUT") return undefined;
        throw new ApiProblem(405, "method not allowed", "unexpected");
      },
    });
    renderApp("/d/eng/settings");
    const card = await membersCard();
    const field = within(card).getByRole("combobox", { name: "Account" });

    await userEvent.type(field, "a");
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(searches()).toEqual([]);

    await userEvent.type(field, "n");
    const list = await within(card).findByRole("listbox", {
      name: "People found",
    });
    expect(searches()).toEqual(["/accounts/search?q=an&domain=eng"]);
    expect(field).toHaveAttribute("aria-expanded", "true");

    const owner = within(list).getByRole("option", { name: /Ada Lovelace/ });
    expect(owner).toHaveTextContent("owner");
    expect(owner).toHaveAttribute("aria-disabled", "true");
    expect(
      within(list).getByRole("option", { name: /Mem Ber/ }),
    ).toHaveTextContent("already a member");

    // A row that cannot be picked leaves the field as it was.
    await userEvent.click(owner);
    expect(field).toHaveValue("an");

    await userEvent.click(
      within(list).getByRole("option", { name: /Ann Other/ }),
    );
    expect(field).toHaveValue("ann");
    expect(field).toHaveAttribute("aria-expanded", "false");

    await userEvent.click(within(card).getByRole("button", { name: "Invite" }));
    await waitFor(() => {
      expect(sentBody("/domains/eng/members/ann", "PUT")).toEqual({
        level: "viewer",
        reflection: "default",
      });
    });
  });

  it("picks with the arrow keys and Enter", async () => {
    serve();
    renderApp("/d/eng/settings");
    const card = await membersCard();
    const field = within(card).getByRole("combobox", { name: "Account" });
    await userEvent.type(field, "an");
    await within(card).findByRole("listbox", { name: "People found" });

    await userEvent.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}{Enter}");
    expect(field).toHaveValue("ann");
  });

  it("says when nobody matches, and offers no link where none can be made", async () => {
    serve({ "/accounts/search": () => ({ results: [] }) });
    renderApp("/d/eng/settings");
    const card = await membersCard();
    await userEvent.type(
      within(card).getByRole("combobox", { name: "Account" }),
      "zed",
    );
    expect(await within(card).findByText("No account matches.")).toBeVisible();
    expect(
      within(card).queryByRole("button", { name: "Invite someone new" }),
    ).toBeNull();
    expect(
      within(card).queryByRole("heading", { name: "Invite links" }),
    ).toBeNull();
    expect(apiMock.mock.calls.some(([path]) => path === "/invites")).toBe(
      false,
    );
  });

  it("transfers ownership to the account picked", async () => {
    serve({
      "/domains/eng/owner": (_path, init) => {
        if (init?.method === "PUT") return undefined;
        throw new ApiProblem(405, "method not allowed", "unexpected");
      },
    });
    renderApp("/d/eng/settings");
    const card = await membersCard();
    await userEvent.click(
      within(card).getByRole("button", { name: "Transfer ownership" }),
    );
    const field = within(card).getByRole("combobox", { name: "New owner" });
    await userEvent.type(field, "me");
    const list = await within(card).findByRole("listbox", {
      name: "People found",
    });
    // A member may become the owner; the owner is marked.
    expect(
      within(list).getByRole("option", { name: /Mem Ber/ }),
    ).not.toHaveTextContent("already a member");
    await userEvent.click(
      within(list).getByRole("option", { name: /Mem Ber/ }),
    );
    expect(field).toHaveValue("mem");

    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm transfer" }),
    );
    await waitFor(() => {
      expect(sentBody("/domains/eng/owner", "PUT")).toEqual({ owner: "mem" });
    });
  });

  it("closes only the list on Escape, keeping the transfer open", async () => {
    serve();
    renderApp("/d/eng/settings");
    const card = await membersCard();
    await userEvent.click(
      within(card).getByRole("button", { name: "Transfer ownership" }),
    );
    const field = within(card).getByRole("combobox", { name: "New owner" });
    await userEvent.type(field, "an");
    await within(card).findByRole("listbox", { name: "People found" });
    await userEvent.keyboard("{Escape}");
    expect(within(card).queryByRole("listbox")).toBeNull();
    expect(
      within(card).getByRole("button", { name: "Confirm transfer" }),
    ).toBeVisible();
  });

  it("is not drawn on a domain GitHub governs", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: null,
            governance: "github",
            members: [],
            github_people: [],
            reflection_collaborators: null,
          }),
      },
      "root",
      "admin",
    );
    renderApp("/d/eng/settings");
    const card = await membersCard();
    expect(
      within(card).queryByRole("combobox", { name: "Account" }),
    ).toBeNull();
    expect(
      within(card).queryByRole("button", { name: "Transfer ownership" }),
    ).toBeNull();
    expect(
      within(card).queryByRole("heading", { name: "Invite links" }),
    ).toBeNull();
  });
});

describe("invite links in the members card", () => {
  function serveInvites(routes: Record<string, Answer> = {}) {
    serve({
      "/domains/eng/members": () => membersResponse({ can_invite: true }),
      "/accounts/search": () => ({ results: [] }),
      "/invites": (_path, init) => {
        if (init?.method === "POST") {
          return {
            invite: inviteFixture({ id: 9, label: "zed" }),
            token: "cin_secret",
            link: "/invite#cin_secret",
            relative: true,
          };
        }
        return {
          sign_in: "local",
          invites: [
            inviteFixture(),
            inviteFixture({ id: 8, domain: "other", label: "elsewhere" }),
            inviteFixture({ id: 6, created_by: "root", label: "by root" }),
            inviteFixture({ id: 5, state: "used", label: "taken" }),
          ],
        };
      },
      ...routes,
    });
  }

  it("offers a link after a search with no hit, and shows it once", async () => {
    serveInvites();
    renderApp("/d/eng/settings");
    const card = await membersCard();
    await userEvent.type(
      within(card).getByRole("combobox", { name: "Account" }),
      "zed",
    );
    await userEvent.click(
      await within(card).findByRole("button", { name: "Invite someone new" }),
    );
    const access = within(card).getByRole("combobox", {
      name: "Access in eng",
    });
    expect(access).toHaveFocus();
    await userEvent.selectOptions(access, "editor");
    await userEvent.type(within(card).getByLabelText("Label"), "zed");
    await userEvent.click(
      within(card).getByRole("button", { name: "Make link" }),
    );

    await waitFor(() => {
      expect(sentBody("/invites", "POST")).toEqual({
        domain: "eng",
        level: "editor",
        label: "zed",
        expires_in_days: 7,
      });
    });
    const shown = await within(card).findByRole("group", {
      name: "Invite link",
    });
    expect(within(shown).getByLabelText("Invite link")).toHaveValue(
      `${window.location.origin}/invite#cin_secret`,
    );
    expect(shown).toHaveTextContent("It is not shown again.");
    await userEvent.click(within(shown).getByRole("button", { name: "Done" }));
    expect(
      within(card).queryByRole("group", { name: "Invite link" }),
    ).toBeNull();
  });

  it("lists only the caller's own pending links to this domain, with revoke", async () => {
    serveInvites({
      "/invites/7": (_path, init) => {
        if (init?.method === "DELETE") return undefined;
        throw new ApiProblem(405, "method not allowed", "unexpected");
      },
    });
    renderApp("/d/eng/settings");
    const card = await membersCard();
    const list = await within(card).findByRole("list", {
      name: "Your pending links",
    });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(1);
    expect(rows[0]).toHaveTextContent("for ann");

    await userEvent.click(
      within(list).getByRole("button", { name: "Revoke for ann (link 7)" }),
    );
    await userEvent.click(
      within(list).getByRole("button", { name: "Confirm revoke of link 7" }),
    );
    await waitFor(() => {
      expect(
        apiMock.mock.calls.some(
          ([path, init]) => path === "/invites/7" && init?.method === "DELETE",
        ),
      ).toBe(true);
    });
  });

  it("has a plain button for a link and says the server's refusal", async () => {
    serveInvites({
      "/invites": (_path, init) => {
        if (init?.method === "POST") {
          throw new ApiProblem(409, "conflict", "too many pending links");
        }
        return { sign_in: "local", invites: [] };
      },
    });
    renderApp("/d/eng/settings");
    const card = await membersCard();
    await userEvent.click(
      await within(card).findByRole("button", { name: "Make an invite link" }),
    );
    await userEvent.click(
      within(card).getByRole("button", { name: "Make link" }),
    );
    expect(await within(card).findByRole("alert")).toHaveTextContent(
      "too many pending links",
    );
    expect(
      within(card).queryByRole("group", { name: "Invite link" }),
    ).toBeNull();
  });
});
