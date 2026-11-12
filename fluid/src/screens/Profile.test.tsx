/**
 * The profile screen, which is two cards: the GitHub identity this account
 * shares as, and the MCP tokens an agent authenticates the daemon with, acting
 * as this account.
 *
 * The GitHub half pins what somebody about to share has to be able to trust:
 * that both ways in are on offer, that the device flow shows the code and where
 * to type it, that a token typed in is sent once and left nowhere, that the
 * connected card names the account and since when, and that a refusal - a
 * viewer's, or a sign-in somebody else already started - is the server's own
 * sentence rather than a house message pasted over it.
 *
 * The agent access half pins the opposite direction: that the card is offered
 * to a viewer exactly as it is to an editor and never gated by an instance's
 * read-only setting, that a freshly issued or rotated secret is shown exactly
 * once and gone from the DOM the moment its dialog is dismissed, that the
 * listing never carries the secret at all, that every one of its four failure
 * surfaces shows the server's own words, that a revoke abandoned by Escape or
 * Keep hands focus back to the row's own Revoke button, and that a failed
 * issue is never retried into a second, unseen token.
 */

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiProblem, api } from "../api/client";
import type { MeResponse } from "../api/model";
import type { Answer } from "../test/harness";
import {
  answersFor,
  domainsResponse,
  meResponse,
  renderApp,
  userFixture,
} from "../test/harness";
import App from "../App";
import { navigateTo } from "../base";
import { MemoryRouter, useLocation } from "react-router";
import { Tooltips } from "../components/primitives";
import { AgentAccessCard } from "./Profile";
import { grantEndedText } from "./grantEnded";

vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return { ...actual, api: vi.fn(), setCsrfToken: vi.fn() };
});

// A whole-page navigation jsdom cannot follow; the tests assert where it
// would have gone.
vi.mock("../base", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../base")>();
  return { ...actual, navigateTo: vi.fn() };
});

/** The address the router is at, for tests that check it was rewritten. */
function LocationProbe() {
  const location = useLocation();
  return (
    <output data-testid="location">{`${location.pathname}${location.search}`}</output>
  );
}

const apiMock = vi.mocked(api);
const navigateMock = vi.mocked(navigateTo);

/** The wire shape of the App link, in whichever state a test needs. */
function linkView(overrides: Record<string, unknown> = {}) {
  return {
    linked: false,
    login: null,
    github_id: null,
    host: null,
    linked_at: null,
    token_live: false,
    pending: null,
    error: null,
    app_configured: true,
    ...overrides,
  };
}

/** An account linked through the App, with a working token. */
const LINKED = linkView({
  linked: true,
  login: "ada-gh",
  github_id: 501,
  host: "github.com",
  linked_at: "2026-10-28T09:12:44Z",
  token_live: true,
});

/** The wire shape of an identity, in whichever state a test needs. */
function identityPayload(overrides: Record<string, unknown> = {}) {
  return {
    account: "ada",
    connected: false,
    login: null,
    connected_at: null,
    token_store: null,
    pending: null,
    error: null,
    ...overrides,
  };
}

/** The identity as it stands once a credential is on file. */
const CONNECTED = identityPayload({
  connected: true,
  login: "octo",
  connected_at: "2026-08-29T09:12:44Z",
  token_store: "keyring",
});

/** A device flow waiting for its browser half. */
const PENDING = {
  user_code: "ABCD-1234",
  verification_url: "https://github.example/device",
  expires_in_secs: 900,
};

/** The app, signed in as `ada` in the given role. */
function serveAs(
  role: "admin" | "editor" | "viewer",
  routes: Record<string, Answer> = {},
  me: Partial<MeResponse> = {},
) {
  apiMock.mockImplementation(
    answersFor({
      "/auth/me": () => meResponse({ user: userFixture({ role }), ...me }),
      "/domains": domainsResponse,
      "/me/github-identity": () => identityPayload(),
      // No GitHub App by default, which keeps the older identity card.
      "/me/github-link": () => linkView({ app_configured: false }),
      "/me/mcp-tokens": () => [],
      "/me/oauth-grants": () => [],
      // The default is an instance with no provider and an account with no
      // link, which is the shape in which the SSO card is not there at all.
      "/auth/providers": () => ({ local: true, oidc: { enabled: false } }),
      "/me/identity-links": () => ({ links: [], has_password: true }),
      ...routes,
    }),
  );
}

/** An instance with a provider configured, for the SSO card's own tests. */
function withProvider(routes: Record<string, Answer> = {}) {
  return {
    "/auth/providers": () => ({
      local: true,
      oidc: { enabled: true, name: "Contoso" },
    }),
    ...routes,
  };
}

/** One link, as the listing hands it back. */
function linkPayload(overrides: Record<string, unknown> = {}) {
  return {
    issuer: "https://idp.example",
    subject: "sub-1",
    linked_at: "2026-09-01T09:12:44Z",
    linked_by: "ada",
    ...overrides,
  };
}

