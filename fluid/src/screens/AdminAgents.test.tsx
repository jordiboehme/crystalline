/**
 * Administration > Agents.
 *
 * Pinned: the list shows each agent with its loops and its tokens (label,
 * expiry, last use), marks a live token that ends within 14 days, and shows
 * no runs; with `auth.mcp` off the page says so and the create button holds
 * back with that reason and sends nothing; a token is shown once in a field
 * with a copy button and goes when the person presses Done or leaves the
 * page; the server's sentence for a third token is shown; a token is revoked
 * and an agent removed only after the second press.
 */

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { Agent, AgentsResponse } from "../api/model";
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

const DAY_MS = 24 * 60 * 60 * 1000;

/** An instant `days` from now, as the server writes it. */
function inDays(days: number): string {
  return new Date(Date.now() + days * DAY_MS).toISOString();
}

function nightly(): Agent {
  return {
    name: "nightly",
    display: "Nightly runner",
    loops: "both",
    disabled: false,
    created_at: "2026-11-01T10:00:00Z",
    created_by: "root",
    tokens: [
      {
        id: 2,
        label: "soon",
        created_at: "2026-11-01T10:00:00Z",
        expires_at: inDays(3),
        last_used: null,
        revoked_at: null,
        state: "live",
      },
      {
        id: 1,
        label: "old",
        created_at: "2026-10-01T10:00:00Z",
        expires_at: inDays(2),
        last_used: "2026-11-02T08:00:00Z",
        revoked_at: "2026-11-03T08:00:00Z",
        state: "revoked",
      },
    ],
  };
}

function reflector(): Agent {
  return {
    name: "curator",
    display: "curator",
    loops: "reflect",
    disabled: false,
    created_at: "2026-11-01T10:00:00Z",
    created_by: "cli",
    tokens: [
      {
        id: 3,
        label: "far",
        created_at: "2026-11-01T10:00:00Z",
        expires_at: inDays(60),
        last_used: "2026-11-20T08:00:00Z",
        revoked_at: null,
        state: "live",
      },
    ],
  };
}

/** The app signed in as an admin, with the agents list and `routes` served. */
function serve(
  listing: Partial<AgentsResponse> = {},
  routes: Record<string, Answer> = {},
  readOnly = false,
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () =>
        meResponse({
          user: userFixture({ name: "root", role: "admin" }),
          read_only: readOnly,
        }),
      "/domains": domainsResponse,
      "/agents": (_path, init) => {
        const write = routes["/agents"];
        if (write && init?.method === "POST") return write(_path, init);
        return { agents: [nightly(), reflector()], auth_mcp: true, ...listing };
      },
      ...Object.fromEntries(
        Object.entries(routes).filter(([path]) => path !== "/agents"),
      ),
    }),
  );
}

/** The body of the request sent to `path` with `method`, parsed. */
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

/** The card an agent is drawn in. */
async function cardFor(display: string): Promise<HTMLElement> {
  const heading = await screen.findByRole("heading", {
    level: 3,
    name: display,
  });
  const card = heading.closest("section");
  if (!card) throw new Error(`no card for ${display}`);
  return card;
}

beforeEach(() => {
  apiMock.mockReset();
});

