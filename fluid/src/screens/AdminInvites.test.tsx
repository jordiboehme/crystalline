/**
 * Administration > Invites.
 *
 * Pinned: a link is made with the role, the private domain and its level,
 * the label and the life asked for, and is shown once with a copy button; a
 * relative link is completed with this page's origin and keeps the base
 * path the server put in it; the local-account choice is offered only when
 * the instance signs people in through an identity provider; the list shows
 * pending and used links, and revoked and expired ones under the filter; a
 * pending link is revoked only after the second press; the server's
 * sentence for a refused link is shown.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { Invite, InviteSignIn } from "../api/model";
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

function invite(overrides: Partial<Invite>): Invite {
  return {
    id: 1,
    role: "viewer",
    domain: null,
    level: null,
    label: null,
    local_account: false,
    created_by: "root",
    created_at: "2026-11-20T10:00:00Z",
    expires_at: "2026-11-27T10:00:00Z",
    used_at: null,
    used_by: null,
    revoked_at: null,
    state: "pending",
    ...overrides,
  };
}

function invites(): Invite[] {
  return [
    invite({ id: 4, label: "for Pia", domain: "team", level: "editor" }),
    invite({
      id: 3,
      label: "for Nina",
      state: "used",
      used_at: "2026-11-21T09:00:00Z",
      used_by: "nina",
    }),
    invite({ id: 2, state: "revoked", revoked_at: "2026-11-21T09:00:00Z" }),
    invite({ id: 1, label: "old", state: "expired" }),
  ];
}

/** The listing with one shared and one private domain. */
function domains() {
  const listing = domainsResponse();
  return {
    ...listing,
    domains: [
      ...listing.domains,
      { ...listing.domains[0], name: "team", private: true },
    ],
  };
}

/** The app signed in as an admin, with the invites and `routes` served. */
function serve(signIn: InviteSignIn = "local", post?: Answer) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({ user: userFixture({ name: "root", role: "admin" }) }),
      "/domains": domains,
      "/invites": (path, init) => {
        if (init?.method === "POST") {
          if (!post) throw new ApiProblem(404, "not found", "no stub");
          return post(path, init);
        }
        return { invites: invites(), sign_in: signIn };
      },
      "/invites/4": () => undefined,
    }),
  );
}

function sentBody(path: string, method: string): unknown {
  const call = apiMock.mock.calls.find(
    ([sent, init]) => sent === path && init?.method === method,
  );
  if (!call) throw new Error(`no ${method} to ${path}`);
  const body = call[1]?.body;
  if (typeof body !== "string") {
    throw new Error(`the ${method} to ${path} carried no JSON body`);
  }
  return JSON.parse(body) as unknown;
}

function sent(path: string, method: string): boolean {
  return apiMock.mock.calls.some(
    ([to, init]) => to === path && init?.method === method,
  );
}

function made(link: string, relative: boolean): Answer {
  return () => ({
    invite: invite({ id: 9 }),
    token: "cin_secret",
    link,
    relative,
  });
}