/** Every call the app made to the personal identity surface. */
function identityCalls(): unknown[] {
  return apiMock.mock.calls.filter(([path]) =>
    String(path).startsWith("/me/github-identity"),
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

beforeEach(() => {
  apiMock.mockReset();
  navigateMock.mockReset();
});

describe("the profile screen", () => {
  it("is offered to every signed-in account, inside the identity menu", async () => {
    serveAs("editor");
    renderApp("/profile");

    const user = userEvent.setup();
    await user.click(
      await screen.findByRole("button", { name: "Ada Lovelace" }),
    );
    expect(
      await screen.findByRole("menuitem", { name: "Profile" }),
    ).toHaveAttribute("href", "/profile");
  });

  it("shows the disconnected card with both connect paths", async () => {
    serveAs("editor");
    renderApp("/profile");

    expect(
      await screen.findByRole("heading", { name: "GitHub identity" }),
    ).toBeInTheDocument();
    expect(await screen.findByText(/not connected/i)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /connect with github/i }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText(/personal access token/i)).toHaveAttribute(
      "type",
      "password",
    );
    expect(
      screen.getByRole("button", { name: /connect with token/i }),
    ).toBeInTheDocument();
  });

  it("starts the device flow and shows code, link and polling state", async () => {
    let identity = identityPayload();
    serveAs("editor", {
      "/me/github-identity": () => identity,
      "/me/github-identity/connect": () => {
        identity = identityPayload({ pending: PENDING });
        return identity;
      },
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: /connect with github/i }),
    );

    expect(await screen.findByText("ABCD-1234")).toBeInTheDocument();
    const link = screen.getByRole("link", {
      name: /open github\.com and enter the code/i,
    });
    expect(link).toHaveAttribute("href", "https://github.example/device");
    expect(link).toHaveAttribute("target", "_blank");
    expect(
      screen.getByText(/waiting for the browser confirmation/i),
    ).toBeInTheDocument();
  });

  /**
   * The half the flow's own screen cannot see: the confirmation happens in
   * another window, so nothing tells this card it happened and the poll is
   * what learns it. No click anywhere - the card is rendered already waiting,
   * which is the state the server reports to a browser that reloaded mid-flow
   * as much as to the one that started it.
   *
   * On the real clock, and the generous timeouts are why: the card polls every
   * three seconds, and testing-library's waiter does not recognise vitest's
   * fake clock (it looks for jest's), so a faked one would never be advanced
   * by the wait and the test would hang rather than run fast. One real poll
   * interval is the price of covering the one thing this card does that
   * nothing else can cover for it.
   */
  it("polls a pending sign-in through to connected", async () => {
    let identity: Record<string, unknown> = identityPayload({
      pending: PENDING,
    });
    serveAs("editor", { "/me/github-identity": () => identity });
    renderApp("/profile");

    expect(await screen.findByText("ABCD-1234")).toBeInTheDocument();

    // Confirmed in the other window.
    identity = CONNECTED;

    expect(
      await screen.findByText(/connected as @octo/i, {}, { timeout: 8000 }),
    ).toBeInTheDocument();
    expect(screen.queryByText("ABCD-1234")).not.toBeInTheDocument();
    expect(
      screen.queryByText(/waiting for the browser confirmation/i),
    ).not.toBeInTheDocument();
  }, 15000);

  it("surfaces a sign-in somebody else already started, in the server's words", async () => {
    serveAs("editor", {
      "/me/github-identity/connect": () => {
        throw new ApiProblem(
          409,
          "conflict",
          "another sign-in is in progress on this instance: wait for it to finish, then start yours again",
        );
      },
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: /connect with github/i }),
    );

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/another sign-in is in progress/i);
    // And the card stays where it was: nothing to type, nothing to confirm.
    expect(screen.queryByText("ABCD-1234")).not.toBeInTheDocument();
  });

  it("pastes a token and shows the identity it stored", async () => {
    let identity: Record<string, unknown> = identityPayload();
    serveAs("editor", {
      "/me/github-identity": (_path, init) => {
        if (init?.method === "PUT") {
          throw new Error("the token goes to its own route");
        }
        return identity;
      },
      "/me/github-identity/token": () => {
        identity = CONNECTED;
        return identity;
      },
    });
    renderApp("/profile");

    const field = await screen.findByLabelText(/personal access token/i);
    await userEvent.type(field, "ghp_secret");
    await userEvent.click(
      screen.getByRole("button", { name: /connect with token/i }),
    );

    await waitFor(() => {
      expect(sentBody("/me/github-identity/token", "PUT")).toEqual({
        token: "ghp_secret",
      });
    });
    // The field empties only once the server took it, and the card then names
    // the account, since when, and where the credential lives.
    await waitFor(() => {
      expect(field).toHaveValue("");
    });
    expect(await screen.findByText(/connected as @octo/i)).toBeInTheDocument();
    expect(screen.getByText(/since 2026-08-29/i)).toHaveTextContent(/keyring/i);
  });

  it("keeps the token when the server refuses it", async () => {
    serveAs("editor", {
      "/me/github-identity/token": () => {
        throw new ApiProblem(
          422,
          "unprocessable entity",
          "GitHub refused that token: it may be expired or lack the repo scope",
        );
      },
    });
    renderApp("/profile");

    const field = await screen.findByLabelText(/personal access token/i);
    await userEvent.type(field, "ghp_stale");
    await userEvent.click(
      screen.getByRole("button", { name: /connect with token/i }),
    );

    expect(await screen.findByText(/lack the repo scope/i)).toBeInTheDocument();
    expect(field).toHaveValue("ghp_stale");
  });

  it("disconnects behind a two-step confirm", async () => {
    let identity: Record<string, unknown> = CONNECTED;
    const forgotten = vi.fn(() => {
      identity = identityPayload();
      return identity;
    });
    serveAs("editor", {
      "/me/github-identity": (_path, init) =>
        init?.method === "DELETE" ? forgotten() : identity,
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Disconnect" }),
    );
    expect(forgotten).not.toHaveBeenCalled();

    await userEvent.click(
      screen.getByRole("button", { name: /confirm disconnect/i }),
    );
    await waitFor(() => {
      expect(forgotten).toHaveBeenCalled();
    });
    expect(await screen.findByText(/not connected/i)).toBeInTheDocument();
  });

  it("tells a viewer that sharing is not theirs, and asks the server nothing", async () => {
    serveAs("viewer");
    renderApp("/profile");

    expect(
      await screen.findByText(/sharing is not available for viewer accounts/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /connect with github/i }),
    ).not.toBeInTheDocument();
    // The route refuses a viewer anyway, so the card does not go and ask.
    expect(identityCalls()).toHaveLength(0);
  });

  it("offers no way to connect on a read-only instance", async () => {
    serveAs(
      "editor",
      { "/me/github-identity": () => CONNECTED },
      { read_only: true },
    );
    renderApp("/profile");

    // The read still works, so the card says what is on file.
    expect(await screen.findByText(/connected as @octo/i)).toBeInTheDocument();
    // Every verb that would change it is refused by the server, so none of
    // them is offered: the app draws no door that will not open.
    expect(
      screen.queryByRole("button", { name: /connect with github/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText(/personal access token/i),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Disconnect" }),
    ).not.toBeInTheDocument();
    expect(
      await screen.findByText(/nothing here can be connected or disconnected/i),
    ).toBeInTheDocument();
  });
});

