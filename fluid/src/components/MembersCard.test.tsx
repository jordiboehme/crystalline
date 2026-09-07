/**
 * The members card: who owns a domain, who is invited into it while it is
 * private, and the one control that decides whether it is private at all.
 *
 * Mounted through the domain screen, the way `ProposalsCard` and the sync
 * card are, because what is under test is compositional: what the card draws
 * is derived from `GET /domains/{domain}/members` plus the signed-in
 * account's own name and its admin flag, never from a hand-written
 * "what may I do here" field the route does not carry.
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

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

const apiMock = vi.mocked(api);

/** One membership row, editor by default. */
function memberFixture(overrides: Record<string, unknown> = {}) {
  return {
    principal: "mem",
    level: "editor",
    added_by: "ada",
    added_at: "2026-09-01T00:00:00Z",
    ...overrides,
  };
}

/** The members read, shared by default - the state a domain nobody touched answers with. */
function membersResponse(overrides: Record<string, unknown> = {}) {
  return {
    owner: null,
    visibility: "shared",
    members: [],
    ...overrides,
  };
}

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

/** The card itself, once its own read has landed. */
async function membersCard(): Promise<HTMLElement> {
  return screen.findByRole("region", { name: "Members" });
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the members card", () => {
  it("gives the owner the visibility control, the owner row, the member table and the invite form", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [memberFixture({ principal: "mem", level: "editor" })],
          }),
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng");
    const card = await membersCard();

    expect(within(card).getByText("Private")).toBeVisible();
    expect(within(card).getByText("ada")).toBeVisible();
    expect(
      within(card).getByRole("button", { name: "Transfer ownership" }),
    ).toBeVisible();
    expect(
      within(card).getByRole("button", { name: "Share with everyone" }),
    ).toBeVisible();
    expect(within(card).getByText("mem")).toBeVisible();
    expect(
      within(card).getByRole("combobox", { name: "Level for mem" }),
    ).toHaveValue("editor");
    expect(
      within(card).getByRole("button", { name: "Remove mem" }),
    ).toBeVisible();
    expect(within(card).getByRole("button", { name: "Invite" })).toBeVisible();
  });

  it("shows no visibility control to a manager, though it may invite, relevel and remove", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [
              memberFixture({ principal: "mgr", level: "manager" }),
              memberFixture({ principal: "mem", level: "viewer" }),
            ],
          }),
      },
      "mgr",
      "editor",
    );

    renderApp("/d/eng");
    const card = await membersCard();

    // Neither direction: a manager invites and changes levels, and never
    // changes visibility.
    expect(
      within(card).queryByRole("button", { name: "Share with everyone" }),
    ).toBeNull();
    expect(
      within(card).queryByRole("button", { name: "Make private" }),
    ).toBeNull();
    // Nor may it hand the domain on.
    expect(
      within(card).queryByRole("button", { name: "Transfer ownership" }),
    ).toBeNull();

    // Its own row: relevel and leave.
    expect(
      within(card).getByRole("combobox", { name: "Level for mgr" }),
    ).toHaveValue("manager");
    expect(
      within(card).getByRole("button", { name: "Leave mgr" }),
    ).toBeVisible();
    // Somebody else's: relevel and remove, the manage right in full.
    expect(
      within(card).getByRole("combobox", { name: "Level for mem" }),
    ).toHaveValue("viewer");
    expect(
      within(card).getByRole("button", { name: "Remove mem" }),
    ).toBeVisible();
    expect(within(card).getByRole("button", { name: "Invite" })).toBeVisible();
  });

  it("gives a plain member only its own leave, no invite form and no visibility control", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [
              memberFixture({ principal: "mem", level: "editor" }),
              memberFixture({ principal: "other", level: "viewer" }),
            ],
          }),
      },
      "mem",
      "editor",
    );

    renderApp("/d/eng");
    const card = await membersCard();

    expect(within(card).queryByRole("combobox")).toBeNull();
    expect(
      within(card).getByRole("button", { name: "Leave mem" }),
    ).toBeVisible();
    expect(
      within(card).queryByRole("button", { name: /^Remove /i }),
    ).toBeNull();
    expect(within(card).queryByRole("button", { name: "Invite" })).toBeNull();
    expect(
      within(card).queryByRole("button", { name: "Share with everyone" }),
    ).toBeNull();
    // The other row's level, read rather than changed.
    expect(within(card).getByText("viewer")).toBeVisible();
  });

  it("renders an ownerless domain honestly, and offers an admin an assign-manager form", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({ owner: null, visibility: "private", members: [] }),
      },
      "boss",
      "admin",
    );

    renderApp("/d/eng");
    const card = await membersCard();

    expect(
      within(card).getByText(/No owner - removed from the account roster/),
    ).toBeVisible();
    expect(
      within(card).queryByRole("button", { name: "Transfer ownership" }),
    ).toBeNull();
    expect(within(card).getByText(/This domain has no owner/)).toBeVisible();
    expect(
      within(card).getByRole("button", { name: "Assign manager" }),
    ).toBeVisible();
    expect(within(card).getByRole("combobox", { name: "Level" })).toHaveValue(
      "manager",
    );
    // The admin may still open it back up: `own` from the admin flag alone.
    expect(
      within(card).getByRole("button", { name: "Share with everyone" }),
    ).toBeVisible();
  });

  it("offers an admin the way to close a shared domain, with the disk-truth caption", async () => {
    serve(
      {
        "/domains/eng/visibility": (_path, init) => {
          if (init?.method === "PUT") {
            return undefined;
          }
          throw new ApiProblem(405, "method not allowed", "unexpected method");
        },
      },
      "boss",
      "admin",
    );

    renderApp("/d/eng");
    const card = await membersCard();

    expect(within(card).getByText("Shared with everyone")).toBeVisible();
    expect(
      within(card).getByText(
        "Private domains protect from other users of this instance, not from whoever operates the machine.",
      ),
    ).toBeVisible();
    const trigger = within(card).getByRole("button", { name: "Make private" });
    await userEvent.click(trigger);
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm make private" }),
    );

    await waitFor(() => {
      expect(sentBody("/domains/eng/visibility", "PUT")).toEqual({
        private: true,
      });
    });
    expect(
      await within(card).findByText("This domain is private now."),
    ).toBeVisible();
  });

  it("offers a non-admin nothing but the plain shared state", async () => {
    serve({}, "mem", "editor");

    renderApp("/d/eng");
    const card = await membersCard();

    expect(
      within(card).getByText(
        "Every account on this instance can read this domain.",
      ),
    ).toBeVisible();
    expect(
      within(card).queryByRole("button", { name: "Make private" }),
    ).toBeNull();
  });

  it("renders the server's own refusal rather than pretending the row changed", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [memberFixture({ principal: "mem", level: "editor" })],
          }),
        "/domains/eng/members/mem": () => {
          throw new ApiProblem(
            403,
            "forbidden",
            "your membership on this domain is editor, and manager access is required",
          );
        },
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng");
    const card = await membersCard();

    await userEvent.click(
      within(card).getByRole("button", { name: "Remove mem" }),
    );
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm remove mem" }),
    );

    expect(
      await within(card).findByText(/your membership on this domain is editor/),
    ).toBeVisible();
    // Refused, so the row nothing happened to is still there.
    expect(within(card).getByText("mem")).toBeVisible();
  });

  it("returns focus to the row's own Remove button after Escape and after Keep", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [memberFixture({ principal: "mem", level: "editor" })],
          }),
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng");
    const card = await membersCard();

    const trigger = within(card).getByRole("button", { name: "Remove mem" });
    await userEvent.click(trigger);
    expect(
      within(card).getByRole("button", { name: "Confirm remove mem" }),
    ).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(
      within(card).queryByRole("button", { name: "Confirm remove mem" }),
    ).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();

    await userEvent.click(trigger);
    await userEvent.click(within(card).getByRole("button", { name: "Keep" }));
    expect(trigger).toHaveFocus();
  });

  it("leaves a member's own reach into the domain behind after it leaves", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [memberFixture({ principal: "mem", level: "editor" })],
          }),
        "/domains/eng/members/mem": (_path, init) => {
          if (init?.method === "DELETE") {
            return undefined;
          }
          throw new ApiProblem(405, "method not allowed", "unexpected method");
        },
      },
      "mem",
      "editor",
    );

    renderApp("/d/eng");
    const card = await membersCard();

    await userEvent.click(
      within(card).getByRole("button", { name: "Leave mem" }),
    );
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm leave mem" }),
    );

    // Nowhere to stay: this account no longer sees a private domain it just
    // left, the same way `UnregisterDomain`'s own domain becomes a wrong
    // address for a caller who just unregistered it.
    expect(await screen.findByRole("heading", { name: "Home" })).toBeVisible();
  });

  it("hands a domain away by typing the new owner's name and confirming, and leaves behind it when the caller was the owner", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [],
          }),
        "/domains/eng/owner": (_path, init) => {
          if (init?.method === "PUT") {
            return undefined;
          }
          throw new ApiProblem(405, "method not allowed", "unexpected method");
        },
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng");
    const card = await membersCard();

    await userEvent.click(
      within(card).getByRole("button", { name: "Transfer ownership" }),
    );
    await userEvent.type(within(card).getByLabelText("New owner"), "newowner");
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm transfer" }),
    );

    await waitFor(() => {
      expect(sentBody("/domains/eng/owner", "PUT")).toEqual({
        owner: "newowner",
      });
    });
    expect(await screen.findByRole("heading", { name: "Home" })).toBeVisible();
  });
});
