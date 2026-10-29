/**
 * The members card: who owns a domain and who is invited into it while it is
 * private. Whether it is private at all is the danger zone's question, one
 * card further down the page, and the tests for it live beside that card.
 *
 * Mounted through the domain screen, the way `ProposalsCard` and the sync
 * card are, because what is under test is compositional: what the card draws
 * is derived from `GET /domains/{domain}/members` plus the signed-in
 * account's own name and its admin flag, never from a hand-written
 * "what may I do here" field the route does not carry.
 */

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
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
    reflection: "default",
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
  it("sets both levels side by side, with curator implied for editors", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [
              memberFixture({
                principal: "mem",
                level: "none",
                reflection: "contributor",
              }),
              memberFixture({ principal: "ed", level: "editor" }),
            ],
          }),
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();

    const options = (name: string) =>
      within(within(card).getByRole("combobox", { name }))
        .getAllByRole("option")
        .map((option) => option.textContent);
    expect(options("Level")).toEqual(["none", "viewer", "editor", "manager"]);
    expect(
      within(card).getByRole("combobox", { name: "Level for mem" }),
    ).toHaveValue("none");
    expect(
      within(card).getByRole("combobox", { name: "Reflection for mem" }),
    ).toHaveValue("contributor");
    expect(options("Reflection for mem")).toEqual(["reader", "contributor"]);
    const edRow = within(card)
      .getByRole("rowheader", { name: "ed" })
      .closest("tr") as HTMLElement;
    expect(within(edRow).getByText("curator")).toBeVisible();
    expect(
      within(card).queryByRole("combobox", { name: "Reflection for ed" }),
    ).toBeNull();
  });

  it("sends a row's Reflection change with its knowledge level", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [memberFixture({ principal: "mem", level: "viewer" })],
          }),
        "/domains/eng/members/mem": () => undefined,
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();
    await userEvent.selectOptions(
      within(card).getByRole("combobox", { name: "Reflection for mem" }),
      "contributor",
    );
    await waitFor(() => {
      expect(sentBody("/domains/eng/members/mem", "PUT")).toEqual({
        level: "viewer",
        reflection: "contributor",
      });
    });
  });

  it("moves a row to none with a Reflection level, never with nothing", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [memberFixture({ principal: "mem", level: "viewer" })],
          }),
        "/domains/eng/members/mem": () => undefined,
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();
    await userEvent.selectOptions(
      within(card).getByRole("combobox", { name: "Level for mem" }),
      "none",
    );
    await waitFor(() => {
      expect(sentBody("/domains/eng/members/mem", "PUT")).toEqual({
        level: "none",
        reflection: "reader",
      });
    });
  });

  it("resets a chosen Reflection level when the invite level curates", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({ owner: "ada", visibility: "private", members: [] }),
        "/domains/eng/members/ann": () => undefined,
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();
    await userEvent.type(
      within(card).getByRole("textbox", { name: "Account" }),
      "ann",
    );
    await userEvent.selectOptions(
      within(card).getByRole("combobox", { name: "Reflection" }),
      "contributor",
    );
    await userEvent.selectOptions(
      within(card).getByRole("combobox", { name: "Level" }),
      "editor",
    );
    expect(within(card).getByLabelText("Reflection")).toHaveTextContent(
      "curator",
    );
    await userEvent.click(within(card).getByRole("button", { name: "Invite" }));
    await waitFor(() => {
      expect(sentBody("/domains/eng/members/ann", "PUT")).toEqual({
        level: "editor",
        reflection: "default",
      });
    });
  });

  it("invites at knowledge none with a Reflection level, never with nothing", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({ owner: "ada", visibility: "private", members: [] }),
        "/domains/eng/members/ann": () => undefined,
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();
    await userEvent.type(
      within(card).getByRole("textbox", { name: "Account" }),
      "ann",
    );
    await userEvent.selectOptions(
      within(card).getByRole("combobox", { name: "Level" }),
      "none",
    );
    expect(
      within(card).getByRole("combobox", { name: "Reflection" }),
    ).toHaveValue("reader");
    await userEvent.click(within(card).getByRole("button", { name: "Invite" }));
    await waitFor(() => {
      expect(sentBody("/domains/eng/members/ann", "PUT")).toEqual({
        level: "none",
        reflection: "reader",
      });
    });
  });

  it("gives the owner the owner row, the member table and the invite form", async () => {
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

    renderApp("/d/eng/settings");
    const card = await membersCard();

    expect(within(card).getByText("Private")).toBeVisible();
    expect(within(card).getByText("ada")).toBeVisible();
    expect(
      within(card).getByRole("button", { name: "Transfer ownership" }),
    ).toBeVisible();
    // Whether the domain is private at all is decided in the danger zone at
    // the foot of the page, not here: this card administers a team.
    expect(
      within(card).queryByRole("button", { name: "Share with everyone" }),
    ).toBeNull();
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

    renderApp("/d/eng/settings");
    const card = await membersCard();

    // Neither direction, here or anywhere else on the page: a manager invites
    // and changes levels, and never changes visibility.
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

    renderApp("/d/eng/settings");
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

  it("renders an ownerless domain honestly, and offers an admin both an assign-manager form and a way to give it an owner", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({ owner: null, visibility: "private", members: [] }),
      },
      "boss",
      "admin",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();

    expect(
      within(card).getByText(/No owner - removed from the account roster/),
    ).toBeVisible();
    // `set_owner` needs only `DomainRight::Own` and a private domain - it
    // never inspects the current owner - so an admin has a path back to a
    // domain having one at all, not only the assign-manager route.
    expect(
      within(card).getByRole("button", { name: "Transfer ownership" }),
    ).toBeVisible();
    expect(within(card).getByText(/This domain has no owner/)).toBeVisible();
    expect(
      within(card).getByRole("button", { name: "Assign manager" }),
    ).toBeVisible();
    expect(within(card).getByRole("combobox", { name: "Level" })).toHaveValue(
      "manager",
    );
  });

  it("gives a manager on an ownerless domain neither owner control", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: null,
            visibility: "private",
            members: [memberFixture({ principal: "mgr", level: "manager" })],
          }),
      },
      "mgr",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();

    expect(
      within(card).queryByRole("button", { name: "Transfer ownership" }),
    ).toBeNull();
    expect(
      within(card).queryByRole("button", { name: "Assign manager" }),
    ).toBeNull();
    // It still invites at an ordinary level, the manage right in full.
    expect(within(card).getByRole("button", { name: "Invite" })).toBeVisible();
  });

  it("offers a non-admin nothing but the plain shared state", async () => {
    serve({}, "mem", "editor");

    renderApp("/d/eng/settings");
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

  it("invites an account at the level chosen in the form, and clears the field once it lands", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({ owner: "ada", visibility: "private", members: [] }),
        "/domains/eng/members/newmem": (_path, init) => {
          if (init?.method === "PUT") {
            return undefined;
          }
          throw new ApiProblem(405, "method not allowed", "unexpected method");
        },
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();

    await userEvent.type(within(card).getByLabelText("Account"), "newmem");
    await userEvent.selectOptions(
      within(card).getByRole("combobox", { name: "Level" }),
      "editor",
    );
    await userEvent.click(within(card).getByRole("button", { name: "Invite" }));

    await waitFor(() => {
      expect(sentBody("/domains/eng/members/newmem", "PUT")).toEqual({
        level: "editor",
        reflection: "default",
      });
    });
    // The form clears on the invite's own success, not on submit - so this
    // is the request having landed, not merely having been sent.
    await waitFor(() => {
      expect(within(card).getByLabelText("Account")).toHaveValue("");
    });
  });

  it("keeps a refused invite's typed name on screen instead of discarding it", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({ owner: "ada", visibility: "private", members: [] }),
        "/domains/eng/members/nobody": () => {
          throw new ApiProblem(422, "unprocessable", "no such account");
        },
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();

    await userEvent.type(within(card).getByLabelText("Account"), "nobody");
    await userEvent.click(within(card).getByRole("button", { name: "Invite" }));

    // The refusal renders, and the typed name stays so it can be fixed rather
    // than retyped.
    expect(await within(card).findByText(/no such account/)).toBeVisible();
    expect(within(card).getByLabelText("Account")).toHaveValue("nobody");
  });

  it("changes a member's level from its row", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [memberFixture({ principal: "mem", level: "editor" })],
          }),
        "/domains/eng/members/mem": (_path, init) => {
          if (init?.method === "PUT") {
            return undefined;
          }
          throw new ApiProblem(405, "method not allowed", "unexpected method");
        },
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();

    await userEvent.selectOptions(
      within(card).getByRole("combobox", { name: "Level for mem" }),
      "manager",
    );

    await waitFor(() => {
      expect(sentBody("/domains/eng/members/mem", "PUT")).toEqual({
        level: "manager",
        reflection: "default",
      });
    });
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

    renderApp("/d/eng/settings");
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

    renderApp("/d/eng/settings");
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

    renderApp("/d/eng/settings");
    const card = await membersCard();

    await userEvent.click(
      within(card).getByRole("button", { name: "Leave mem" }),
    );
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm leave mem" }),
    );

    // Nowhere to stay: this account no longer sees a private domain it just
    // left, the same way an unregistered domain's own page becomes a wrong
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

    renderApp("/d/eng/settings");
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

  it("stays put when an owner transfers a domain to itself, since nothing changed", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({ owner: "ada", visibility: "private", members: [] }),
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

    renderApp("/d/eng/settings");
    const card = await membersCard();

    await userEvent.click(
      within(card).getByRole("button", { name: "Transfer ownership" }),
    );
    await userEvent.type(within(card).getByLabelText("New owner"), "ada");
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm transfer" }),
    );

    await waitFor(() => {
      expect(sentBody("/domains/eng/owner", "PUT")).toEqual({ owner: "ada" });
    });
    // A transfer to the same account the caller already is loses it nothing,
    // unlike handing the domain to somebody else - so it is not bounced away.
    expect(await within(card).findByText("ada")).toBeVisible();
    expect(screen.queryByRole("heading", { name: "Home" })).toBeNull();
  });

  it("keeps an admin who is also a member on the page after it leaves, since an admin never lost the domain", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [memberFixture({ principal: "boss", level: "viewer" })],
          }),
        "/domains/eng/members/boss": (_path, init) => {
          if (init?.method === "DELETE") {
            return undefined;
          }
          throw new ApiProblem(405, "method not allowed", "unexpected method");
        },
      },
      "boss",
      "admin",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();

    await userEvent.click(
      within(card).getByRole("button", { name: "Leave boss" }),
    );
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm leave boss" }),
    );

    await waitFor(() => {
      expect(
        apiMock.mock.calls.some(
          ([sent, init]) =>
            sent === "/domains/eng/members/boss" && init?.method === "DELETE",
        ),
      ).toBe(true);
    });
    // `decide` answers `Own` for any admin before it ever looks at the acl -
    // an admin keeps the domain regardless of its own row, unlike a plain
    // member, so it is not bounced to `/`.
    expect(await screen.findByText("boss")).toBeVisible();
    expect(screen.queryByRole("heading", { name: "Home" })).toBeNull();
  });

  it("draws only the plain shared state for an anonymous viewer of a shared domain", async () => {
    serve(
      {
        "/auth/me": () => meResponse({ anonymous: true }),
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng/settings");
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

  it("keeps its four disabled controls in the tab order and inert, rather than removed from it, on a read-only instance", async () => {
    // `aria-disabled`, not the native `disabled` attribute: a control taken
    // fully out of the tab order can never be landed on by a keyboard user,
    // so the reason `aria-describedby` attaches to it could never be heard.
    // This repo already ruled on exactly this trade in `Layout.tsx`'s own
    // `ShareChanges`.
    serve(
      {
        "/auth/me": () =>
          meResponse({
            user: userFixture({ name: "boss", role: "admin" }),
            read_only: true,
          }),
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [memberFixture({ principal: "mem", level: "editor" })],
          }),
      },
      "boss",
      "admin",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();

    const reason =
      "This instance is read only, so nothing here can be changed.";
    const transfer = within(card).getByRole("button", {
      name: "Transfer ownership",
    });
    const relevel = within(card).getByRole("combobox", {
      name: "Level for mem",
    });
    const remove = within(card).getByRole("button", { name: "Remove mem" });
    const invite = within(card).getByRole("button", { name: "Invite" });
    for (const control of [transfer, relevel, remove, invite]) {
      expect(control).not.toBeDisabled();
      expect(control).toHaveAttribute("aria-disabled", "true");
      expect(control).toHaveAccessibleDescription(reason);
      // Still IN the tab order, which is the whole point of `aria-disabled`
      // over `disabled`: focusing a node by script proves only that it can
      // hold focus, so what is asserted is that nothing took it out of the
      // sequential order - a native button or select without `tabindex="-1"`
      // is in it by definition.
      expect(control).not.toHaveAttribute("tabindex", "-1");
      control.focus();
      expect(control).toHaveFocus();
    }
    // And reached by the keyboard for real, on the one that ends the form: a
    // Tab from the Reflection picker (the level picker's neighbour) lands on
    // Invite rather than skipping past it.
    within(card).getByRole("combobox", { name: "Reflection" }).focus();
    await userEvent.tab();
    expect(invite).toHaveFocus();

    // The submit's guard needs something to submit, or a refusal to call the
    // API says only that the form was empty. `readonly` refuses a person's
    // typing and not a programmatic change, which is exactly what is wanted
    // here: a filled field on a read-only instance. The same field and the same
    // press DO reach the API on a writable one - "invites an account at the
    // level chosen in the form" above - so what is asserted below is the guard
    // and not an empty form.
    const account = within(card).getByLabelText("Account");
    fireEvent.change(account, { target: { value: "newbie" } });
    expect(account).toHaveValue("newbie");

    const before = apiMock.mock.calls.length;
    await userEvent.click(transfer);
    await userEvent.selectOptions(relevel, "manager");
    await userEvent.click(remove);
    await userEvent.click(invite);
    // A guarded press is a press that does nothing: none of the four reached
    // the network, and the two confirm-pattern controls never even opened
    // their second step.
    expect(apiMock.mock.calls.length).toBe(before);
    expect(
      within(card).queryByRole("button", { name: "Confirm transfer" }),
    ).toBeNull();
    expect(
      within(card).queryByRole("button", { name: "Confirm remove mem" }),
    ).toBeNull();
    expect(relevel).toHaveValue("editor");
  });

  it("marks the invite form's account field read-only and its level picker inert too, not just the submit button", async () => {
    serve(
      {
        "/auth/me": () =>
          meResponse({
            user: userFixture({ name: "boss", role: "admin" }),
            read_only: true,
          }),
        "/domains/eng/members": () =>
          membersResponse({ owner: "ada", visibility: "private", members: [] }),
      },
      "boss",
      "admin",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();

    const reason =
      "This instance is read only, so nothing here can be changed.";
    const account = within(card).getByLabelText("Account");
    const level = within(card).getByRole("combobox", { name: "Level" });
    const reflection = within(card).getByRole("combobox", {
      name: "Reflection",
    });

    // Read-only, not disabled: a plain text field needs no guarded press, so
    // the native `readonly` attribute already refuses edits on its own while
    // keeping the field focusable and its reason announced.
    expect(account).toHaveAttribute("readonly");
    expect(account).toHaveAccessibleDescription(reason);
    expect(level).toHaveAttribute("aria-disabled", "true");
    expect(level).toHaveAccessibleDescription(reason);
    expect(reflection).toHaveAttribute("aria-disabled", "true");
    expect(reflection).toHaveAccessibleDescription(reason);

    // In the tab order rather than merely focusable, for the reason the
    // five-control test above spells out.
    for (const control of [account, level, reflection]) {
      expect(control).not.toHaveAttribute("tabindex", "-1");
      control.focus();
      expect(control).toHaveFocus();
    }
    expect(reflection).toHaveFocus();
    await userEvent.tab();
    expect(within(card).getByRole("button", { name: "Invite" })).toHaveFocus();

    await userEvent.type(account, "x");
    expect(account).toHaveValue("");
    await userEvent.selectOptions(level, "manager");
    expect(level).toHaveValue("viewer");
    await userEvent.selectOptions(reflection, "contributor");
    expect(reflection).toHaveValue("default");
  });

  it("clears the typed owner and returns focus to Transfer ownership after Escape", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({ owner: "ada", visibility: "private", members: [] }),
      },
      "ada",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();

    const trigger = within(card).getByRole("button", {
      name: "Transfer ownership",
    });
    await userEvent.click(trigger);
    const field = within(card).getByLabelText("New owner");
    await userEvent.type(field, "newowner");
    expect(field).toHaveValue("newowner");

    await userEvent.keyboard("{Escape}");
    expect(
      within(card).queryByRole("button", { name: "Confirm transfer" }),
    ).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();

    // Reopening finds the field empty: `abandon()` cleared the typed value,
    // not only the confirm step, on this - the `children` - path of the
    // shared confirm, same as it always has on the childless one.
    await userEvent.click(trigger);
    expect(within(card).getByLabelText("New owner")).toHaveValue("");
  });
});