describe("the agent access card", () => {
  it("lists the caller's own tokens, never a secret", async () => {
    serveAs("editor", {
      "/me/mcp-tokens": () => [
        {
          id: 1,
          label: "laptop",
          created_at: "2026-08-29T09:12:44Z",
          last_used: "2026-09-01T10:00:00Z",
        },
        {
          id: 2,
          label: "ci",
          created_at: "2026-08-20T00:00:00Z",
          last_used: null,
        },
      ],
    });
    renderApp("/profile");

    expect(
      await screen.findByRole("heading", { name: "Agent access" }),
    ).toBeInTheDocument();
    expect(await screen.findByText("laptop")).toBeInTheDocument();
    expect(screen.getByText("2026-08-29")).toBeInTheDocument();
    expect(screen.getByText("ci")).toBeInTheDocument();
    expect(screen.getByText("Never")).toBeInTheDocument();
    expect(screen.queryByText(/cmt_/)).not.toBeInTheDocument();
  });

  it("says so when no token has been issued yet", async () => {
    serveAs("viewer");
    renderApp("/profile");

    expect(
      await screen.findByText(/no tokens issued yet/i),
    ).toBeInTheDocument();
  });

  it("is offered to a viewer too, since an agent acts as its user", async () => {
    serveAs("viewer");
    renderApp("/profile");

    expect(
      await screen.findByRole("heading", { name: "Agent access" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Issue token" }),
    ).toBeInTheDocument();
  });

  it("issues a token, reveals it exactly once, and never again after it is dismissed", async () => {
    let tokens: Record<string, unknown>[] = [];
    serveAs("editor", {
      "/me/mcp-tokens": (_path, init) => {
        if (init?.method === "POST") {
          tokens = [
            {
              id: 9,
              label: "laptop",
              created_at: "2026-09-07T00:00:00Z",
              last_used: null,
            },
          ];
          return { id: 9, label: "laptop", token: "cmt_deadbeef" };
        }
        return tokens;
      },
    });
    renderApp("/profile");

    const field = await screen.findByLabelText("Label");
    await userEvent.type(field, "laptop");
    await userEvent.click(screen.getByRole("button", { name: "Issue token" }));

    await waitFor(() => {
      expect(sentBody("/me/mcp-tokens", "POST")).toEqual({ label: "laptop" });
    });
    // The field clears once the server took the label, the same rule the
    // GitHub token field above follows.
    await waitFor(() => {
      expect(field).toHaveValue("");
    });

    const dialog = await screen.findByRole("dialog", { name: "laptop" });
    expect(within(dialog).getByText("cmt_deadbeef")).toBeInTheDocument();
    expect(
      within(dialog).getByText(
        (_text, node) =>
          node?.textContent ===
          "Add this as header Authorization: Bearer cmt_deadbeef to the crystalline entry in your agent's MCP registration.",
      ),
    ).toBeInTheDocument();

    await userEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    // Dismissed, and gone from the DOM for good - the secret is held nowhere
    // this screen could show it back from.
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.queryByText("cmt_deadbeef")).not.toBeInTheDocument();
    expect(await screen.findByText("laptop")).toBeInTheDocument();
  });

  it("rotates a token and reveals the fresh secret", async () => {
    const listing = [
      {
        id: 3,
        label: "laptop",
        created_at: "2026-08-01T00:00:00Z",
        last_used: null,
      },
    ];
    serveAs("editor", {
      "/me/mcp-tokens": () => listing,
      "/me/mcp-tokens/3/rotate": () => ({
        id: 3,
        label: "laptop",
        token: "cmt_freshbeef",
      }),
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Rotate laptop" }),
    );

    const dialog = await screen.findByRole("dialog", { name: "laptop" });
    expect(within(dialog).getByText("cmt_freshbeef")).toBeInTheDocument();
  });

  it("revokes a token behind a two-step confirm", async () => {
    let listing = [
      {
        id: 5,
        label: "laptop",
        created_at: "2026-08-01T00:00:00Z",
        last_used: null,
      },
    ];
    const revoked = vi.fn(() => {
      listing = [];
    });
    serveAs("editor", {
      "/me/mcp-tokens": () => listing,
      "/me/mcp-tokens/5": (_path, init) => {
        if (init?.method === "DELETE") {
          revoked();
        }
        return undefined;
      },
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Revoke laptop" }),
    );
    expect(revoked).not.toHaveBeenCalled();

    await userEvent.click(
      screen.getByRole("button", { name: "Confirm revoke laptop" }),
    );
    await waitFor(() => {
      expect(revoked).toHaveBeenCalled();
    });
    expect(
      await screen.findByText(/no tokens issued yet/i),
    ).toBeInTheDocument();
  });

  it("offers issue, rotate and revoke on a read-only instance too, since a token is account state rather than knowledge", async () => {
    serveAs(
      "editor",
      {
        "/me/mcp-tokens": () => [
          {
            id: 1,
            label: "laptop",
            created_at: "2026-08-01T00:00:00Z",
            last_used: null,
          },
        ],
      },
      { read_only: true },
    );
    renderApp("/profile");

    // The read-only setting protects the knowledge base; it says nothing
    // about a token, which is unrelated to it - every control here is drawn.
    expect(await screen.findByText("laptop")).toBeInTheDocument();
    expect(screen.getByLabelText("Label")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Issue token" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Rotate laptop" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Revoke laptop" }),
    ).toBeInTheDocument();
  });

  it("shows the server's own words when issuing a token is refused", async () => {
    serveAs("editor", {
      "/me/mcp-tokens": (_path, init) => {
        if (init?.method === "POST") {
          throw new ApiProblem(
            422,
            "unprocessable entity",
            "you already hold the maximum number of tokens",
          );
        }
        return [];
      },
    });
    renderApp("/profile");

    const field = await screen.findByLabelText("Label");
    await userEvent.type(field, "laptop");
    await userEvent.click(screen.getByRole("button", { name: "Issue token" }));

    expect(
      await screen.findByText(/you already hold the maximum/i),
    ).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows the server's own words when rotating a token is refused", async () => {
    serveAs("editor", {
      "/me/mcp-tokens": () => [
        {
          id: 4,
          label: "laptop",
          created_at: "2026-08-01T00:00:00Z",
          last_used: null,
        },
      ],
      "/me/mcp-tokens/4/rotate": () => {
        throw new ApiProblem(
          404,
          "not found",
          "no such MCP token: it may already have been revoked",
        );
      },
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Rotate laptop" }),
    );

    expect(await screen.findByText(/no such mcp token/i)).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows the server's own words when revoking a token is refused", async () => {
    serveAs("editor", {
      "/me/mcp-tokens": () => [
        {
          id: 6,
          label: "laptop",
          created_at: "2026-08-01T00:00:00Z",
          last_used: null,
        },
      ],
      "/me/mcp-tokens/6": (_path, init) => {
        if (init?.method === "DELETE") {
          throw new ApiProblem(
            404,
            "not found",
            "no such MCP token: it may already have been revoked",
          );
        }
        return undefined;
      },
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Revoke laptop" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Confirm revoke laptop" }),
    );

    expect(await screen.findByText(/no such mcp token/i)).toBeInTheDocument();
    // Refused, so the row nothing happened to is still there.
    expect(screen.getByText("laptop")).toBeInTheDocument();
  });

  it("shows the server's own words when the listing itself fails", async () => {
    serveAs("editor", {
      "/me/mcp-tokens": () => {
        throw new ApiProblem(
          500,
          "internal error",
          "the account store could not be read",
        );
      },
    });
    renderApp("/profile");

    expect(
      await screen.findByText(/the account store could not be read/i),
    ).toBeInTheDocument();
  });

  it("returns focus to the row's own Revoke button after Escape and after Keep", async () => {
    serveAs("editor", {
      "/me/mcp-tokens": () => [
        {
          id: 7,
          label: "laptop",
          created_at: "2026-08-01T00:00:00Z",
          last_used: null,
        },
      ],
    });
    renderApp("/profile");

    const trigger = await screen.findByRole("button", {
      name: "Revoke laptop",
    });

    await userEvent.click(trigger);
    expect(
      screen.getByRole("button", { name: "Confirm revoke laptop" }),
    ).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(
      screen.queryByRole("button", { name: "Confirm revoke laptop" }),
    ).not.toBeInTheDocument();
    // The trigger is rendered unconditionally for exactly this: its ref stays
    // live while confirming, so abandoning has something real to focus.
    expect(trigger).toHaveFocus();

    await userEvent.click(trigger);
    await userEvent.click(screen.getByRole("button", { name: "Keep" }));
    expect(trigger).toHaveFocus();
  });

  /**
   * The client's default retries once, after roughly a second, for anything
   * that is not a 4xx - real time, since testing-library's waiter does not
   * recognise vitest's fake clock. `retry: false` on `issue` is what keeps a
   * dropped connection from minting a second, unseen token; this fails
   * without it.
   */
  it("does not retry a failed issue, so a dropped connection never mints a second token", async () => {
    const attempts = vi.fn();
    serveAs("editor", {
      "/me/mcp-tokens": (_path, init) => {
        if (init?.method === "POST") {
          attempts();
          throw new ApiProblem(
            0,
            "network error",
            "could not reach the server: it may be down",
          );
        }
        return [];
      },
    });
    renderApp("/profile");

    const field = await screen.findByLabelText("Label");
    await userEvent.type(field, "laptop");
    await userEvent.click(screen.getByRole("button", { name: "Issue token" }));

    expect(
      await screen.findByText(/could not reach the server/i),
    ).toBeInTheDocument();
    expect(attempts).toHaveBeenCalledTimes(1);

    await new Promise((resolve) => setTimeout(resolve, 1500));
    expect(attempts).toHaveBeenCalledTimes(1);
  }, 8000);

  it("never lets the issued secret become a value React Query itself retains", async () => {
    apiMock.mockImplementation(
      answersFor({
        "/me/mcp-tokens": (_path, init) => {
          if (init?.method === "POST") {
            return { id: 11, label: "laptop", token: "cmt_deadbeef" };
          }
          return [];
        },
      }),
    );
    const client = new QueryClient({
      defaultOptions: { mutations: { retry: false } },
    });

    render(
      <QueryClientProvider client={client}>
        <Tooltips>
          <AgentAccessCard />
        </Tooltips>
      </QueryClientProvider>,
    );

    const field = await screen.findByLabelText("Label");
    await userEvent.type(field, "laptop");
    await userEvent.click(screen.getByRole("button", { name: "Issue token" }));
    await screen.findByRole("dialog", { name: "laptop" });

    // The mutation cache is inspected directly - not the DOM - because the
    // point is what React Query itself retains, which the screen could not
    // reveal either way.
    expect(JSON.stringify(client.getMutationCache().getAll())).not.toContain(
      "cmt_deadbeef",
    );
  });

  // The race behind a red `mcp-tokens.spec.ts` in CI: the list's first read
  // is slow, the person issues a token before it answers, and that read,
  // taken before the insert, answers last with no rows. A bare invalidate
  // joined it and the list said "No tokens issued yet" until focus came back.
  it("lists a token issued while the first read of the list is still on the wire", async () => {
    let rows: Record<string, unknown>[] = [];
    let reads = 0;
    let answerFirstRead!: () => void;
    apiMock.mockImplementation(
      answersFor({
        "/me/mcp-tokens": (_path, init) => {
          if (init?.method === "POST") {
            rows = [
              {
                id: 9,
                label: "laptop",
                created_at: "2026-09-07T00:00:00Z",
                last_used: null,
              },
            ];
            return { id: 9, label: "laptop", token: "cmt_deadbeef" };
          }
          reads += 1;
          const snapshot = [...rows];
          if (reads > 1) {
            return snapshot;
          }
          return new Promise((resolve) => {
            answerFirstRead = () => resolve(snapshot);
          });
        },
      }),
    );
    const client = new QueryClient({
      defaultOptions: {
        queries: { retry: false },
        mutations: { retry: false },
      },
    });

    render(
      <QueryClientProvider client={client}>
        <Tooltips>
          <AgentAccessCard />
        </Tooltips>
      </QueryClientProvider>,
    );

    expect(await screen.findByText("Reading your tokens")).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("Label"), "laptop");
    await userEvent.click(screen.getByRole("button", { name: "Issue token" }));
    const dialog = await screen.findByRole("dialog", { name: "laptop" });
    await userEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    // Only now does the first read answer, with the list as it stood
    // before the insert.
    answerFirstRead();

    // The dialog is gone, so this is the row in the list and not the
    // dialog's own title.
    expect(await screen.findByText("laptop")).toBeInTheDocument();
    expect(screen.queryByText(/no tokens issued yet/i)).not.toBeInTheDocument();
    expect(reads).toBe(2);
  });
});

describe("the connected clients card", () => {
  it("lists grants with their client and last use", async () => {
    serveAs(
      "editor",
      {
        "/me/oauth-grants": () => [
          {
            id: 1,
            client_id: "coc_1a2b3c",
            client_name: "Claude",
            redirect_host: "claude.ai",
            created_at: "2026-08-29T09:12:44Z",
            last_used: "2026-09-01T10:00:00Z",
            refresh_expires_at: "2026-10-08T04:00:00Z",
          },
          {
            id: 2,
            client_id: "coc_4d5e6f",
            client_name: "a local agent",
            redirect_host: "127.0.0.1:51902",
            created_at: "2026-08-20T00:00:00Z",
            last_used: null,
            refresh_expires_at: "2026-10-20T00:00:00Z",
          },
        ],
      },
      { oauth: true },
    );
    renderApp("/profile");

    expect(
      await screen.findByRole("heading", { name: "Connected clients" }),
    ).toBeInTheDocument();
    expect(await screen.findByText("Claude")).toBeInTheDocument();
    expect(screen.getByText("claude.ai")).toBeInTheDocument();
    expect(screen.getByText("2026-08-29")).toBeInTheDocument();
    expect(screen.getByText("2026-09-01")).toBeInTheDocument();
    expect(screen.getByText("a local agent")).toBeInTheDocument();
    expect(screen.getByText("127.0.0.1:51902")).toBeInTheDocument();
    expect(screen.getByText("Never")).toBeInTheDocument();
  });

  it("asks for the ended grants too", async () => {
    serveAs("editor", {}, { oauth: true });
    renderApp("/profile");

    await screen.findByRole("heading", { name: "Connected clients" });
    await waitFor(() => {
      expect(
        apiMock.mock.calls.some(
          ([path]) => path === "/me/oauth-grants?include_ended=true",
        ),
      ).toBe(true);
    });
  });

  it("says once why a client is gone, under the live ones", async () => {
    serveAs(
      "editor",
      {
        "/me/oauth-grants": () => [
          {
            id: 3,
            client_id: "coc_live",
            client_name: "Claude",
            redirect_host: "claude.ai",
            created_at: "2026-11-01T00:00:00Z",
            last_used: null,
            refresh_expires_at: "2026-12-01T00:00:00Z",
            ended: null,
          },
          {
            id: 2,
            client_id: "coc_gone",
            client_name: "Claude Code",
            redirect_host: "localhost:33418",
            created_at: "2026-11-01T00:00:00Z",
            last_used: "2026-11-11T10:00:00Z",
            refresh_expires_at: "2026-11-12T13:18:51Z",
            ended: { at: "2026-11-12T13:18:51Z", reason: "replayed" },
          },
          {
            id: 1,
            client_id: "coc_old",
            client_name: "a local agent",
            redirect_host: "127.0.0.1:51902",
            created_at: "2026-10-01T00:00:00Z",
            last_used: null,
            refresh_expires_at: "2026-11-10T08:00:00Z",
            ended: { at: "2026-11-10T08:00:00Z", reason: "revoked" },
          },
        ],
      },
      { oauth: true },
    );
    renderApp("/profile");

    expect(
      await screen.findByRole("heading", { name: "Recently ended" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /Claude Code \(2026-11-12\): Signed out because an old sign-in token was used again\./,
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/a local agent \(2026-11-10\): You revoked it\./),
    ).toBeInTheDocument();
    // An ended grant is no row of the table, so it offers no revoke.
    expect(
      screen.getByRole("button", { name: "Revoke Claude (#3)" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Revoke Claude Code/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Revoke a local agent/ }),
    ).not.toBeInTheDocument();
  });

  it("still says no client is connected when only ended ones are left", async () => {
    serveAs(
      "editor",
      {
        "/me/oauth-grants": () => [
          {
            id: 4,
            client_id: "coc_gone",
            client_name: "Claude Code",
            redirect_host: "localhost:33418",
            created_at: "2026-11-01T00:00:00Z",
            last_used: null,
            refresh_expires_at: "2026-11-12T00:00:00Z",
            ended: { at: "2026-11-12T00:00:00Z", reason: "client_revoked" },
          },
        ],
      },
      { oauth: true },
    );
    renderApp("/profile");

    expect(
      await screen.findByText(/no client connected yet/i),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Claude Code \(2026-11-12\): The client signed out\./),
    ).toBeInTheDocument();
  });

  it("has one plain sentence for every reason", () => {
    expect(grantEndedText("replayed")).toBe(
      "Signed out because an old sign-in token was used again.",
    );
    expect(grantEndedText("revoked")).toBe("You revoked it.");
    expect(grantEndedText("client_revoked")).toBe("The client signed out.");
    for (const reason of ["replayed", "revoked", "client_revoked"] as const) {
      expect(grantEndedText(reason)).not.toMatch(/[\u2013\u2014]/);
    }
  });

  it("says so when no client is connected yet", async () => {
    serveAs("viewer", {}, { oauth: true });
    renderApp("/profile");

    expect(
      await screen.findByText(/no client connected yet/i),
    ).toBeInTheDocument();
  });

  it("is absent on an instance that never turned OAuth on", async () => {
    serveAs(
      "editor",
      {
        // If the card ignored the gate and fetched anyway, this would be
        // the listing it drew from - present so a leak reads as a real
        // failure rather than an accidental empty-state pass.
        "/me/oauth-grants": () => [
          {
            id: 1,
            client_id: "coc_1a2b3c",
            client_name: "Claude",
            redirect_host: "claude.ai",
            created_at: "2026-08-29T09:12:44Z",
            last_used: null,
            refresh_expires_at: "2026-10-08T04:00:00Z",
          },
        ],
      },
      { oauth: false },
    );
    renderApp("/profile");

    // Agent access is the marker that the screen finished rendering, so the
    // absence below is an absence rather than a race with the initial load.
    await screen.findByRole("heading", { name: "Agent access" });
    expect(
      screen.queryByRole("heading", { name: "Connected clients" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Claude")).not.toBeInTheDocument();
    // And the gate is on the fetch too, not only the render.
    expect(
      apiMock.mock.calls.some(([path]) =>
        String(path).startsWith("/me/oauth-grants"),
      ),
    ).toBe(false);
  });

  it("is present on an instance that serves OAuth", async () => {
    serveAs("editor", { "/me/oauth-grants": () => [] }, { oauth: true });
    renderApp("/profile");

    expect(
      await screen.findByRole("heading", { name: "Connected clients" }),
    ).toBeInTheDocument();
  });

  it("revokes a grant and shows the server's refusal word for word", async () => {
    let listing = [
      {
        id: 9,
        client_id: "coc_1a2b3c",
        client_name: "Claude",
        redirect_host: "claude.ai",
        created_at: "2026-08-01T00:00:00Z",
        last_used: null,
        refresh_expires_at: "2026-10-08T04:00:00Z",
      },
    ];
    let refuse = true;
    serveAs(
      "editor",
      {
        "/me/oauth-grants": () => listing,
        "/me/oauth-grants/9": (_path, init) => {
          if (init?.method === "DELETE") {
            if (refuse) {
              throw new ApiProblem(
                404,
                "not found",
                "no such connected client: it may already have been revoked",
              );
            }
            listing = [];
          }
          return undefined;
        },
      },
      { oauth: true },
    );
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Revoke Claude (#9)" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Confirm revoke Claude (#9)" }),
    );

    // The server's own refusal, word for word.
    expect(
      await screen.findByText(/no such connected client/i),
    ).toBeInTheDocument();

    refuse = false;
    await userEvent.click(
      screen.getByRole("button", { name: "Revoke Claude (#9)" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Confirm revoke Claude (#9)" }),
    );

    expect(
      await screen.findByText(/no client connected yet/i),
    ).toBeInTheDocument();
  });

  it("treats a 404 on revoke as already gone, not a red error left on screen", async () => {
    let listing = [
      {
        id: 11,
        client_id: "coc_1a2b3c",
        client_name: "Claude",
        redirect_host: "claude.ai",
        created_at: "2026-08-01T00:00:00Z",
        last_used: null,
        refresh_expires_at: "2026-10-08T04:00:00Z",
      },
    ];
    serveAs(
      "editor",
      {
        "/me/oauth-grants": () => listing,
        "/me/oauth-grants/11": (_path, init) => {
          if (init?.method === "DELETE") {
            // Already gone - revoked from another tab, say, or simply
            // expired - so the store no longer holds it even though this
            // is the first time THIS button asked. The server answers 404,
            // and the button's job (make sure it is disconnected) is done
            // either way.
            listing = [];
            throw new ApiProblem(
              404,
              "not found",
              "no such connected client: it may already have been revoked",
            );
          }
          return undefined;
        },
      },
      { oauth: true },
    );
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Revoke Claude (#11)" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Confirm revoke Claude (#11)" }),
    );

    // The server's exact sentence, but as a neutral notice - never
    // `role="alert"` - because a grant that is already gone is not a
    // failure of this press.
    const notice = await screen.findByText(/no such connected client/i);
    expect(notice).toHaveAttribute("role", "status");
    // And the list is refreshed rather than the stale row left behind.
    expect(
      await screen.findByText(/no client connected yet/i),
    ).toBeInTheDocument();
    expect(screen.queryByText("Claude")).not.toBeInTheDocument();
  });
});

describe("the SSO identity card", () => {
  it("is absent when there is no provider and no identity to show", async () => {
    serveAs("editor");
    renderApp("/profile");

    // The GitHub card is the marker that the screen finished rendering, so
    // the absence below is an absence rather than a race.
    await screen.findByRole("heading", { name: "GitHub identity" });
    expect(
      screen.queryByRole("heading", { name: "SSO identity" }),
    ).not.toBeInTheDocument();
  });

  it("starts a link with a POST rather than a navigable link", async () => {
    serveAs(
      "editor",
      withProvider({
        "/auth/oidc/login": () => ({ location: "https://idp.example/auth" }),
      }),
    );
    renderApp("/profile");

    expect(
      await screen.findByRole("heading", { name: "SSO identity" }),
    ).toBeInTheDocument();
    expect(
      await screen.findByText(/no provider identity linked/i),
    ).toBeInTheDocument();
    // Never an anchor: the session cookie is SameSite=Lax, so a GET that
    // started a link could be started by any other origin. A POST cannot be
    // sent cross-site with the cookie, and `api` attaches the CSRF token.
    expect(
      screen.queryByRole("link", { name: /^Link Contoso$/ }),
    ).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Link Contoso" }));

    await waitFor(() => {
      expect(
        apiMock.mock.calls.some(
          ([path, init]) =>
            path === "/auth/oidc/login" && init?.method === "POST",
        ),
      ).toBe(true);
    });
    // Where the browser goes next is the server's answer, navigated to as a
    // whole page. jsdom cannot follow that, so what is asserted here is the
    // request; `crates/service/tests/auth/oidc.rs` drives the rest of the journey.
  });

  it("shows the server's words when a link cannot be started", async () => {
    serveAs(
      "editor",
      withProvider({
        "/auth/oidc/login": () => {
          throw new ApiProblem(
            404,
            "not found",
            "this instance has no single sign-on provider configured",
          );
        },
      }),
    );
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Link Contoso" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "this instance has no single sign-on provider configured",
    );
  });

  it("never claims nothing is linked while the read is in flight or failed", async () => {
    // The read never lands. What must NOT appear is the sentence for an
    // account with nothing linked, which for an account that signs in through
    // a provider is the opposite of the truth, and the button that would
    // start a second link beside the one it already holds.
    let settle: (() => void) | undefined;
    serveAs(
      "editor",
      withProvider({
        "/me/identity-links": () =>
          new Promise((resolve) => {
            settle = () => {
              resolve({ links: [linkPayload()], has_password: false });
            };
          }),
      }),
    );
    renderApp("/profile");

    expect(
      await screen.findByText(/reading your provider identities/i),
    ).toBeInTheDocument();
    expect(screen.queryByText(/no provider identity linked/i)).toBeNull();
    expect(screen.queryByRole("button", { name: "Link Contoso" })).toBeNull();

    settle?.();
    expect(await screen.findByText("https://idp.example")).toBeInTheDocument();
  });

  it("says so when the identity read fails, rather than that there is nothing", async () => {
    serveAs(
      "editor",
      withProvider({
        "/me/identity-links": () => {
          throw new ApiProblem(
            500,
            "internal",
            "the accounts database is busy",
          );
        },
      }),
    );
    renderApp("/profile");

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "the accounts database is busy",
    );
    expect(screen.queryByText(/no provider identity linked/i)).toBeNull();
    expect(screen.queryByRole("button", { name: "Link Contoso" })).toBeNull();
  });

  it("names an unfamiliar linker rather than calling it this profile", async () => {
    serveAs("editor", {
      "/me/identity-links": () => ({
        links: [linkPayload({ linked_by: "migration" })],
        has_password: true,
      }),
    });
    renderApp("/profile");

    expect(await screen.findByText(/linked by migration/i)).toBeInTheDocument();
  });

  it("shows a linked identity, who linked it, and unlinks it", async () => {
    let links = [linkPayload({ linked_by: "jit" })];
    serveAs(
      "editor",
      withProvider({
        "/me/identity-links": () => ({ links, has_password: true }),
        "/me/identity-links/https%3A%2F%2Fidp.example": () => {
          links = [];
          return undefined;
        },
      }),
    );
    renderApp("/profile");

    expect(await screen.findByText("https://idp.example")).toBeInTheDocument();
    expect(screen.getByText(/created by a first sign-on/i)).toBeInTheDocument();
    // With one identity already held, there is nothing to link: one identity
    // per provider is the rule.
    expect(
      screen.queryByRole("link", { name: "Link Contoso" }),
    ).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Unlink" }));

    expect(await screen.findByRole("status")).toHaveTextContent(
      "The identity is unlinked.",
    );
    expect(
      await screen.findByText(/no provider identity linked/i),
    ).toBeInTheDocument();
  });

  it("shows the server's own words when the last way in cannot be given up", async () => {
    serveAs(
      "editor",
      withProvider({
        "/me/identity-links": () => ({
          links: [linkPayload({ linked_by: "jit" })],
          has_password: false,
        }),
        "/me/identity-links/https%3A%2F%2Fidp.example": () => {
          throw new ApiProblem(
            409,
            "conflict",
            "this is the only way into account 'ada': it has no password, so unlinking " +
              "its last identity would leave nobody able to sign in - give it a password " +
              "first with `crystalline users passwd ada`",
          );
        },
      }),
    );
    renderApp("/profile");

    // The card says so before anybody presses anything, and the refusal says
    // it again in the server's own sentence.
    expect(
      await screen.findByText(/this identity is its only way in/i),
    ).toBeInTheDocument();

    await userEvent.click(
      await screen.findByRole("button", { name: "Unlink" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "crystalline users passwd ada",
    );
    expect(screen.getByText("https://idp.example")).toBeInTheDocument();
  });

  it("keeps showing a link whose provider was turned off, so it can be given up", async () => {
    serveAs("editor", {
      "/me/identity-links": () => ({
        links: [linkPayload({ linked_by: "cli" })],
        has_password: true,
      }),
    });
    renderApp("/profile");

    expect(
      await screen.findByRole("heading", { name: "SSO identity" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/linked by an administrator/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Unlink" })).toBeInTheDocument();
  });
});

describe("the GitHub App link card", () => {
  it("shows Connect GitHub when an App is configured and no link exists", async () => {
    serveAs("editor", {
      "/me/github-link": () => linkView(),
      "/me/github-link/start": () => ({
        authorize_url: "https://github.example/login/oauth/authorize?state=s",
      }),
    });
    renderApp("/profile");

    const connect = await screen.findByRole("button", {
      name: "Connect GitHub",
    });
    // The App replaces the older ways in: no device flow, no pasted token.
    expect(
      screen.queryByRole("button", { name: /connect with github/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText(/personal access token/i),
    ).not.toBeInTheDocument();

    await userEvent.click(connect);
    await waitFor(() => {
      expect(navigateMock).toHaveBeenCalledWith(
        "https://github.example/login/oauth/authorize?state=s",
      );
    });
    expect(
      apiMock.mock.calls.some(
        ([path, init]) =>
          path === "/me/github-link/start" && init?.method === "POST",
      ),
    ).toBe(true);
  });

  it("shows the login and Unlink when linked", async () => {
    let link = LINKED;
    serveAs("editor", {
      "/me/github-link": (_path, init) => {
        if (init?.method === "DELETE") {
          link = linkView();
          return { linked: false, was_linked: true, warning: null };
        }
        return link;
      },
    });
    renderApp("/profile");

    expect(await screen.findByText("Linked to @ada-gh")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Connect GitHub" }),
    ).not.toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "Unlink GitHub" }),
    );
    expect(
      await screen.findByRole("button", { name: "Connect GitHub" }),
    ).toBeInTheDocument();
    expect(screen.queryByText("Linked to @ada-gh")).not.toBeInTheDocument();
  });

  it("shows the server's warning when GitHub could not revoke the token", async () => {
    serveAs("editor", {
      "/me/github-link": (_path, init) =>
        init?.method === "DELETE"
          ? {
              linked: false,
              was_linked: true,
              warning: "unlinked here; revoke it in your GitHub settings",
            }
          : LINKED,
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Unlink GitHub" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "unlinked here; revoke it in your GitHub settings",
    );
  });

  it("asks to connect again when the GitHub access expired", async () => {
    serveAs("editor", {
      "/me/github-link": () => linkView({ ...LINKED, token_live: false }),
    });
    renderApp("/profile");

    expect(
      await screen.findByText(
        "GitHub access expired. Connect again to share as yourself.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Connect GitHub" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Unlink GitHub" }),
    ).toBeInTheDocument();
  });

  it("shows the id-taken sentence from the query string", async () => {
    serveAs("editor", { "/me/github-link": () => linkView() });
    renderApp("/profile?github_error=id_taken");

    expect(
      await screen.findByText(
        "This GitHub account is already linked to another Crystalline account. An admin can unlink it there.",
      ),
    ).toBeInTheDocument();
  });

  it.each([
    ["state", /could not be matched to a link you started/i],
    ["session", /you were signed out/i],
    ["denied", /cancelled at GitHub/i],
    ["failed", /could not be linked/i],
    ["something-new", /could not be linked/i],
    ["constructor", /could not be linked/i],
  ])("says plainly what github_error=%s means", async (code, sentence) => {
    serveAs("editor", { "/me/github-link": () => linkView() });
    renderApp(`/profile?github_error=${code}`);

    expect(await screen.findByText(sentence)).toBeInTheDocument();
  });

  it("confirms a finished link from the query string", async () => {
    serveAs("editor", { "/me/github-link": () => LINKED });
    renderApp("/profile?github=linked");

    expect(
      await screen.findByText("Your GitHub account is linked."),
    ).toHaveAttribute("role", "status");
  });

  it("is offered to a viewer too, since a link is who you are at GitHub", async () => {
    serveAs("viewer", { "/me/github-link": () => linkView() });
    renderApp("/profile");

    expect(
      await screen.findByRole("button", { name: "Connect GitHub" }),
    ).toBeInTheDocument();
  });

  it("offers no way to link or unlink on a read-only instance", async () => {
    serveAs("editor", { "/me/github-link": () => LINKED }, { read_only: true });
    renderApp("/profile");

    expect(await screen.findByText("Linked to @ada-gh")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Unlink GitHub" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Connect GitHub" }),
    ).not.toBeInTheDocument();
  });

  it("keeps the old identity card without an App", async () => {
    serveAs("editor");
    renderApp("/profile");

    expect(
      await screen.findByRole("button", { name: /connect with github/i }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Connect GitHub" }),
    ).not.toBeInTheDocument();
  });

  it("shows the old card's disconnect warning in the server's words", async () => {
    serveAs("editor", {
      "/me/github-identity": (_path, init) =>
        init?.method === "DELETE"
          ? identityPayload({
              warning: "unlinked here; revoke it in your GitHub settings",
            })
          : CONNECTED,
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Disconnect" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: /confirm disconnect/i }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "unlinked here; revoke it in your GitHub settings",
    );
  });

  it("offers to forget an older sign-in when an App is configured and no link exists", async () => {
    let identity = CONNECTED;
    const forgotten = vi.fn();
    serveAs("editor", {
      "/me/github-link": () => linkView(),
      "/me/github-identity": (_path, init) => {
        if (init?.method === "DELETE") {
          forgotten();
          identity = identityPayload();
          return identity;
        }
        return identity;
      },
    });
    renderApp("/profile");

    expect(
      await screen.findByText(
        "You still have an older GitHub sign-in as @octo.",
      ),
    ).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Forget it" }));
    await waitFor(() => {
      expect(forgotten).toHaveBeenCalled();
    });
    expect(
      await screen.findByText("The older sign-in is forgotten."),
    ).toHaveAttribute("role", "status");
    await waitFor(() => {
      expect(
        screen.queryByText(/older GitHub sign-in/),
      ).not.toBeInTheDocument();
    });
  });

  it("names no account for an older sign-in GitHub gave no login for", async () => {
    serveAs("editor", {
      "/me/github-link": () => linkView(),
      "/me/github-identity": () =>
        identityPayload({ ...CONNECTED, login: null }),
    });
    renderApp("/profile");

    expect(
      await screen.findByText("You still have an older GitHub sign-in."),
    ).toBeInTheDocument();
    expect(screen.queryByText(/sign-in as @/)).not.toBeInTheDocument();
  });

  it("does not offer to forget an older sign-in when linked", async () => {
    serveAs("editor", {
      "/me/github-link": () => LINKED,
      "/me/github-identity": () => CONNECTED,
    });
    renderApp("/profile");

    expect(await screen.findByText("Linked to @ada-gh")).toBeInTheDocument();
    expect(screen.queryByText(/older GitHub sign-in/)).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Forget it" }),
    ).not.toBeInTheDocument();
    // The personal slot is the App token then, so it is not even read.
    expect(identityCalls()).toHaveLength(0);
  });

  it("re-reads the identity the share dialog uses after an unlink", async () => {
    serveAs("editor", {
      "/me/github-link": (_path, init) =>
        init?.method === "DELETE"
          ? { linked: false, was_linked: true, warning: null }
          : LINKED,
    });
    const invalidate = vi.spyOn(QueryClient.prototype, "invalidateQueries");
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Unlink GitHub" }),
    );
    await waitFor(() => {
      expect(invalidate).toHaveBeenCalledWith({
        queryKey: ["me-github-identity"],
      });
    });
    invalidate.mockRestore();
  });

  it.each([422, 409])(
    "tells a non-admin to ask an admin when the start is refused %s",
    async (status) => {
      serveAs("editor", {
        "/me/github-link": () => linkView(),
        "/me/github-link/start": () => {
          throw new ApiProblem(
            status,
            "conflict",
            "an admin adds it with: crystalline github app set",
          );
        },
      });
      renderApp("/profile");

      await userEvent.click(
        await screen.findByRole("button", { name: "Connect GitHub" }),
      );
      expect(await screen.findByRole("alert")).toHaveTextContent(
        "Ask an admin to check the GitHub App.",
      );
      expect(
        screen.queryByText(/crystalline github app set/),
      ).not.toBeInTheDocument();
    },
  );

  const REASONS: [string, string][] = [
    [
      "no_client_secret",
      "The GitHub App on this instance has no client secret yet, so it cannot link accounts. Ask an admin to add it.",
    ],
    [
      "no_public_url",
      "This instance does not know its public address yet, so GitHub cannot send you back. Ask an admin to set it.",
    ],
    [
      "app_unusable",
      "The GitHub App on this instance is set up but cannot be used right now. Ask an admin to check its settings.",
    ],
    [
      "keychain",
      "This instance cannot read the GitHub App's stored keys right now. Ask an admin to check the server.",
    ],
    [
      "no_app",
      "This instance has no GitHub App yet. Ask an admin to set one up.",
    ],
  ];

  it.each(
    (["viewer", "editor"] as const).flatMap((role) =>
      REASONS.map(([reason, text]) => [role, reason, text] as const),
    ),
  )(
    "tells a %s why the start is refused for %s",
    async (role, reason, text) => {
      serveAs(role, {
        "/me/github-link": () => linkView(),
        "/me/github-link/start": () => {
          throw new ApiProblem(
            409,
            "conflict",
            "an admin adds it with: crystalline github app set",
            { reason },
          );
        },
      });
      renderApp("/profile");

      await userEvent.click(
        await screen.findByRole("button", { name: "Connect GitHub" }),
      );
      expect(await screen.findByRole("alert")).toHaveTextContent(text);
      expect(
        screen.queryByText(/crystalline github app set/),
      ).not.toBeInTheDocument();
    },
  );

  it("tells a viewer about an unusable App on the 500 it answers", async () => {
    serveAs("viewer", {
      "/me/github-link": () => linkView(),
      "/me/github-link/start": () => {
        throw new ApiProblem(
          500,
          "internal error",
          "The GitHub App credential is unusable: check the private key",
          { reason: "app_unusable" },
        );
      },
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Connect GitHub" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The GitHub App on this instance is set up but cannot be used right now. Ask an admin to check its settings.",
    );
  });

  it.each([["something_new"], [7]])(
    "falls back to who to ask for an unknown reason %s",
    async (reason) => {
      serveAs("editor", {
        "/me/github-link": () => linkView(),
        "/me/github-link/start": () => {
          throw new ApiProblem(409, "conflict", "the server's words", {
            reason,
          });
        },
      });
      renderApp("/profile");

      await userEvent.click(
        await screen.findByRole("button", { name: "Connect GitHub" }),
      );
      expect(await screen.findByRole("alert")).toHaveTextContent(
        "Ask an admin to check the GitHub App.",
      );
    },
  );

  it("shows an admin the server's own fix even with a reason", async () => {
    serveAs("admin", {
      "/me/github-link": () => linkView(),
      "/me/github-link/start": () => {
        throw new ApiProblem(
          409,
          "conflict",
          "an admin adds it with: crystalline github app set",
          { reason: "no_client_secret" },
        );
      },
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Connect GitHub" }),
    );
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(
      "an admin adds it with: crystalline github app set",
    );
    expect(alert).not.toHaveTextContent(/no client secret yet/);
  });

  it("shows an admin the server's own fix when the start is refused", async () => {
    serveAs("admin", {
      "/me/github-link": () => linkView(),
      "/me/github-link/start": () => {
        throw new ApiProblem(
          409,
          "conflict",
          "an admin adds it with: crystalline github app set",
        );
      },
    });
    renderApp("/profile");

    await userEvent.click(
      await screen.findByRole("button", { name: "Connect GitHub" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "an admin adds it with: crystalline github app set",
    );
  });

  it("shows and clears a landing even when the link read fails", async () => {
    serveAs("editor", {
      "/me/github-link": () => {
        throw new ApiProblem(500, "internal", "the read failed");
      },
    });
    render(
      <MemoryRouter initialEntries={["/profile?github_error=id_taken"]}>
        <App />
        <LocationProbe />
      </MemoryRouter>,
    );

    expect(
      await screen.findByText(
        "This GitHub account is already linked to another Crystalline account. An admin can unlink it there.",
      ),
    ).toBeInTheDocument();
    // No App could be read, so the older card shows, and the landing is
    // still taken out of the address.
    expect(
      await screen.findByRole("button", { name: /connect with github/i }),
    ).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.getByTestId("location")).toHaveTextContent(/^\/profile$/);
    });
  });

  it.each([
    [
      "personal with an App",
      "editor",
      { share_identity: "personal" },
      "The GitHub account your shared work is opened under. Link it once, and what you share from this instance carries your GitHub name.",
    ],
    [
      "instance with an App",
      "editor",
      { share_identity: "instance" },
      "This instance opens proposals with its GitHub App. Link your GitHub account so your commits carry your name.",
    ],
    [
      "instance with an App, for someone who may not share",
      "viewer",
      { share_identity: "instance" },
      "Link your GitHub account once so this instance knows who you are there.",
    ],
    [
      "personal with no App",
      "editor",
      { share_identity: "personal", app_configured: false },
      "The account your shared work is opened under. Proposals you share from this instance carry your GitHub name, so it needs write access to the repositories your team domains track.",
    ],
    [
      "instance with no App",
      "editor",
      { share_identity: "instance", app_configured: false },
      "Shares from this instance use its own GitHub credential. Your GitHub identity is used only if an admin turns on personal sharing.",
    ],
  ] as const)(
    "says whose name a proposal carries: %s",
    async (_label, role, link, text) => {
      serveAs(role, { "/me/github-link": () => linkView(link) });
      renderApp("/profile");
      const line = await screen.findByText(text);
      expect(line).toBeInTheDocument();
      expect(line.textContent).not.toMatch(/crystalline |share_identity/);
    },
  );

  it("tells a viewer what linking is for", async () => {
    serveAs("viewer", { "/me/github-link": () => linkView() });
    renderApp("/profile");

    expect(
      await screen.findByText(
        "Link your GitHub account once so this instance knows who you are there.",
      ),
    ).toBeInTheDocument();
  });

  it("sends a browser without an account back to the sign-in, keeping the landing", async () => {
    serveAs("editor", {
      "/auth/me": () => meResponse({ user: null, anonymous: true }),
      "/auth/providers": () => ({
        local: true,
        oidc: { enabled: true, name: "Contoso" },
      }),
    });
    renderApp("/profile?github_error=session");

    // The provider button carries the interrupted destination as return_to,
    // which is how `from` (with its search) is observable here.
    const button = await screen.findByRole("link", {
      name: "Sign in with Contoso",
    });
    expect(button).toHaveAttribute(
      "href",
      "/api/v1/auth/oidc/login?return_to=%2Fprofile%3Fgithub_error%3Dsession",
    );
  });
});
