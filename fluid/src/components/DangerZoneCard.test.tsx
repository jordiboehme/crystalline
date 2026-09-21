/**
 * The danger zone: the two ways of taking a domain away from the people who
 * read it, in a box of their own at the foot of the domain page.
 *
 * Both ask for the domain's name to be typed. Unregistering ends the
 * instance's reach into a folder, or deletes a virtual domain's engrams
 * outright; closing a shared domain, or opening a private one, changes who
 * may see everything in it and forgets the membership list on the way. A
 * second press alone is what this app asks for a loss it can describe in one
 * sentence, and neither of these is one.
 *
 * Mounted through the domain screen, the way the members card is, because
 * what the card draws follows the signed-in account's role and the members
 * read rather than anything handed to it.
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

/** The members read, shared by default - the state a domain nobody touched answers with. */
function membersResponse(overrides: Record<string, unknown> = {}) {
  return {
    owner: null,
    visibility: "shared",
    members: [],
    ...overrides,
  };
}

/** The listing as it reads for a domain of the given kind. */
function listingOf(kind: string) {
  const listing = domainsResponse();
  return { ...listing, domains: [{ ...listing.domains[0], kind }] };
}

function serve(
  routes: Record<string, Answer> = {},
  role: Role = "editor",
  name = "ada",
  probe: () => unknown = () =>
    meResponse({ user: userFixture({ name, role }) }),
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": probe,
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
      "/activity": () => ({ timeframe: "7d", items: [] }),
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

/** Every path the app asked for, in order. */
function requested(): string[] {
  return apiMock.mock.calls.map((call) => call[0]);
}

/** The card itself. */
async function dangerZone(): Promise<HTMLElement> {
  return screen.findByRole("region", { name: "Danger zone" });
}

/** Arm a control and type the domain's name into the field it opens. */
async function arm(card: HTMLElement, label: string) {
  await userEvent.click(within(card).getByRole("button", { name: label }));
  await userEvent.type(
    within(card).getByLabelText("Type eng to confirm"),
    "eng",
  );
}

beforeEach(() => {
  apiMock.mockReset();
  localStorage.clear();
});

describe("the danger zone", () => {
  it("unregisters behind the domain's own name, and says the files stay", async () => {
    const removed = vi.fn(() => ({ files_kept: true, rooms_closed: 0 }));
    serve(
      {
        "/domains/eng": (_path, init) =>
          init?.method === "DELETE" ? removed() : domainsResponse(),
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await dangerZone();

    await userEvent.click(
      within(card).getByRole("button", { name: "Unregister domain" }),
    );
    // The first press only asks. Nothing has been unregistered yet, and the
    // second press will not fire on an empty field.
    expect(removed).not.toHaveBeenCalled();
    expect(within(card).getByText(/files stay on disk/i)).toBeVisible();
    const confirm = within(card).getByRole("button", {
      name: "Confirm unregister",
    });
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(confirm);
    expect(removed).not.toHaveBeenCalled();

    await userEvent.type(
      within(card).getByLabelText("Type eng to confirm"),
      "eng",
    );
    await userEvent.click(confirm);

    await waitFor(() => {
      expect(removed).toHaveBeenCalled();
    });
    // The domain the reader was on is gone, so the screen is: home, with the
    // listing every screen reads asked again rather than left one domain long.
    expect(
      await screen.findByRole("heading", { level: 1, name: "Home" }),
    ).toBeVisible();
    await waitFor(() => {
      expect(
        requested().filter((path) => path === "/domains").length,
      ).toBeGreaterThan(1);
    });
  });

  it("draws neither the card nor its controls below admin", async () => {
    serve();

    renderApp("/d/eng");
    await screen.findByRole("heading", { level: 1, name: "eng" });

    expect(screen.queryByRole("region", { name: "Danger zone" })).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Unregister domain" }),
    ).toBeNull();
    expect(screen.queryByRole("button", { name: "Make private" })).toBeNull();
  });

  it("gives a non-admin owner the control that opens its domain back up", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({ owner: "ada", visibility: "private" }),
        "/domains/eng/visibility": (_path, init) => {
          if (init?.method === "PUT") {
            return undefined;
          }
          throw new ApiProblem(405, "method not allowed", "unexpected method");
        },
      },
      "editor",
      "ada",
    );

    renderApp("/d/eng");
    const card = await dangerZone();

    // `set_visibility` asks for `Own` on this direction, which the owner has
    // without being an instance admin: the card is drawn for the one control
    // this caller may actually reach.
    await arm(card, "Share with everyone");
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm share with everyone" }),
    );

    await waitFor(() => {
      expect(sentBody("/domains/eng/visibility", "PUT")).toEqual({
        private: false,
      });
    });
  });

  it("gives a non-admin owner no way to unregister the domain", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({ owner: "ada", visibility: "private" }),
      },
      "editor",
      "ada",
    );

    renderApp("/d/eng");
    const card = await dangerZone();

    // Owning a domain is not administering the instance: `DELETE /domains`
    // is admin-only, so the trigger for it is not drawn beside a control the
    // same caller may use.
    await within(card).findByRole("button", { name: "Share with everyone" });
    expect(
      within(card).queryByRole("button", { name: "Unregister domain" }),
    ).toBeNull();
  });

  it("gives an admin the same control on an ownerless private domain", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({ owner: null, visibility: "private" }),
      },
      "admin",
      "boss",
    );

    renderApp("/d/eng");
    const card = await dangerZone();

    // Nobody owns it, so the owner clause has nothing to match and the right
    // comes from the admin flag alone - which is what gives an ownerless
    // private domain a way back out of private.
    expect(
      await within(card).findByRole("button", { name: "Share with everyone" }),
    ).toBeVisible();
  });

  it("gives a manager of a private domain no visibility control", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({
            owner: "ada",
            visibility: "private",
            members: [
              {
                principal: "mgr",
                level: "manager",
                added_by: "ada",
                added_at: "2026-09-01T00:00:00Z",
              },
            ],
          }),
      },
      "editor",
      "mgr",
    );

    renderApp("/d/eng");
    // The members card is proof the page settled and the read landed: a
    // manager administers the team and never decides who may see the domain.
    await screen.findByRole("region", { name: "Members" });

    expect(screen.queryByRole("region", { name: "Danger zone" })).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Share with everyone" }),
    ).toBeNull();
    expect(screen.queryByRole("button", { name: "Make private" })).toBeNull();
  });

  it("returns focus to the trigger when a refusal blocks the confirm", async () => {
    serve(
      {
        "/domains/eng": (_path, init) => {
          if (init?.method === "DELETE") {
            throw new ApiProblem(
              409,
              "conflict",
              "domain 'eng' is defined by the environment and cannot be unregistered here",
            );
          }
          return domainsResponse();
        },
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await dangerZone();
    const trigger = within(card).getByRole("button", {
      name: "Unregister domain",
    });

    await arm(card, "Unregister domain");
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm unregister" }),
    );

    // The refusal lives in the card's mutation `onError`, which collapses the
    // confirmation without reaching the control's own trigger ref; the fix is
    // a transition-aware effect in `DestructiveAction`, not a copy of
    // `abandon()`'s one-liner.
    const alert = await within(card).findByRole("alert");
    expect(alert).toHaveTextContent(/cannot be unregistered/);
    // The confirm buttons unmounted with the refusal, which would otherwise
    // drop focus to the document body - a keyboard or screen-reader user
    // loses their place entirely. Identity, not merely "not the trigger".
    expect(document.activeElement).toBe(trigger);
  });

  it("leaves focus where the blur path put it, not on the trigger", async () => {
    serve({}, "admin");

    renderApp("/d/eng");
    const card = await dangerZone();
    const trigger = within(card).getByRole("button", {
      name: "Unregister domain",
    });

    await userEvent.click(trigger);
    const field = within(card).getByLabelText("Type eng to confirm");
    expect(field).toHaveFocus();

    // Shift-tab out of the confirm row entirely: the first hop stays inside
    // it (the field and the trigger are under the same wrapper), the second
    // leaves it for the import button in the backup card above. That
    // crossing is what the wrapper's own `onBlur` collapses the confirmation
    // for - deliberately, because focus moved somewhere else on purpose.
    await userEvent.tab({ shift: true });
    expect(trigger).toHaveFocus();
    await userEvent.tab({ shift: true });
    const importButton = screen.getByRole("button", {
      name: "Import archive",
    });

    // The counterweight: a fix that steals focus back to the trigger whenever
    // the confirmation goes away would pass a check that only asserts "not
    // the trigger" on a jsdom that parks focus on the body mid-blur.
    expect(document.activeElement).toBe(importButton);
    expect(
      within(card).queryByRole("button", { name: "Confirm unregister" }),
    ).toBeNull();
  });

  it("warns that a virtual domain's engrams go with it", async () => {
    serve({ "/domains": () => listingOf("virtual") }, "admin");

    renderApp("/d/eng");
    const card = await dangerZone();

    await userEvent.click(
      within(card).getByRole("button", { name: "Unregister domain" }),
    );

    // Nothing stays on disk here, so nothing here says it does: the engrams
    // are the database's, and the way to keep a copy is named.
    expect(within(card).getByText(/live in the database/i)).toBeVisible();
    expect(within(card).getByText(/cannot be undone/i)).toBeVisible();
    expect(within(card).getByText(/download the archive first/i)).toBeVisible();
    expect(within(card).queryByText(/files stay on disk/i)).toBeNull();
  });

  it("sends the purge confirmation for a virtual domain", async () => {
    const deletes: string[] = [];
    serve(
      {
        "/domains": () => listingOf("virtual"),
        "/domains/eng": (path, init) => {
          if (init?.method !== "DELETE") {
            return domainsResponse();
          }
          deletes.push(path);
          return { files_kept: false, rooms_closed: 0 };
        },
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await dangerZone();
    await arm(card, "Unregister domain");
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm unregister" }),
    );

    // The typed name IS the confirmation the server asks for, so it travels
    // with the request rather than being re-collected server-side.
    await waitFor(() => {
      expect(deletes.length).toBe(1);
    });
    expect(deletes[0]).toContain("purge=true");
  });

  it("sends no purge confirmation for a file domain", async () => {
    const deletes: string[] = [];
    serve(
      {
        "/domains": () => listingOf("file"),
        "/domains/eng": (path, init) => {
          if (init?.method !== "DELETE") {
            return domainsResponse();
          }
          deletes.push(path);
          return { files_kept: true, rooms_closed: 0 };
        },
      },
      "admin",
    );

    renderApp("/d/eng");
    const card = await dangerZone();
    await arm(card, "Unregister domain");
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm unregister" }),
    );

    // Nothing is deleted here, so nothing is confirmed: a file domain's
    // markdown survives the removal and the flag would be meaningless.
    await waitFor(() => {
      expect(deletes.length).toBe(1);
    });
    expect(deletes[0]).not.toContain("purge");
  });

  it("arms the confirmation from the command palette's own row", async () => {
    serve({}, "admin");

    renderApp("/d/eng");
    const card = await dangerZone();

    const user = userEvent.setup();
    await user.keyboard("{Meta>}k{/Meta}");
    const dialog = await screen.findByRole("dialog");
    const row = await within(dialog).findByRole("option", {
      name: /Unregister domain/,
    });
    await user.click(row);

    // The palette row does what the button does, which is to ASK: the
    // keyboard route lands on the same typed field rather than skipping the
    // step the control exists for.
    expect(
      await within(card).findByLabelText("Type eng to confirm"),
    ).toBeVisible();
  });

  it("focuses the typed field once the palette arms it, after the palette's own focus restore", async () => {
    serve({}, "admin");

    renderApp("/d/eng");
    const card = await dangerZone();

    const user = userEvent.setup();
    await user.keyboard("{Meta>}k{/Meta}");
    const dialog = await screen.findByRole("dialog");
    const row = await within(dialog).findByRole("option", {
      name: /Unregister domain/,
    });
    await user.click(row);

    // The closing palette restores focus to its own trigger in a later
    // passive cleanup; without an explicit focus call in the same effect as
    // the scroll, that restore would win the race and strand focus off the
    // field the confirmation exists to have typed into.
    const field = await within(card).findByLabelText("Type eng to confirm");
    expect(document.activeElement).toBe(field);
  });

  it("closes a shared domain once the name is typed, with the disk-truth caption", async () => {
    serve(
      {
        "/domains/eng/visibility": (_path, init) => {
          if (init?.method === "PUT") {
            return undefined;
          }
          throw new ApiProblem(405, "method not allowed", "unexpected method");
        },
      },
      "admin",
      "boss",
    );

    renderApp("/d/eng");
    const card = await dangerZone();

    // Which way the control points is the members read's answer, so it lands
    // with that read rather than with the card.
    expect(
      await within(card).findByText(
        "Private domains protect from other users of this instance, not from whoever operates the machine.",
      ),
    ).toBeVisible();
    await userEvent.click(
      within(card).getByRole("button", { name: "Make private" }),
    );
    const confirm = within(card).getByRole("button", {
      name: "Confirm make private",
    });
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await userEvent.type(
      within(card).getByLabelText("Type eng to confirm"),
      "eng",
    );
    await userEvent.click(confirm);

    await waitFor(() => {
      expect(sentBody("/domains/eng/visibility", "PUT")).toEqual({
        private: true,
      });
    });
    expect(
      await within(card).findByText("This domain is private now."),
    ).toBeVisible();
  });

  it("opens a private domain back up once the name is typed", async () => {
    serve(
      {
        "/domains/eng/members": () =>
          membersResponse({ owner: "boss", visibility: "private" }),
        "/domains/eng/visibility": (_path, init) => {
          if (init?.method === "PUT") {
            return undefined;
          }
          throw new ApiProblem(405, "method not allowed", "unexpected method");
        },
      },
      "admin",
      "boss",
    );

    renderApp("/d/eng");
    const card = await dangerZone();

    expect(
      await within(card).findByText(
        "Opening this domain forgets who was invited into it.",
      ),
    ).toBeVisible();
    await userEvent.click(
      within(card).getByRole("button", { name: "Share with everyone" }),
    );
    const confirm = within(card).getByRole("button", {
      name: "Confirm share with everyone",
    });
    // The other direction asks exactly as much: this one throws the
    // membership list away.
    expect(confirm).toHaveAttribute("aria-disabled", "true");
    await userEvent.type(
      within(card).getByLabelText("Type eng to confirm"),
      "eng",
    );
    await userEvent.click(confirm);

    await waitFor(() => {
      expect(sentBody("/domains/eng/visibility", "PUT")).toEqual({
        private: false,
      });
    });
    expect(
      await within(card).findByText("This domain is shared with everyone."),
    ).toBeVisible();
  });

  it("renders the server's own refusal of a visibility change", async () => {
    serve(
      {
        "/domains/eng/visibility": (_path, init) => {
          if (init?.method === "PUT") {
            throw new ApiProblem(
              403,
              "forbidden",
              "your membership on this domain is viewer",
            );
          }
          throw new ApiProblem(405, "method not allowed", "unexpected method");
        },
      },
      "admin",
      "boss",
    );

    renderApp("/d/eng");
    const card = await dangerZone();

    await within(card).findByRole("button", { name: "Make private" });
    await arm(card, "Make private");
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm make private" }),
    );

    expect(await within(card).findByRole("alert")).toHaveTextContent(
      /membership on this domain is viewer/,
    );
  });

  it("keeps both controls inert, and says why, on a read-only instance", async () => {
    serve({}, "admin", "boss", () =>
      meResponse({
        user: userFixture({ name: "boss", role: "admin" }),
        read_only: true,
      }),
    );

    renderApp("/d/eng");
    const card = await dangerZone();

    const trigger = await within(card).findByRole("button", {
      name: "Make private",
    });
    // `aria-disabled`, not the native attribute: a control taken out of the
    // tab order can never be landed on, so the reason could never be heard.
    expect(trigger).not.toBeDisabled();
    expect(trigger).toHaveAttribute("aria-disabled", "true");
    expect(trigger).toHaveAccessibleDescription(
      "This instance is read only, so nothing here can be changed.",
    );

    const before = apiMock.mock.calls.length;
    await userEvent.click(trigger);
    expect(apiMock.mock.calls.length).toBe(before);
    expect(
      within(card).queryByRole("button", { name: "Confirm make private" }),
    ).toBeNull();

    // Unregister sits beside it in the same card, and a read-only instance
    // shuts both controls with the same reason rather than leaving one live.
    const unregisterTrigger = within(card).getByRole("button", {
      name: "Unregister domain",
    });
    expect(unregisterTrigger).not.toBeDisabled();
    expect(unregisterTrigger).toHaveAttribute("aria-disabled", "true");
    expect(unregisterTrigger).toHaveAccessibleDescription(
      "This instance is read only, so nothing here can be changed.",
    );

    await userEvent.click(unregisterTrigger);
    expect(apiMock.mock.calls.length).toBe(before);
    expect(
      within(card).queryByRole("button", { name: "Confirm unregister" }),
    ).toBeNull();
  });
});