/** GitHub's people on a governed `eng`: maint manages, wri writes, one unlinked reader. */
function githubPeople() {
  return [
    {
      login: "maint-gh",
      github_role: "maintain",
      level: "manager",
      account: "maint",
    },
    {
      login: "nobody-gh",
      github_role: "read",
      level: "viewer",
      account: null,
    },
    { login: "wri-gh", github_role: "write", level: "editor", account: "wri" },
  ];
}

/** The members read of a governed `eng`, shared by default. */
function governedResponse(overrides: Record<string, unknown> = {}) {
  return membersResponse({
    governance: "github",
    github_people: githubPeople(),
    overridden: [],
    reflection_collaborators: null,
    ...overrides,
  });
}

describe("the members card on a domain GitHub governs", () => {
  it("renders People on GitHub read-only", async () => {
    serve(
      { "/domains/eng/members": () => governedResponse() },
      "wri",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();
    const people = within(card).getByRole("table", {
      name: "People on GitHub",
    });
    expect(
      within(people).getByRole("rowheader", { name: "maint-gh" }),
    ).toBeVisible();
    const nobody = within(people)
      .getByRole("rowheader", { name: "nobody-gh" })
      .closest("tr") as HTMLElement;
    expect(within(nobody).getByText("read")).toBeVisible();
    expect(within(nobody).getByText("viewer")).toBeVisible();
    expect(within(nobody).getByText("Not linked")).toBeVisible();
    // Nothing here changes what GitHub says, and nothing hands the domain on.
    expect(within(card).queryAllByRole("combobox")).toEqual([]);
    expect(within(card).queryByRole("button", { name: "Invite" })).toBeNull();
    expect(
      within(card).queryByRole("button", { name: "Transfer ownership" }),
    ).toBeNull();
    // A writer sees who may read, and cannot change it.
    const everyone = within(card).getByRole("radio", {
      name: "Everyone signed in",
    });
    expect(everyone).toBeChecked();
    expect(everyone).toBeDisabled();
  });

  it("renders overridden grants under their heading", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          governedResponse({
            visibility: "private",
            // The old acl owner is never sent: GitHub decides.
            owner: null,
            overridden: [memberFixture({ principal: "wri", level: "viewer" })],
          }),
      },
      "maint",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();
    const overridden = within(card).getByRole("table", {
      name: "Set in Crystalline, overridden by GitHub",
    });
    expect(
      within(overridden).getByRole("rowheader", { name: "wri" }),
    ).toBeVisible();
    // Read-only: GitHub decides, so no row offers a change or a removal.
    expect(within(overridden).queryAllByRole("combobox")).toEqual([]);
    expect(within(overridden).queryAllByRole("button")).toEqual([]);
  });

  it("adds and removes a Reflection collaborator", async () => {
    let grants = [
      {
        account: "ann",
        level: "reader",
        added_by: "maint",
        added_at: "2026-10-01T00:00:00Z",
      },
    ];
    serve(
      {
        "/domains/eng/members": () =>
          governedResponse({ reflection_collaborators: grants }),
        "/domains/eng/reflection-collaborators/bob": (_path, init) => {
          if (init?.method === "PUT") {
            grants = [
              ...grants,
              {
                account: "bob",
                level: "contributor",
                added_by: "maint",
                added_at: "2026-10-02T00:00:00Z",
              },
            ];
          }
          return undefined;
        },
        "/domains/eng/reflection-collaborators/ann": (_path, init) => {
          if (init?.method === "DELETE") {
            grants = grants.filter((grant) => grant.account !== "ann");
          }
          return undefined;
        },
      },
      "maint",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();
    const section = within(card).getByRole("region", {
      name: "Reflection collaborators",
    });
    expect(
      within(section).getByRole("rowheader", { name: "ann" }),
    ).toBeVisible();

    await userEvent.type(
      within(section).getByRole("textbox", { name: "Collaborator" }),
      "bob",
    );
    await userEvent.selectOptions(
      within(section).getByRole("combobox", { name: "Reflection access" }),
      "contributor",
    );
    await userEvent.click(
      within(section).getByRole("button", { name: "Add collaborator" }),
    );
    await waitFor(() => {
      expect(
        sentBody("/domains/eng/reflection-collaborators/bob", "PUT"),
      ).toEqual({ level: "contributor" });
    });
    expect(
      await within(section).findByRole("rowheader", { name: "bob" }),
    ).toBeVisible();

    await userEvent.click(
      within(section).getByRole("button", { name: "Remove ann" }),
    );
    await userEvent.click(
      within(section).getByRole("button", { name: "Confirm remove ann" }),
    );
    await waitFor(() => {
      expect(
        within(section).queryByRole("rowheader", { name: "ann" }),
      ).toBeNull();
    });
    expect(
      apiMock.mock.calls.some(
        ([path, init]) =>
          path === "/domains/eng/reflection-collaborators/ann" &&
          init?.method === "DELETE",
      ),
    ).toBe(true);
  });

  it("lets a GitHub maintainer switch who may read", async () => {
    serve(
      {
        "/domains/eng/members": () => governedResponse(),
        "/domains/eng/visibility": () => undefined,
      },
      "maint",
      "editor",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();
    const github = within(card).getByRole("radio", {
      name: "Only people with access on GitHub",
    });
    expect(github).toBeEnabled();
    await userEvent.click(github);
    await waitFor(() => {
      expect(sentBody("/domains/eng/visibility", "PUT")).toEqual({
        private: true,
      });
    });
  });

  it("keeps the switch read-only where the environment sets who may read", async () => {
    const sentence = "Set by CRYSTALLINE_DOMAIN_ENG_READERS; change it there.";
    serve(
      {
        "/domains/eng/members": () =>
          governedResponse({ visibility_fixed_by: sentence }),
      },
      "root",
      "admin",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();
    expect(
      within(card).getByRole("radio", { name: "Everyone signed in" }),
    ).toBeDisabled();
    expect(within(card).getByText(sentence)).toBeVisible();
  });

  it("tells an admin why a closed domain is closed", async () => {
    const reason =
      "the GitHub App is not installed on acme/eng: install it there or disconnect the domain";
    serve(
      {
        "/domains/eng/members": () =>
          governedResponse({ closed_reason: reason, github_people: [] }),
      },
      "root",
      "admin",
    );

    renderApp("/d/eng/settings");
    const card = await membersCard();
    expect(
      within(card).getByText(
        "Closed. The GitHub App is not installed on acme/eng: install it there or disconnect the domain.",
      ),
    ).toBeVisible();
  });
});