/** One of the lists, by its heading. */
async function listFor(title: string): Promise<HTMLElement> {
  const heading = await screen.findByRole("heading", { level: 3, name: title });
  const section = heading.closest("section");
  if (!section) throw new Error(`no list ${title}`);
  return section;
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the invites page", () => {
  it("lists pending and used links, and the others under the filter", async () => {
    serve();
    renderApp("/admin/invites");

    const pending = await listFor("Pending");
    expect(pending).toHaveTextContent("for Pia");
    expect(pending).toHaveTextContent("editor in team");
    const used = await listFor("Used");
    expect(used).toHaveTextContent("for Nina");
    expect(used).toHaveTextContent("by nina");
    expect(screen.queryByText("link 2")).toBeNull();

    await userEvent.selectOptions(screen.getByLabelText("Show"), "revoked");
    expect(await listFor("Revoked")).toHaveTextContent("link 2");
    expect(screen.queryByRole("heading", { name: "Pending" })).toBeNull();
    await userEvent.selectOptions(screen.getByLabelText("Show"), "expired");
    expect(await listFor("Expired")).toHaveTextContent("old");
  });

  it("makes a link for a private domain and shows it once", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>(() =>
      Promise.resolve(),
    );
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    serve(
      "local",
      made("https://kb.example.com/crystalline/invite#cin_secret", false),
    );
    renderApp("/admin/invites");

    await listFor("Pending");
    const domain = screen.getByLabelText("Domain");
    // Only the private domain can be invited into.
    expect(
      within(domain)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual(["No domain", "team"]);
    await userEvent.selectOptions(screen.getByLabelText("Role"), "none");
    await userEvent.selectOptions(domain, "team");
    await userEvent.selectOptions(
      screen.getByLabelText("Level in team"),
      "manager",
    );
    await userEvent.type(screen.getByLabelText("Label"), "for Ida");
    const days = screen.getByLabelText("Expires in days");
    await userEvent.clear(days);
    await userEvent.type(days, "3");
    await userEvent.click(screen.getByRole("button", { name: "Make link" }));

    const shown = await screen.findByRole("group", { name: "Invite link" });
    expect(sentBody("/invites", "POST")).toEqual({
      role: "none",
      domain: "team",
      level: "manager",
      label: "for Ida",
      expires_in_days: 3,
      local_account: false,
    });
    expect(within(shown).getByLabelText("Invite link")).toHaveValue(
      "https://kb.example.com/crystalline/invite#cin_secret",
    );
    expect(shown).toHaveTextContent("It is not shown again.");
    await userEvent.click(
      within(shown).getByRole("button", { name: "Copy the invite link" }),
    );
    expect(writeText).toHaveBeenCalledWith(
      "https://kb.example.com/crystalline/invite#cin_secret",
    );
    await userEvent.click(within(shown).getByRole("button", { name: "Done" }));
    expect(screen.queryByDisplayValue(/cin_secret/)).toBeNull();
  });

  it("completes a relative link with this page's origin", async () => {
    serve("local", made("/crystalline/invite#cin_secret", true));
    renderApp("/admin/invites");

    await listFor("Pending");
    await userEvent.click(screen.getByRole("button", { name: "Make link" }));
    expect(
      await screen.findByDisplayValue(
        `${window.location.origin}/crystalline/invite#cin_secret`,
      ),
    ).toBeInTheDocument();
    expect(sentBody("/invites", "POST")).toEqual({
      role: "viewer",
      domain: null,
      level: null,
      label: null,
      expires_in_days: 7,
      local_account: false,
    });
  });

  it("offers a local account only where an identity provider signs people in", async () => {
    serve("local");
    renderApp("/admin/invites");
    await listFor("Pending");
    expect(
      screen.queryByLabelText(/Allow a local account/),
    ).not.toBeInTheDocument();
  });

  it("sends the local-account choice behind a proxy", async () => {
    serve("proxy", made("/invite#cin_secret", true));
    renderApp("/admin/invites");
    await listFor("Pending");
    await userEvent.click(
      await screen.findByLabelText(/Allow a local account/),
    );
    await userEvent.click(screen.getByRole("button", { name: "Make link" }));
    await screen.findByRole("group", { name: "Invite link" });
    expect(sentBody("/invites", "POST")).toMatchObject({
      local_account: true,
    });
  });

  it("shows the server's sentence when a link is refused", async () => {
    const sentence = "pending invite links reached the account limit";
    serve("local", () => {
      throw new ApiProblem(409, "Conflict", sentence);
    });
    renderApp("/admin/invites");
    await listFor("Pending");
    await userEvent.click(screen.getByRole("button", { name: "Make link" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(sentence);
    expect(screen.queryByRole("group", { name: "Invite link" })).toBeNull();
  });

  it("revokes a pending link only after the second press", async () => {
    serve();
    renderApp("/admin/invites");

    const pending = await listFor("Pending");
    await userEvent.click(
      within(pending).getByRole("button", { name: "Revoke for Pia (link 4)" }),
    );
    expect(sent("/invites/4", "DELETE")).toBe(false);
    // A used link offers no revoke.
    expect(
      within(await listFor("Used")).queryByRole("button", { name: /Revoke/ }),
    ).toBeNull();
    await userEvent.click(
      within(pending).getByRole("button", { name: "Confirm revoke of link 4" }),
    );
    await waitFor(() => {
      expect(sent("/invites/4", "DELETE")).toBe(true);
    });
    expect(await screen.findByRole("status")).toHaveTextContent(
      "Revoked for Pia.",
    );
  });
  it("shows only the link being revoked as pending", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/auth/me": () =>
          meResponse({ user: userFixture({ name: "root", role: "admin" }) }),
        "/domains": domains,
        "/invites": () => ({
          invites: [
            invite({ id: 4, label: "a" }),
            invite({ id: 5, label: "b" }),
          ],
          sign_in: "local",
        }),
        "/invites/4": () => new Promise(() => undefined),
      }),
    );
    renderApp("/admin/invites");

    const pending = await listFor("Pending");
    await userEvent.click(
      within(pending).getByRole("button", { name: "Revoke a (link 4)" }),
    );
    await userEvent.click(
      within(pending).getByRole("button", { name: "Confirm revoke of link 4" }),
    );
    await waitFor(() => {
      expect(
        within(pending).getByRole("button", { name: "Revoke a (link 4)" }),
      ).toHaveAttribute("aria-disabled", "true");
    });
    expect(
      within(pending).getByRole("button", { name: "Revoke b (link 5)" }),
    ).toHaveAttribute("aria-disabled", "false");
  });
});