describe("the agents page", () => {
  it("lists each agent with its loops and its tokens", async () => {
    serve();
    renderApp("/admin/agents");

    const card = await cardFor("Nightly runner");
    expect(within(card).getByText("nightly")).toBeInTheDocument();
    expect(
      within(card).getByText("Loops: Reflect and evolve"),
    ).toBeInTheDocument();
    const table = within(card).getByRole("table", {
      name: "Tokens of nightly",
    });
    const rows = within(table).getAllByRole("row");
    expect(rows[1]).toHaveTextContent("soon");
    expect(rows[1]).toHaveTextContent("Live");
    expect(rows[1]).toHaveTextContent("Never");
    expect(rows[2]).toHaveTextContent("Revoked");
    const other = await cardFor("curator");
    expect(within(other).getByText("Loops: Reflect")).toBeInTheDocument();
    // No run is shown: run records live per domain.
    expect(screen.queryByText(/last run/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
  });

  it("marks a live token that ends within 14 days, and only that one", async () => {
    serve();
    renderApp("/admin/agents");

    const card = await cardFor("Nightly runner");
    const rows = within(
      within(card).getByRole("table", { name: "Tokens of nightly" }),
    ).getAllByRole("row");
    expect(rows[1]).toHaveTextContent("Ends in 3 days.");
    // Revoked, so it ends nothing, even if its date is near.
    expect(rows[2]).not.toHaveTextContent(/Ends/);
    expect(await cardFor("curator")).not.toHaveTextContent(/Ends/);
  });

  it("says when auth.mcp is off and holds the create button back", async () => {
    serve({ auth_mcp: false });
    renderApp("/admin/agents");

    const note = await screen.findByRole("note", { name: "auth.mcp is off" });
    expect(note).toHaveTextContent("Agents work only with auth.mcp on");
    expect(note).toHaveTextContent("restart the server");
    const create = screen.getByRole("button", { name: "Add agent" });
    expect(create).toHaveAttribute("aria-disabled", "true");
    expect(create).toHaveAccessibleDescription(
      "Agents work only with auth.mcp on, and it is off on this instance.",
    );
    await userEvent.type(screen.getByLabelText("Name"), "nightly2");
    await userEvent.click(create);
    expect(sent("/agents", "POST")).toBe(false);
  });

  it("makes an agent and shows its token once, until Done", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>(() =>
      Promise.resolve(),
    );
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    serve(
      { agents: [] },
      {
        "/agents": () => ({
          agent: { ...reflector(), name: "builder", display: "Builder" },
          token: {
            id: 9,
            label: "issued 2026-11-23",
            expires_at: inDays(90),
            token: "cag_secret",
          },
        }),
      },
    );
    renderApp("/admin/agents");

    expect(await screen.findByText("No agents yet.")).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("Name"), "builder");
    await userEvent.type(screen.getByLabelText("Display name"), "Builder");
    await userEvent.selectOptions(screen.getByLabelText("Loops"), "evolve");
    await userEvent.click(screen.getByRole("button", { name: "Add agent" }));

    const shown = await screen.findByRole("group", {
      name: "Token for builder",
    });
    expect(sentBody("/agents", "POST")).toEqual({
      name: "builder",
      display: "Builder",
      loops: "evolve",
      label: null,
      expires_days: 90,
    });
    const field = within(shown).getByLabelText("Token for builder");
    expect(field).toHaveValue("cag_secret");
    expect(field).toHaveFocus();
    expect(shown).toHaveTextContent("It is not shown again.");
    await userEvent.click(
      within(shown).getByRole("button", { name: "Copy the token" }),
    );
    expect(writeText).toHaveBeenCalledWith("cag_secret");
    await waitFor(() => {
      expect(within(shown).getByRole("status")).toHaveTextContent("Copied");
    });
    // The form is empty again for the next one.
    expect(screen.getByLabelText("Name")).toHaveValue("");

    await userEvent.click(within(shown).getByRole("button", { name: "Done" }));
    expect(screen.queryByDisplayValue("cag_secret")).not.toBeInTheDocument();
  });

  it("forgets a shown token when the person leaves the page", async () => {
    serve(
      {},
      {
        "/agents/nightly/tokens": () => ({
          id: 5,
          label: "second",
          expires_at: inDays(90),
          token: "cag_second",
        }),
      },
    );
    renderApp("/admin/agents");

    const card = await cardFor("Nightly runner");
    await userEvent.type(within(card).getByLabelText("Token label"), "second");
    await userEvent.click(
      within(card).getByRole("button", { name: "Issue a token for nightly" }),
    );
    expect(await screen.findByDisplayValue("cag_second")).toBeInTheDocument();
    expect(sentBody("/agents/nightly/tokens", "POST")).toEqual({
      label: "second",
      expires_days: 90,
    });

    const nav = screen.getByRole("navigation", { name: "Administration" });
    await userEvent.click(within(nav).getByRole("link", { name: "General" }));
    await screen.findByRole("heading", { level: 1, name: "General" });
    await userEvent.click(within(nav).getByRole("link", { name: "Agents" }));
    await cardFor("Nightly runner");
    expect(screen.queryByDisplayValue("cag_second")).not.toBeInTheDocument();
  });

  it("shows the server's sentence when a third token is refused", async () => {
    const sentence =
      "nightly already has 2 live tokens: revoke one with 'crystalline agents revoke' first";
    serve(
      {},
      {
        "/agents/nightly/tokens": () => {
          throw new ApiProblem(409, "Conflict", sentence);
        },
      },
    );
    renderApp("/admin/agents");

    const card = await cardFor("Nightly runner");
    await userEvent.click(
      within(card).getByRole("button", { name: "Issue a token for nightly" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(sentence);
    expect(screen.queryByRole("group", { name: /Token for/ })).toBeNull();
  });

  it("revokes a token only after the second press", async () => {
    serve({}, { "/agents/nightly/tokens/2": () => undefined });
    renderApp("/admin/agents");

    const card = await cardFor("Nightly runner");
    await userEvent.click(
      within(card).getByRole("button", { name: "Revoke soon (token 2)" }),
    );
    expect(sent("/agents/nightly/tokens/2", "DELETE")).toBe(false);
    // The revoked token offers no revoke.
    expect(
      within(card).queryByRole("button", { name: "Revoke old (token 1)" }),
    ).toBeNull();
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm revoke of token 2" }),
    );
    await waitFor(() => {
      expect(sent("/agents/nightly/tokens/2", "DELETE")).toBe(true);
    });
    expect(await screen.findByRole("status")).toHaveTextContent(
      "Revoked soon of nightly.",
    );
  });

  it("keeps the agent when the person steps back from the remove", async () => {
    serve();
    renderApp("/admin/agents");

    const card = await cardFor("Nightly runner");
    expect(card).toHaveTextContent("discards its drafts");
    const trigger = within(card).getByRole("button", {
      name: "Remove nightly",
    });
    await userEvent.click(trigger);
    await userEvent.keyboard("{Escape}");
    expect(trigger).toHaveFocus();
    expect(
      within(card).queryByRole("button", { name: "Confirm remove of nightly" }),
    ).toBeNull();
    expect(sent("/agents/nightly", "DELETE")).toBe(false);
  });

  it("removes an agent after the second press", async () => {
    serve({}, { "/agents/nightly": () => undefined });
    renderApp("/admin/agents");

    const card = await cardFor("Nightly runner");
    await userEvent.click(
      within(card).getByRole("button", { name: "Remove nightly" }),
    );
    await userEvent.click(
      within(card).getByRole("button", { name: "Confirm remove of nightly" }),
    );
    await waitFor(() => {
      expect(sent("/agents/nightly", "DELETE")).toBe(true);
    });
    expect(await screen.findByRole("status")).toHaveTextContent(
      "Removed nightly.",
    );
  });

  it("holds every change back on a read-only instance", async () => {
    serve({}, {}, true);
    renderApp("/admin/agents");

    const card = await cardFor("Nightly runner");
    expect(screen.getByRole("button", { name: "Add agent" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(
      within(card).getByRole("button", { name: "Remove nightly" }),
    ).toHaveAttribute("aria-disabled", "true");
    expect(
      within(card).getByRole("button", { name: "Issue a token for nightly" }),
    ).toHaveAttribute("aria-disabled", "true");
  });
});
